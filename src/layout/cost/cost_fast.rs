//! `cost_fast` 热路径 + `cost_breakdown` 诊断。
//!
//! - `cost_fast`: 复用预计算的 `SAContext` + `CostBuf`, SA 主循环里每步调一次
//! - `cost_breakdown`: 返回成本 + 各项明细, 调试 / profile 用
//! - `cost_breakdown_inner`: breakdown 的实际实现, 跟 cost_fast 共享热路径

use crate::circuit::Circuit;
use crate::layout::breadboard::{Breadboard, Polarity};
use crate::layout::placement::{BBox, Rotation};

use super::Weights;
use super::context::{CostBuf, SAContext};
use super::mst::mst_wire_length_fast;
use super::state::SAState;

// `cp_*` 宏由 profile.rs 用 `#[macro_export]` 导出到 crate 根, 这里 import 进本模块作用域
use crate::{cp_bbox, cp_call, cp_collect, cp_compact, cp_mst, cp_pin, cp_rail};

// SA 走线估算: 用 net_buckets (Vec<Vec<usize>>) 代替 HashMap

/// 快速版本: 复用预计算的 context 和 buffers。
/// 在 simulate() 的热循环里替代 `cost()`。
pub(crate) fn cost_fast(
    
        state: &SAState,
        circuit: &Circuit,
        board: &Breadboard,
        bridged_pins: &[(crate::circuit::PinId, crate::layout::breadboard::HoleId)],
        w: &Weights,
        ctx: &SAContext,
        buf: &mut CostBuf,
    ) -> f64 {
        cp_call!();
        let _t_collect = std::time::Instant::now();
        buf.clear();
        buf.pin_idx_sorted.clear(); // fused: 不在 clear() 里 clear, 这里手动

        let cols_i = board.cols() as i32;
        let n_comps = state.placeable.len();
        // fused: 在 collect 阶段同时计算 oob_count + 填 pin_idx_sorted (跳过虚拟 + 越界)。
        // 省两遍对 buf.holes 的扫描。
        let mut oob_count = 0u32;

        // 1. 收集所有 pin 的 (col, row, rail_id) 和所属 net, 以及每个元件的 bbox。
        for (idx, _comp_id) in state.placeable.iter().enumerate() {
            if state.bridged[idx] {
                // bridged bbox 已预计算进 ctx (fill_bridged_bboxes), O(1) 查表。
                // 若 ctx 未预计算 (e.g. `cost()` 后向兼容接口从外部调), 退到实时计算。
                let precomputed = ctx.comp_infos[idx]
                    .bridged_bboxes
                    .as_ref()
                    .and_then(|bbs| bbs.get(state.active_bridge_idx[idx]).copied())
                    .or_else(|| {
                        state.active_bridge_pair(idx).map(|pair| {
                            let p0 = board.hole(pair[0].0).position;
                            let p1 = board.hole(pair[1].0).position;
                            BBox {
                                min_x: p0.x.min(p1.x),
                                max_x: p0.x.max(p1.x),
                                min_y: p0.y.min(p1.y),
                                max_y: p0.y.max(p1.y),
                            }
                        })
                    });
                buf.bboxes.push(precomputed);
                continue;
            }

            let comp_info = &ctx.comp_infos[idx];
            let px = state.x[idx];
            let py = state.y[idx];
            let ri = super::context::rot_index(state.rotation[idx]);

            for pin_data in &comp_info.pins {
                let (offsets, net) = pin_data;
                let offset = offsets[ri];
                let x = px + offset.x;
                let y = py + offset.y;
                let rail_id = board.rail_id_at(x, y);
                let hole_idx = buf.holes.len();
                buf.holes.push((x, y, rail_id));
                buf.nets.push(*net);
                buf.is_virtual.push(false);
                if rail_id == u32::MAX {
                    oob_count += 1;
                } else {
                    buf.pin_idx_sorted.push(hole_idx);
                    buf.rail_map[rail_id as usize].push(*net);
                    if let Some(n) = net {
                        buf.net_buckets[n.0].push(hole_idx);
                    }
                }
            }

            // BBox: translate precomputed R0 bbox according to rotation
            let bbox_r0 = &comp_info.bbox_r0;
            let world_bbox = match state.rotation[idx] {
                Rotation::R0 => BBox {
                    min_x: bbox_r0.min_x + px,
                    max_x: bbox_r0.max_x + px,
                    min_y: bbox_r0.min_y + py,
                    max_y: bbox_r0.max_y + py,
                },
                Rotation::R180 => BBox {
                    min_x: -bbox_r0.max_x + px,
                    max_x: -bbox_r0.min_x + px,
                    min_y: -bbox_r0.max_y + py,
                    max_y: -bbox_r0.min_y + py,
                },
                Rotation::R90 => BBox {
                    min_x: -bbox_r0.max_y + px,
                    max_x: -bbox_r0.min_y + px,
                    min_y: bbox_r0.min_x + py,
                    max_y: bbox_r0.max_x + py,
                },
                Rotation::R270 => BBox {
                    min_x: bbox_r0.min_y + px,
                    max_x: bbox_r0.max_y + px,
                    min_y: -bbox_r0.max_x + py,
                    max_y: -bbox_r0.min_x + py,
                },
            };
            buf.bboxes.push(Some(world_bbox));
        }

    // 1b. 注入用户预摆的 bridged 元件的 pin (位置已预计算进 ctx)
    // 当 ctx 未预计算 (外部 cost() 调用且测试手工造 state 时), 退到实时算。
    if !ctx.external_bridged_world.is_empty() {
        for &(x, y, rail_id, net) in &ctx.external_bridged_world {
            let hole_idx = buf.holes.len();
            buf.holes.push((x, y, rail_id));
            buf.nets.push(net);
            buf.is_virtual.push(false);
            if rail_id == u32::MAX {
                oob_count += 1;
            } else {
                buf.pin_idx_sorted.push(hole_idx);
                buf.rail_map[rail_id as usize].push(net);
                if let Some(n) = net {
                    buf.net_buckets[n.0].push(hole_idx);
                }
            }
        }
    } else {
        for &(pin_id, hole_id) in bridged_pins {
            let pin = &circuit.pins[pin_id.0];
            let pos = board.hole(hole_id).position;
            let rail_id = board.rail_id_of(hole_id);
            let hole_idx = buf.holes.len();
            buf.holes.push((pos.x, pos.y, rail_id));
            buf.nets.push(pin.net);
            buf.is_virtual.push(false);
            if rail_id == u32::MAX {
                oob_count += 1;
            } else {
                buf.pin_idx_sorted.push(hole_idx);
                buf.rail_map[rail_id as usize].push(pin.net);
                if let Some(n) = pin.net {
                    buf.net_buckets[n.0].push(hole_idx);
                }
            }
        }
    }

    // 1b'. 注入 SA Toggle 后的 bridged 元件的 pin (位置已预计算进 ctx)
    for idx in 0..n_comps {
        if !state.bridged[idx] {
            continue;
        }
        let active = state.active_bridge_idx[idx];
        let bridged_world = ctx.comp_infos[idx]
            .bridged_pair_world
            .as_ref()
            .map(|v| &v[active]);
        if let Some(world_pair) = bridged_world {
            for &(x, y, rail_id, net) in world_pair {
                let hole_idx = buf.holes.len();
                buf.holes.push((x, y, rail_id));
                buf.nets.push(net);
                buf.is_virtual.push(false);
                if rail_id == u32::MAX {
                    oob_count += 1;
                } else {
                    buf.pin_idx_sorted.push(hole_idx);
                    buf.rail_map[rail_id as usize].push(net);
                    if let Some(n) = net {
                        buf.net_buckets[n.0].push(hole_idx);
                    }
                }
            }
        } else {
            // 回退: 实时 board 查询
            let pair = state
                .active_bridge_pair(idx)
                .expect("bridged=true 必有 pin pair");
            for &(h, pin_id) in &pair {
                let pin = &circuit.pins[pin_id.0];
                let pos = board.hole(h).position;
                let rail_id = board.rail_id_of(h);
                let hole_idx = buf.holes.len();
                buf.holes.push((pos.x, pos.y, rail_id));
                buf.nets.push(pin.net);
                buf.is_virtual.push(false);
                if rail_id == u32::MAX {
                    oob_count += 1;
                } else {
                    buf.pin_idx_sorted.push(hole_idx);
                    buf.rail_map[rail_id as usize].push(pin.net);
                    if let Some(n) = pin.net {
                        buf.net_buckets[n.0].push(hole_idx);
                    }
                }
            }
        }
    }

    // 1c. 注入 power rail 虚拟 pin (位置已预计算进 ctx)
    if !ctx.power_anchor_world.is_empty() {
        for (i, &(x, y, rail_id)) in ctx.power_anchor_world.iter().enumerate() {
            let net_id = ctx.power_anchor_nets[i];
            buf.holes.push((x, y, rail_id));
            buf.nets.push(net_id);
            buf.is_virtual.push(true);
            buf.rail_map[rail_id as usize].push(net_id);
            if let Some(n) = net_id {
                buf.net_buckets[n.0].push(buf.holes.len() - 1);
            }
        }
    } else if let Some(binding) = board.power_rail_binding() {
        for (polarity, net_id) in [
            (Polarity::Negative, binding.negative),
            (Polarity::Positive, binding.positive),
        ] {
            if let Some(anchor) = board.power_rail_anchor(polarity) {
                let pos = board.hole(anchor).position;
                let rail_id = board.rail_id_of(anchor);
                buf.holes.push((pos.x, pos.y, rail_id));
                buf.nets.push(Some(net_id));
                buf.is_virtual.push(true);
                buf.rail_map[rail_id as usize].push(Some(net_id));
                buf.net_buckets[net_id.0].push(buf.holes.len() - 1);
            }
        }
    }

    // 2. OOB: 已 fused 到上面的 collect 阶段。
    cp_collect!(_t_collect.elapsed().as_nanos() as u64);
    let _t_pin = std::time::Instant::now();
    // 3. Pin 碰撞: pin_idx_sorted 已在 collect 阶段填好 (跳过虚拟 + 越界)。排序。
    buf.pin_idx_sorted.sort_unstable_by_key(|&i| unsafe {
        // 安全: i < holes.len()
        *buf.holes.get_unchecked(i)
    });
    let mut coll_count = 0u32;
    let mut j = 0;
    while j < buf.pin_idx_sorted.len() {
        let mut k = j + 1;
        while k < buf.pin_idx_sorted.len()
            && buf.holes[buf.pin_idx_sorted[k]] == buf.holes[buf.pin_idx_sorted[j]]
        {
            k += 1;
        }
        let n = k - j;
        if n > 1 {
            coll_count += (n - 1) as u32;
        }
        j = k;
    }
    cp_pin!(_t_pin.elapsed().as_nanos() as u64);
    let _t_bbox = std::time::Instant::now();

    // 4. bbox 碰撞
    let mut bbox_overlap_count = 0u32;
    for i in 0..buf.bboxes.len() {
        let Some(bi) = buf.bboxes[i] else { continue };
        for j in (i + 1)..buf.bboxes.len() {
            let Some(bj) = buf.bboxes[j] else { continue };
            if !bi.overlaps(&bj) {
                continue;
            }
            for pos in bi.iter_cells() {
                if pos.x >= bj.min_x && pos.x <= bj.max_x && pos.y >= bj.min_y && pos.y <= bj.max_y
                {
                    bbox_overlap_count += 1;
                }
            }
        }
    }
    cp_bbox!(_t_bbox.elapsed().as_nanos() as u64);
    let _t_mst = std::time::Instant::now();

    // 5+6: net_buckets 和 rail_map 都已在 collect 阶段填好。直接扫。
    let mut mst_sum = 0.0f64;
    for bucket in &buf.net_buckets {
        if bucket.len() < 2 {
            continue;
        }
        mst_sum += mst_wire_length_fast(bucket, &buf.holes);
    }
    cp_mst!(_t_mst.elapsed().as_nanos() as u64);
    let _t_rail = std::time::Instant::now();

    let mut col_conflict_pairs = 0usize;
    for rail_owners in buf.rail_map.iter() {
        if rail_owners.len() < 2 {
            continue;
        }
        let base = rail_owners[0];
        for owner in &rail_owners[1..] {
            if *owner != base {
                col_conflict_pairs += 1;
            }
        }
    }
    cp_rail!(_t_rail.elapsed().as_nanos() as u64);
    let _t_compact = std::time::Instant::now();

    // 7. 紧凑度: 按 rail 分组
    for bbox_opt in buf.bboxes.iter() {
        let Some(bbox) = bbox_opt else { continue };
        if bbox.min_x < 0
            || bbox.max_x >= cols_i
            || bbox.min_y < 0
            || bbox.min_y >= board.main_rows() as i32
            || board.is_blocked(bbox.min_y as usize)
        {
            continue;
        }
        let rail_top = board.rail_top(bbox.min_y).unwrap_or(bbox.min_y);
        // rail_top ∈ [0, main_rows) 在上几行已保证
        buf.compact_map[rail_top as usize].push(*bbox);
    }
    let mut area_sum = 0.0f64;
    let mut row_squash_penalty = 0.0f64;
    let mut num_occupied_rails = 0usize;
    for cells in buf.compact_map.iter() {
        if cells.is_empty() {
            continue;
        }
        num_occupied_rails += 1;
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        // 用 min_y..=max_y 范围内的去重计数代替 HashSet<i32>: cells 一般很少
        // (单 rail 上 < 5 个元件), bitmap 比 HashSet 快得多。
        let mut row_seen: u32 = 0;
        for b in cells {
            min_x = min_x.min(b.min_x);
            max_x = max_x.max(b.max_x);
            min_y = min_y.min(b.min_y);
            max_y = max_y.max(b.max_y);
            let row_idx = (b.min_y - min_y) as u32;
            if row_idx < 32 {
                row_seen |= 1u32 << row_idx;
            }
        }
        if min_x <= max_x && min_y <= max_y {
            let width = (max_x - min_x + 1) as f64;
            area_sum += width;
        }
        // 纵向挤压: 元件数 n vs 实际占用行数 ny
        let ny = row_seen.count_ones() as usize;
        let n = cells.len();
        if n > ny {
            row_squash_penalty += (n - ny) as f64;
        }
    }
    let rail_cross = if num_occupied_rails >= 2 {
        w.rail_crossing
    } else {
        0.0
    };
    cp_compact!(_t_compact.elapsed().as_nanos() as u64);

    w.mst * mst_sum
        + w.pin_overlap * coll_count as f64
        + w.b_box_overlap * bbox_overlap_count as f64
        + w.column_conflict * col_conflict_pairs as f64
        + w.out_of_bounds * oob_count as f64
        + w.compactness * area_sum
        + w.row_squash * row_squash_penalty
        + rail_cross
}

/// 调试用: 返回成本的同时返回各项明细。一千成本以上的项重点看。
pub(crate) fn cost_breakdown(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, crate::layout::breadboard::HoleId)],
    w: &Weights,
) -> (f64, CostBreakdown) {
    let ctx = SAContext::new(circuit, &state.placeable);
    let mut buf = CostBuf::new(circuit.nets().len(), board.num_rails(), board.main_rows());
    cost_breakdown_inner(state, circuit, board, bridged_pins, w, &ctx, &mut buf)
}

/// 复制 cost_fast 但记录各项。重复是 debug 专趟代价。
fn cost_breakdown_inner(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, crate::layout::breadboard::HoleId)],
    w: &Weights,
    ctx: &SAContext,
    buf: &mut CostBuf,
) -> (f64, CostBreakdown) {
    buf.clear();

    let cols_i = board.cols() as i32;
    let n_comps = state.placeable.len();

    for (idx, _comp_id) in state.placeable.iter().enumerate() {
        if state.bridged[idx] {
            let bridged_bbox = state.active_bridge_pair(idx).map(|pair| {
                let p0 = board.hole(pair[0].0).position;
                let p1 = board.hole(pair[1].0).position;
                BBox {
                    min_x: p0.x.min(p1.x),
                    max_x: p0.x.max(p1.x),
                    min_y: p0.y.min(p1.y),
                    max_y: p0.y.max(p1.y),
                }
            });
            buf.bboxes.push(bridged_bbox);
            continue;
        }

        let comp_info = &ctx.comp_infos[idx];
        let px = state.x[idx];
        let py = state.y[idx];
        let ri = super::context::rot_index(state.rotation[idx]);

        for pin_data in &comp_info.pins {
            let (offsets, net) = pin_data;
            let offset = offsets[ri];
            let x = px + offset.x;
            let y = py + offset.y;
            let rail_id = board
                .at(x, y)
                .map(|h| board.rail_id_of(h))
                .unwrap_or(u32::MAX);
            buf.holes.push((x, y, rail_id));
            buf.nets.push(*net);
            buf.is_virtual.push(false);
        }

        let bbox_r0 = &comp_info.bbox_r0;
        let world_bbox = match state.rotation[idx] {
            Rotation::R0 => BBox {
                min_x: bbox_r0.min_x + px,
                max_x: bbox_r0.max_x + px,
                min_y: bbox_r0.min_y + py,
                max_y: bbox_r0.max_y + py,
            },
            Rotation::R180 => BBox {
                min_x: -bbox_r0.max_x + px,
                max_x: -bbox_r0.min_x + px,
                min_y: -bbox_r0.max_y + py,
                max_y: -bbox_r0.min_y + py,
            },
            Rotation::R90 => BBox {
                min_x: -bbox_r0.max_y + px,
                max_x: -bbox_r0.min_y + px,
                min_y: bbox_r0.min_x + py,
                max_y: bbox_r0.max_x + py,
            },
            Rotation::R270 => BBox {
                min_x: bbox_r0.min_y + px,
                max_x: bbox_r0.max_y + px,
                min_y: -bbox_r0.max_x + py,
                max_y: -bbox_r0.min_x + py,
            },
        };
        buf.bboxes.push(Some(world_bbox));
    }
    for &(pin_id, hole_id) in bridged_pins {
        let pin = &circuit.pins[pin_id.0];
        let pos = board.hole(hole_id).position;
        let rail_id = board.rail_id_of(hole_id);
        buf.holes.push((pos.x, pos.y, rail_id));
        buf.nets.push(pin.net);
        buf.is_virtual.push(false);
    }
    for idx in 0..n_comps {
        if !state.bridged[idx] {
            continue;
        }
        let pair = state.active_bridge_pair(idx).expect("bridged");
        for &(h, pin_id) in &pair {
            let pin = &circuit.pins[pin_id.0];
            let pos = board.hole(h).position;
            let rail_id = board.rail_id_of(h);
            buf.holes.push((pos.x, pos.y, rail_id));
            buf.nets.push(pin.net);
            buf.is_virtual.push(false);
        }
    }
    if let Some(binding) = board.power_rail_binding() {
        for (polarity, net_id) in [
            (Polarity::Negative, binding.negative),
            (Polarity::Positive, binding.positive),
        ] {
            if let Some(anchor) = board.power_rail_anchor(polarity) {
                let pos = board.hole(anchor).position;
                let rail_id = board.rail_id_of(anchor);
                buf.holes.push((pos.x, pos.y, rail_id));
                buf.nets.push(Some(net_id));
                buf.is_virtual.push(true);
            }
        }
    }
    let mut oob_count = 0u32;
    for &(_, _, rail_id) in &buf.holes {
        if rail_id == u32::MAX {
            oob_count += 1;
        }
    }
    let mut coll_count = 0u32;
    buf.pin_idx_sorted.clear();
    for (i, hole) in buf.holes.iter().enumerate() {
        if hole.2 != u32::MAX && !buf.is_virtual[i] {
            buf.pin_idx_sorted.push(i);
        }
    }
    buf.pin_idx_sorted
        .sort_unstable_by_key(|&i| unsafe { *buf.holes.get_unchecked(i) });
    let mut j = 0;
    while j < buf.pin_idx_sorted.len() {
        let mut k = j + 1;
        while k < buf.pin_idx_sorted.len()
            && buf.holes[buf.pin_idx_sorted[k]] == buf.holes[buf.pin_idx_sorted[j]]
        {
            k += 1;
        }
        let n = k - j;
        if n > 1 {
            coll_count += (n - 1) as u32;
        }
        j = k;
    }
    let mut bbox_overlap_count = 0u32;
    for i in 0..buf.bboxes.len() {
        let Some(bi) = buf.bboxes[i] else { continue };
        for j in (i + 1)..buf.bboxes.len() {
            let Some(bj) = buf.bboxes[j] else { continue };
            if !bi.overlaps(&bj) {
                continue;
            }
            for pos in bi.iter_cells() {
                if pos.x >= bj.min_x && pos.x <= bj.max_x && pos.y >= bj.min_y && pos.y <= bj.max_y
                {
                    bbox_overlap_count += 1;
                }
            }
        }
    }
    for (i, &net_opt) in buf.nets.iter().enumerate() {
        let hole = buf.holes[i];
        if hole.2 == u32::MAX {
            continue;
        }
        if let Some(net) = net_opt {
            buf.net_buckets[net.0].push(i);
        }
    }
    let mut mst_sum = 0.0f64;
    for bucket in &buf.net_buckets {
        if bucket.len() < 2 {
            continue;
        }
        mst_sum += mst_wire_length_fast(bucket, &buf.holes);
    }
    for (i, &net_opt) in buf.nets.iter().enumerate() {
        let (_, _, rail_id) = buf.holes[i];
        if rail_id == u32::MAX {
            continue;
        }
        buf.rail_map[rail_id as usize].push(net_opt);
    }
    let mut col_conflict_pairs = 0usize;
    for rail_owners in buf.rail_map.iter() {
        if rail_owners.len() < 2 {
            continue;
        }
        let base = rail_owners[0];
        for owner in &rail_owners[1..] {
            if *owner != base {
                col_conflict_pairs += 1;
            }
        }
    }
    for bbox_opt in buf.bboxes.iter() {
        let Some(bbox) = bbox_opt else { continue };
        if bbox.min_x < 0
            || bbox.max_x >= cols_i
            || bbox.min_y < 0
            || bbox.min_y >= board.main_rows() as i32
            || board.is_blocked(bbox.min_y as usize)
        {
            continue;
        }
        let rail_top = board
            .rail_rows(bbox.min_y)
            .first()
            .copied()
            .unwrap_or(bbox.min_y);
        buf.compact_map[rail_top as usize].push(*bbox);
    }
    let mut area_sum = 0.0f64;
    let mut row_squash_penalty = 0.0f64;
    let mut num_occupied_rails = 0usize;
    for cells in buf.compact_map.iter() {
        if cells.is_empty() {
            continue;
        }
        num_occupied_rails += 1;
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut row_seen: u32 = 0;
        for b in cells {
            min_x = min_x.min(b.min_x);
            max_x = max_x.max(b.max_x);
            min_y = min_y.min(b.min_y);
            max_y = max_y.max(b.max_y);
            let row_idx = (b.min_y - min_y) as u32;
            if row_idx < 32 {
                row_seen |= 1u32 << row_idx;
            }
        }
        if min_x <= max_x && min_y <= max_y {
            let width = (max_x - min_x + 1) as f64;
            area_sum += width;
        }
        let ny = row_seen.count_ones() as usize;
        let n = cells.len();
        if n > ny {
            row_squash_penalty += (n - ny) as f64;
        }
    }
    let rail_cross = if num_occupied_rails >= 2 {
        w.rail_crossing
    } else {
        0.0
    };

    let breakdown = CostBreakdown {
        mst: w.mst * mst_sum,
        pin_overlap: w.pin_overlap * coll_count as f64,
        bbox_overlap: w.b_box_overlap * bbox_overlap_count as f64,
        column_conflict: w.column_conflict * col_conflict_pairs as f64,
        out_of_bounds: w.out_of_bounds * oob_count as f64,
        compactness: w.compactness * area_sum,
        row_squash: w.row_squash * row_squash_penalty,
        rail_crossing: rail_cross,
        mst_sum,
        oob_count,
        col_conflict_pairs,
        coll_count,
        bbox_overlap_count,
        area_sum,
        row_squash_penalty,
    };
    let total = breakdown.mst
        + breakdown.pin_overlap
        + breakdown.bbox_overlap
        + breakdown.column_conflict
        + breakdown.out_of_bounds
        + breakdown.compactness
        + breakdown.row_squash
        + breakdown.rail_crossing;
    (total, breakdown)
}

/// 各成本项的调档值, 调试专用。
#[derive(Debug, Clone)]
pub(crate) struct CostBreakdown {
    pub mst: f64,
    pub pin_overlap: f64,
    pub bbox_overlap: f64,
    pub column_conflict: f64,
    pub out_of_bounds: f64,
    pub compactness: f64,
    pub row_squash: f64,
    pub rail_crossing: f64,
    pub mst_sum: f64,
    pub oob_count: u32,
    pub col_conflict_pairs: usize,
    pub coll_count: u32,
    pub bbox_overlap_count: u32,
    pub area_sum: f64,
    pub row_squash_penalty: f64,
}
