//! 桥接探测 + 桥接位世界坐标计算。

use crate::circuit::{Circuit, Component, ComponentId, NetId, PinId, Position};
use crate::layout::breadboard::{Breadboard, HoleId, Polarity, Region};
use crate::layout::placement::{Rotation, rotate};

use super::Weights;
use super::context::{CostBuf, SAContext};
use super::cost_fast::cost_fast;
use super::state::SAState;

pub(crate) fn propose_bridged_pairs(
    comp: &Component,
    circuit: &Circuit,
    board: &Breadboard,
    power_net_ids: &[NetId],
) -> Vec<[(HoleId, PinId); 2]> {
    debug_assert_eq!(comp.pins.len(), 2, "bridgeable 必有 2 pin");

    // 1. 分 power / signal pin
    let Some(power_pin_id) = comp
        .pins
        .iter()
        .find(|&&pid| {
            circuit.pins[pid.0]
                .net
                .map(|n| power_net_ids.contains(&n))
                .unwrap_or(false)
        })
        .copied()
    else {
        return Vec::new();
    };
    let signal_pin_id = comp
        .pins
        .iter()
        .find(|&&pid| pid != power_pin_id)
        .copied()
        .expect("bridgeable 必有 2 pin (debug_assert 已守)");
    let power_net = circuit.pins[power_pin_id.0].net;

    // 2. 查 footprint pad offsets
    let Some(fp_id) = comp.footprint else {
        return Vec::new();
    };
    let fp = &circuit.footprints[fp_id.0];
    let power_off = fp
        .pins()
        .get(circuit.pins[power_pin_id.0].physical_pin_index)
        .map(|p| p.offset);
    let signal_off = fp
        .pins()
        .get(circuit.pins[signal_pin_id.0].physical_pin_index)
        .map(|p| p.offset);
    let (Some(power_off), Some(signal_off)) = (power_off, signal_off) else {
        return Vec::new();
    };

    let delta = Position {
        x: signal_off.x - power_off.x,
        y: signal_off.y - power_off.y,
    };

    // 3. 只扫那些 `matching rail` 的 power rail 孔：
    //    power pin 的 net 被绑到某个 power rail 极性后, 该 rail 上的孔都是合法位。
    //    极性不匹配的 rail (matching_rail_ids 之外的 power rail) 不扫 — 那样的 pair
    //    会让 power pin 落到错极性 rail, 生成 1e6 列冲突惩罚 (物理上不该走)。
    let matching_rail_ids = collect_matching_rail_ids(board, power_net);

    // 4. 单次扫描: 只 matching。matching 没产出合法 pair 则返空 (启发式退化为
    //    bridgeable 元件保持 OnBoard)。
    //
    //    历史背景: 之前会接着扫 “其他” rail (fallback) — 这个 fallback 让 power pin
    //    有机会落到错极性 rail, 从而跟虚拟 rail 锚点冲突, 启动 cost 就是 1e6。
    //    SA 从这种起点难走出来, 最后选定 1e6 解。去掉 fallback 后 cache 仅含
    //    极性对齐的 pair, 杜绝这个类死锁。
    let all_power_holes: Vec<HoleId> = (0..board.holes().len())
        .map(HoleId)
        .filter(|h| board.region_of(*h) == Region::PowerRail)
        .collect();
    let matching: Vec<HoleId> = all_power_holes
        .iter()
        .copied()
        .filter(|h| matching_rail_ids.contains(&board.rail_id_of(*h)))
        .collect();

    let mut out = Vec::new();
    for &h in matching.iter() {
        let h_pos = board.hole(h).position;
        for &rot in &[Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let rotated = rotate(delta, rot);
            let signal_pos = Position {
                x: h_pos.x + rotated.x,
                y: h_pos.y + rotated.y,
            };
            if let Some(signal_h) = board.at(signal_pos.x, signal_pos.y)
                && board.region_of(signal_h) == Region::MainRail
            {
                out.push([(h, power_pin_id), (signal_h, signal_pin_id)]);
            }
        }
    }
    out
}

/// 旧 API 兼容: 返回**第一个**合法桥接对 (用于单对场景, 如向后兼容测试)。
/// 新代码请用 `propose_bridged_pairs` + `populate_bridgeable_info`。
#[cfg(test)]
pub(crate) fn propose_bridged_pair(
    comp: &Component,
    circuit: &Circuit,
    board: &Breadboard,
    power_net_ids: &[NetId],
) -> Option<[(HoleId, PinId); 2]> {
    propose_bridged_pairs(comp, circuit, board, power_net_ids)
        .into_iter()
        .next()
}

/// 收集 rail_id 集合: 这些 power rail 被 bound 到 `pin_net`。
/// `pin_net == None` 返空集 (用户不绑 → 没有 "net 匹配" 的 rail, 启发式走 fallback 扫所有 rail)。
pub(super) fn collect_matching_rail_ids(
    board: &Breadboard,
    pin_net: Option<NetId>,
) -> std::collections::HashSet<u32> {
    let mut ids = std::collections::HashSet::new();
    let Some(pin_net) = pin_net else { return ids };
    let Some(binding) = board.power_rail_binding() else {
        return ids;
    };
    for (polarity, net_id) in [
        (Polarity::Negative, binding.negative),
        (Polarity::Positive, binding.positive),
    ] {
        if net_id != pin_net {
            continue;
        }
        if let Some(anchor) = board.power_rail_anchor(polarity) {
            ids.insert(board.rail_id_of(anchor));
        }
    }
    ids
}

/// 对 state 中所有 `Component.bridgeable = true` 的元件跑启发式, 填充
/// `is_bridgeable` / `bridged_pin_pairs` / `active_bridge_idx`。`bridged` 字段不动
/// (默认 false = OnBoard)。
///
/// 调用时机: `from_greedy` / `from_spectral` 构造完 state 之后,
/// SA 启动前 (在 `sa::simulate` 内部)。`Component.bridgeable = false` 的元件
/// `is_bridgeable` 恒为 false, `bridged_pin_pairs` 为空 Vec —— `Move::ToggleBridging`
/// 不会命中它们。
///
/// **排序**: 对每个 bridgeable 元件, 启发式返回所有合法 (hole, rotation) 对。
/// 这里按 "signal pin 离同 net (signal pin 所在的 net) 其他元件 pin 的几何中心
/// 最近" 排序, 索引 0 = 最佳候选。SA 在 `ToggleBridging` 翻到 bridge 模式时会
/// 遍历这个列表、按 cost 重选并写回 `active_bridge_idx[idx]`。
///
/// **静态中心**: 用 SA 启动**前** state 里其他元件的 pin 位置算中心 (从
/// `state.x/y/rotation` 推, bridged 元件用 `active_bridge_pair`)。
/// 中心一次性算好后就不再随 SA 更新 — cost 自然会把元件从拥挤
/// 位置赶走, 所以不需要动态重算中心。
/// 排序的 hint, 不会把真正最优的候选卡在后面 (因为候选列表只是初始偏置,
/// SA 会按 cost 重排)。
pub(crate) fn populate_bridgeable_info(
    state: &mut SAState,
    circuit: &Circuit,
    board: &Breadboard,
    power_net_ids: &[NetId],
) {
    let n = state.placeable.len();
    debug_assert_eq!(state.is_bridgeable.len(), n);
    debug_assert_eq!(state.bridged.len(), n);
    debug_assert_eq!(state.bridged_pin_pairs.len(), n);
    debug_assert_eq!(state.active_bridge_idx.len(), n);

    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        let comp = &circuit.components[comp_id.0];
        if !comp.bridgeable {
            continue;
        }
        let candidates = propose_bridged_pairs(comp, circuit, board, power_net_ids);
        if candidates.is_empty() {
            // 启发式返空: 该元件本轮无法桥接, is_bridgeable 保持 false,
            // Toggle 不会命中它 (随机退回其他 move, 不污染 seed 序列)。
            continue;
        }

        // 算 signal net 的几何中心 (用 state 当前 (x, y, rotation), bridged 元件
        // 用 active_bridge_pair)。只用于排序 hint, 精度不重要。
        let signal_net_id = comp
            .pins
            .iter()
            .map(|&pid| circuit.pins[pid.0].net)
            .find(|net_opt| net_opt.is_some() && !power_net_ids.contains(&net_opt.unwrap()))
            .flatten();
        let center = signal_net_id
            .and_then(|nid| compute_signal_net_center(circuit, board, state, nid, Some(comp_id)));

        // 按 "signal pin 离中心 Manhattan 距离" 排序, 距离小的优先。
        // Tiebreaker: 在同等距离下优选 top rail 的 power pin ("靠上")。
        //   标准板 top rail y ∈ {-3, -4} (负), main 0..11, bottom 14, 15。
        //   使用 `y < 0` 作为 top rail 的判别, 跨板高配置都能用
        //   (任何"负 y"代表上边, "正大 y"代表下边)。
        // 没有中心 (signal net 只此一个 pin) 时保持原顺序 (启发式扫的顺序)。
        if let Some(center) = center {
            let mut sorted: Vec<(i32, [(HoleId, PinId); 2])> = candidates
                .into_iter()
                .map(|pair| {
                    let signal_pos = board.hole(pair[1].0).position;
                    let dist = (signal_pos.x - center.x).abs() + (signal_pos.y - center.y).abs();
                    (dist, pair)
                })
                .collect();
            sorted.sort_by(|a, b| {
                let dist_cmp = a.0.cmp(&b.0);
                if dist_cmp != std::cmp::Ordering::Equal {
                    return dist_cmp;
                }
                // tiebreaker: power pin 在 top rail (负 y) 优先
                let a_top = board.hole(a.1[0].0).position.y < 0;
                let b_top = board.hole(b.1[0].0).position.y < 0;
                // a_top = true (b_top = false) → a 在前
                match (a_top, b_top) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                }
            });
            state.bridged_pin_pairs[idx] = sorted.into_iter().map(|(_, p)| p).collect();
        } else {
            state.bridged_pin_pairs[idx] = candidates;
        }
        state.is_bridgeable[idx] = true;
        state.active_bridge_idx[idx] = 0; // 启发式最佳 = 索引 0
    }
}

/// Init 阶段: 给每个 `is_bridgeable` 的元件默认走 bridged 模式, 取 cache 里
/// `cost_fast` 最低的那一对。
///
/// **设计动机**: SA 启动时 ToggleBridging 概率只 15%, 多数 seeds 跑完全部
/// iter 都没把 bridgeable 翻过去, 初始 OnBoard 起点拖住 SOTA 收敛。本函数
/// 在 init 阶段把 "翻成桥接" 提前, 起点直接是 "全 bridgeable 都在桥接"。
/// SA 的 ToggleBridging 仍然能翻回 OnBoard (如果那更优), 不锁死。
///
/// **不做 safety net**: 即使 cache 最优 cache pair 的 cost 仍高于 OnBoard
/// 起点也强制翻。理由: SA 能在温度高的时候翻回去, 与不翻、让 SA 自己探索
/// 相比, 主动翻有一个更好的局部起点用于其他 Move 微调。
///
/// **预算**: 每个 bridgeable K 次 cost 评估 (K ≈ cache 长度)。5 个 bridgeable
/// × 5 个候选 ≈ 25 次, 跟 30k iter × 几百 cost 评估相比可以忽略。
///
/// **调用时机**: `populate_bridgeable_info` 之后, SA 主循环首次 `cost_fast`
/// 之前。在 `sa::simulate` 里调用。
pub(crate) fn init_bridgeable_to_bridged(
    state: &mut SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, crate::layout::breadboard::HoleId)],
    weights: &Weights,
    ctx: &SAContext,
    buf: &mut CostBuf,
) {
    let n = state.placeable.len();
    for i in 0..n {
        if !state.is_bridgeable[i] || state.bridged_pin_pairs[i].is_empty() {
            continue;
        }
        let cache_len = state.bridged_pin_pairs[i].len();
        let mut best_cost = f64::INFINITY;
        let mut best_idx = 0;
        for j in 0..cache_len {
            state.active_bridge_idx[i] = j;
            let c = cost_fast(state, circuit, board, bridged_pins, weights, ctx, buf);
            if c < best_cost {
                best_cost = c;
                best_idx = j;
            }
        }
        state.active_bridge_idx[i] = best_idx;
        state.bridged[i] = true;
    }
}

/// 算一个 net 的几何中心 (各 pin 位置的平均)。排除 `exclude_comp` 的 pin
/// (避免启发式把自己要摆的 signal pin 也算进去, 造成 "候选间无差异" 的退化)。
/// 返回 None 当 net 上没有其他 pin (只有 bridgeable 自己一个)。
pub(super) fn compute_signal_net_center(
    circuit: &Circuit,
    board: &Breadboard,
    state: &SAState,
    net_id: NetId,
    exclude_comp: Option<ComponentId>,
) -> Option<Position> {
    let mut sum_x: i64 = 0;
    let mut sum_y: i64 = 0;
    let mut count: i64 = 0;
    for &pid in &circuit.nets[net_id.0].pins {
        let pin = &circuit.pins[pid.0];
        if exclude_comp.is_some_and(|c| c == pin.component) {
            continue;
        }
        // 找这个 component 在 state.placeable 里的 idx
        let Some(idx) = state.placeable.iter().position(|&c| c == pin.component) else {
            continue;
        };
        // 推 pin 的世界坐标
        let pos = pin_world_pos(state, idx, pin, circuit, board);
        sum_x += pos.x as i64;
        sum_y += pos.y as i64;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some(Position {
        x: (sum_x / count) as i32,
        y: (sum_y / count) as i32,
    })
}

/// 推 `state.placeable[idx]` 的指定 pin 当前世界坐标。处理 bridged / OnBoard 两种路径。
pub(super) fn pin_world_pos(
    state: &SAState,
    idx: usize,
    pin: &crate::circuit::Pin,
    circuit: &Circuit,
    board: &Breadboard,
) -> Position {
    if let Some(pair) = state.active_bridge_pair(idx) {
        // bridged: pair 里两条腿, 找 pin.num 跟哪条匹配
        for &(hole, pid) in &pair {
            if pid == pin.id {
                return board.hole(hole).position;
            }
        }
        // pin 不在该元件的桥接 pair 里 (例如该元件 3 pin 但 is_bridgeable=true 的罕见情况)
        // 退回 OnBoard 路径
    }
    let comp = &circuit.components[state.placeable[idx].0];
    let fp_id = comp.footprint.expect("placeable 元件必有 footprint");
    let fp = &circuit.footprints[fp_id.0];
    let physical = fp
        .physical_pin_for(pin)
        .expect("footprint 缺 pin (解析阶段就该爆)");
    let rotated = rotate(physical.offset, state.rotation[idx]);
    Position {
        x: state.x[idx] + rotated.x,
        y: state.y[idx] + rotated.y,
    }
}
