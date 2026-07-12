//! Debug 辅助函数: spectral_debug_positions / diagnose_expensive_seeds /
//! inspect_state_pins / print_seed_cost_report。

use crate::circuit::{Circuit, ComponentId};
use crate::layout::breadboard::{Breadboard, HoleId, Polarity};

#[allow(clippy::too_many_arguments)]
pub(crate) fn diagnose_expensive_seeds(
    states: &[crate::layout::cost::SAState],
    costs: &[f64],
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, HoleId)],
    _weights: &crate::layout::cost::Weights,
    base_seed: u64,
) {
    if states.is_empty() {
        return;
    }

    // 算 median。复制排序后取中间。
    let mut sorted: Vec<f64> = costs.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) * 0.5
    };
    // 阈值: 10倍 median 或绝对 50万 (远大于正常成本), 赇上任一即触发。
    let threshold = (median * 10.0).max(500_000.0);

    let mut found = false;
    for (seed_idx, (&c, state)) in costs.iter().zip(states.iter()).enumerate() {
        if c >= threshold {
            if !found {
                eprintln!();
                eprintln!(
                    "--- [诊断] cost 远超 threshold={:.0} 的 seed 详情 ---",
                    threshold
                );
                found = true;
            }
            eprintln!(
                "\n[seed s{} -> 实际 seed = 0x{:08X}] cost = {:.3}",
                seed_idx,
                base_seed.wrapping_add(seed_idx as u64),
                c
            );
            // 分解成本: 看是哪一项贡婕的。weight 为 默认值的特例:
            // out_of_bounds / column_conflict 都是 1e6 / pin (或冲突对),
            // 其他项 通常 ≤ 几千, 所以高 cost 通常出在这两项。
            let breakdown = crate::layout::cost::cost_breakdown(
                state,
                circuit,
                board,
                bridged_pins,
                &crate::layout::cost::Weights::default(),
            );
            eprintln!(
                "  分解:\n    mst           = {:>10.2}  (sum={:.2})\n    pin_overlap   = {:>10.2}  (count={})\n    bbox_overlap  = {:>10.2}  (cells={})\n    column_conf.  = {:>10.2}  (pairs={})\n    out_of_bounds = {:>10.2}  (oob={})\n    compactness   = {:>10.2}  (area={:.2})\n    row_squash    = {:>10.2}  (penalty={:.2})\n    rail_crossing = {:>10.2}\n    total         = {:>10.2}",
                breakdown.1.mst,
                breakdown.1.mst_sum,
                breakdown.1.pin_overlap,
                breakdown.1.coll_count,
                breakdown.1.bbox_overlap,
                breakdown.1.bbox_overlap_count,
                breakdown.1.column_conflict,
                breakdown.1.col_conflict_pairs,
                breakdown.1.out_of_bounds,
                breakdown.1.oob_count,
                breakdown.1.compactness,
                breakdown.1.area_sum,
                breakdown.1.row_squash,
                breakdown.1.row_squash_penalty,
                breakdown.1.rail_crossing,
                breakdown.0,
            );
            inspect_state_pins(state, circuit, board, bridged_pins, _weights);
        }
    }
    if found {
        eprintln!();
    }
}

/// 打印一个 SAState 里所有 pin 的 (x, y), 标出 OOB / 同 rail 不同 net 的
/// (column_conflict) pin。便于诊断为什么某个 seed 会卡死。
#[allow(clippy::too_many_arguments)]
fn inspect_state_pins(
    state: &crate::layout::cost::SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, HoleId)],
    _weights: &crate::layout::cost::Weights,
) {
    use crate::circuit::NetId;
    use std::collections::BTreeMap;

    // 收集所有 pin 的 (col, row, rail_id, net, comp_id, pin_num), 区分 bridged
    // 还是 OnBoard. bridged 用 active_bridge_pair 的两 pin hole (actual world pos);
    // OnBoard 用 state.x + footprint rotated pin offset.
    struct PinInfo {
        comp_id: crate::circuit::ComponentId,
        pin_num: String,
        x: i32,
        y: i32,
        rail_id: u32,
        net: Option<NetId>,
    }
    let mut all_pins: Vec<PinInfo> = Vec::new();

    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        let comp = &circuit.components[comp_id.0];
        let Some(fid) = comp.footprint else {
            continue;
        };
        let footprint = &circuit.footprints[fid.0];

        if state.bridged[idx] {
            if let Some(pair) = state.active_bridge_pair(idx) {
                for &(hole_id, pin_id) in &pair {
                    let pos = board.hole(hole_id).position;
                    let rail_id = board.rail_id_of(hole_id);
                    let net = circuit.pins[pin_id.0].net;
                    all_pins.push(PinInfo {
                        comp_id,
                        pin_num: circuit.pins[pin_id.0].num.clone(),
                        x: pos.x,
                        y: pos.y,
                        rail_id,
                        net,
                    });
                }
            }
        } else {
            let px = state.x[idx];
            let py = state.y[idx];
            let is_r180 = state.rotation[idx] == crate::layout::placement::Rotation::R180;
            for &pin_id in &comp.pins {
                let pin = &circuit.pins[pin_id.0];
                let physical = footprint
                    .physical_pin_for(pin)
                    .unwrap_or_else(|| panic!("footprint pin 不存在 num={}", pin.num()));
                let offset = if is_r180 {
                    crate::layout::placement::rotate(
                        physical.offset,
                        crate::layout::placement::Rotation::R180,
                    )
                } else {
                    physical.offset
                };
                let abs_x = px + offset.x;
                let abs_y = py + offset.y;
                let rail_id = board
                    .at(abs_x, abs_y)
                    .map(|h| board.rail_id_of(h))
                    .unwrap_or(u32::MAX);
                all_pins.push(PinInfo {
                    comp_id,
                    pin_num: pin.num().to_string(),
                    x: abs_x,
                    y: abs_y,
                    rail_id,
                    net: pin.net,
                });
            }
        }
    }

    // 加上 手动 Bridged 的 pin (放在 Layout 里的, 从 Layout.bridged_pins() 推进)。
    for &(pin_id, hole_id) in bridged_pins {
        let pos = board.hole(hole_id).position;
        let rail_id = board.rail_id_of(hole_id);
        all_pins.push(PinInfo {
            comp_id: circuit.pins[pin_id.0].component,
            pin_num: circuit.pins[pin_id.0].num.clone(),
            x: pos.x,
            y: pos.y,
            rail_id,
            net: circuit.pins[pin_id.0].net,
        });
    }
    // Power rail 虚拟 pin (绑定 net 在 anchor 位置) — 跟 cost_fast 一样注入。
    if let Some(binding) = board.power_rail_binding() {
        for (polarity, net_id) in [
            (Polarity::Negative, binding.negative),
            (Polarity::Positive, binding.positive),
        ] {
            if let Some(anchor) = board.power_rail_anchor(polarity) {
                let pos = board.hole(anchor).position;
                let rail_id = board.rail_id_of(anchor);
                all_pins.push(PinInfo {
                    comp_id: ComponentId(0), // dummy; 虚拟 pin 由 pin_num 标识
                    pin_num: format!("<virtual anchor {:?}>", polarity),
                    x: pos.x,
                    y: pos.y,
                    rail_id,
                    net: Some(net_id),
                });
            }
        }
    }

    // 阅 OOB pin
    let oob: Vec<&PinInfo> = all_pins.iter().filter(|p| p.rail_id == u32::MAX).collect();
    if !oob.is_empty() {
        eprintln!("    OOB pin (rail_id = u32::MAX, 越界 / blocked row / power rail gap):");
        for p in &oob {
            let comp = &circuit.components[p.comp_id.0];
            eprintln!(
                "      {} pad {}  ({:>3}, {:>3})  net={:?}",
                comp.ref_(),
                p.pin_num,
                p.x,
                p.y,
                p.net.map(|n| circuit.nets[n.0].name.as_str())
            );
        }
    } else {
        eprintln!("    无 OOB pin — 高成本可能来自 column_conflict (同 rail 同列不同 net)");
    }

    // 查 column_conflict: 跟 cost_fast 代码严格对齐 — 同 rail_id 不同 net
    // (面包板同一 row 的所有 pin 短路, 任何不同 net 的 pin 都是冲突)。
    let mut by_rail: BTreeMap<u32, Vec<&PinInfo>> = BTreeMap::new();
    for p in &all_pins {
        if p.rail_id == u32::MAX {
            continue;
        }
        by_rail.entry(p.rail_id).or_default().push(p);
    }
    let mut conflicts_found = false;
    for (rail_id, pins_in_rail) in by_rail {
        // cost_fast: base = 第一个 pin 的 net, 后续 pin 与 base net 不同就算冲突。
        // 进 cost 里包含 None vs Some(N) 也会算冲突。
        if pins_in_rail.len() < 2 {
            continue;
        }
        let base_pin = pins_in_rail[0];
        let mut conflict_pins: Vec<&PinInfo> = Vec::new();
        for &p in &pins_in_rail[1..] {
            if p.net != base_pin.net {
                conflict_pins.push(p);
            }
        }
        if !conflict_pins.is_empty() {
            if !conflicts_found {
                eprintln!(
                    "    Column-conflict (同 rail_id 不同 net, base = {} pad {} net={:?}):",
                    circuit.components[base_pin.comp_id.0].ref_(),
                    base_pin.pin_num,
                    base_pin.net.map(|n| circuit.nets[n.0].name.as_str())
                );
                conflicts_found = true;
            }
            eprintln!("      rail_id = {rail_id} (同 rail 上 net 不同):");
            eprintln!(
                "        base:    {} pad {} (col {:>3}) net = {:?}",
                circuit.components[base_pin.comp_id.0].ref_(),
                base_pin.pin_num,
                base_pin.x,
                base_pin.net.map(|n| circuit.nets[n.0].name.as_str())
            );
            for p in &conflict_pins {
                let comp = &circuit.components[p.comp_id.0];
                eprintln!(
                    "        conflict: {} pad {} (col {:>3}) net = {:?}",
                    comp.ref_(),
                    p.pin_num,
                    p.x,
                    p.net.map(|n| circuit.nets[n.0].name.as_str())
                );
            }
        }
    }
}

/// 报告 30 个 seed 的 cost 分布, 帮调试看到 SA 收敛抖动。
/// 在主 binary 里 println! 一份, 其余调用路径 (测试 / 库) 静默。
pub(crate) fn print_seed_cost_report(costs: &[f64], best_cost: f64, base_seed: u64) {
    if costs.is_empty() {
        return;
    }

    // 排序拷贝: 不动原顺序, 只算统计。
    let mut sorted: Vec<f64> = costs.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let n = sorted.len();
    let min = sorted[0];
    let max = sorted[n - 1];
    let sum: f64 = sorted.iter().sum();
    let mean = sum / n as f64;
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) * 0.5
    };
    // 总体标准差 (population)。可能很小, 表示 SA 稳定。
    let variance: f64 = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    // “跨多少 seed 拿到人认 min”。容差用 1e-6 (邻接解）。
    let ties = sorted
        .iter()
        .filter(|&&c| (c - best_cost).abs() < 1e-6)
        .count();

    // ASCII 直方图: 10 个桶对 1 span。
    let span = (max - min).max(1e-9);
    let n_buckets = 10;
    let mut buckets = vec![0usize; n_buckets];
    for &c in &sorted {
        let mut idx = ((c - min) / span * n_buckets as f64) as usize;
        if idx >= n_buckets {
            idx = n_buckets - 1;
        }
        buckets[idx] += 1;
    }
    let max_count = *buckets.iter().max().unwrap();
    let bar_max = 30;

    eprintln!();
    eprintln!(
        "--- SA seed cost 分布 (base_seed = 0x{:08X}) ---",
        base_seed
    );
    eprintln!(
        "  n={n}  min={min:>9.3}  median={median:>9.3}  mean={mean:>9.3}  max={max:>9.3}  std={std_dev:>7.3}"
    );
    eprintln!("  best={best_cost:.3}  其中 {ties}/{n} seed 拿到 (容差 1e-6)");
    eprintln!();
    eprintln!("  cost           | 分布");
    eprintln!("  ---------------+------------------------------");
    for (i, &count) in buckets.iter().enumerate() {
        let lo = min + span * i as f64 / n_buckets as f64;
        let hi = min + span * (i + 1) as f64 / n_buckets as f64;
        let bar_len = max_count
            .checked_div(max_count)
            .map_or(0, |_| (count * bar_max) / max_count);
        let bar = "#".repeat(bar_len);
        eprintln!("  [{lo:>9.3}, {hi:>9.3}) | {bar:<bar_max$} ({count:>3})");
    }
    eprintln!();
    // 全量列出 (16 个以内, 30 也能看)。
    let display = if n > 32 { &sorted[..32] } else { &sorted[..] };
    let joined: Vec<String> = display.iter().map(|c| format!("{c:.2}")).collect();
    eprintln!("  排序后 (前 32 个): [{}]", joined.join(", "));
    if n > 32 {
        eprintln!("  ...还有 {} 个省略", n - 32);
    }
    eprintln!();
}
