//! 接线: Wire (一段跳线) + Router trait + MST + hub-rail 谈判。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, NetId, PinId};

use super::Breadboard;
use super::breadboard::HoleId;
use super::occupancy::{Occupancy, Occupant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(pub(crate) usize);

impl WireId {
    pub fn raw(self) -> usize {
        self.0
    }
}

/// 一段面包板跳线。
///
/// 物理事实: 跳线就是一段线, 两个头插在两个孔里, 中间悬空。
/// 所以 `Wire` 只有 `from` 和 `to` 两个接触点, 没有中间点。
/// 走弯路时用**两根线**在一个公共孔上接续, 而不是给单根线加 waypoint。
#[derive(Debug, Clone)]
pub struct Wire {
    pub id: WireId,
    pub net: NetId,
    pub from: HoleId,
    pub to: HoleId,
}

impl Wire {
    /// Wire 接触的两个孔: `[from, to]`。
    pub fn contacts(&self) -> [HoleId; 2] {
        [self.from, self.to]
    }
}

/// 接线算法接口。给定一个 circuit + board + 当前占用, 返回一组 wire 满足所有 net。
///
/// 走线原则:
/// - Wire 只有 `from` 和 `to` 两个接触点, 绝不共享端点
/// - **同一列的孔已经由面包板内部连通**, 不用 wire 桥接
/// - Wire 只用来跨列连接 (例如把列 5 的某孔连到列 10 的某孔)
/// - 列内多点接到一个 net: 随便挑该列上的孔当 wire 端点, 面包板自动
///   把同列所有 pin 连在一起
///
/// `bridged_pins` 是 bridged 元件 (body 浮在板外) 的 pin-hole 对, 它们已经
/// 物理连好了 (腿到腿的导通就是元件本身), 不需要 wire。**进 net dedup,
/// 但不出 wire**。
pub trait Router {
    fn route(
        &self,
        circuit: &Circuit,
        board: &Breadboard,
        occupancy: &Occupancy,
        bridged_pins: &[(crate::circuit::PinId, HoleId)],
    ) -> Vec<Wire>;
}

// ============================================================
//  MST + Hub-Rail 谈判路由
// ============================================================
//
// 跨 net 冲突 (ColumnConflict) 在合法布局中不存在 — 每个 rail 只属于一个 net。
// 所以 PathFinder 经典的"跨 net 谈判"在这里是死代码。真正需要解决的问题是
// **同 net 内多条 MST 边共享 hub rail 时的孔位分配**。
//
// 算法 (单轮):
// 1. 预处理: 给每个 net 收集 rail 汇点 (按 rail_id dedup), 注入
//    bridged / power-rail-anchor 虚拟 pin, 检测内部跳线。
// 2. 对每个 net 独立跑 Kruskal MST, 用 best_wire 的理想 cost 排序选边结构。
// 3. 每边用 best_wire 选初始孔对 (不排除任何孔 — 可能多条线暂时共孔)。
// 4. Hub-rail 谈判:
//    a. 找出所有被 ≥2 根同 net wire 共用的 rail (hub)。
//    b. 对每个 hub rail:
//       - 每根线枚举该 rail 上的空孔, 选 (history + Manhattan) 最小的。
//       - 若有 ≥2 根线选同一孔 → 冲突 → history[孔] += increment。
//       - 重复直到无冲突 (或回退到枚举暴搜)。
//
// 跟原来跨-net 谈判的关键区别:
//   - 谈判的"冲突方"是同 net 的多根 wire, 不是不同 net。
//   - history 是 hub-rail 本地的 (每次从头初始化)。
//   - 无冲突 = 所有 wire 各占不同孔 = 返回合法结果。

/// 路由器配置。
pub struct PathFinderRouter {
    /// hub 谈判最多跑多少轮
    pub max_iterations: usize,
    /// 每次冲突孔 history 的增量
    pub history_increment: f64,
}

impl Default for PathFinderRouter {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            history_increment: 1.0,
        }
    }
}

impl Router for PathFinderRouter {
    fn route(
        &self,
        circuit: &Circuit,
        board: &Breadboard,
        occupancy: &Occupancy,
        bridged_pins: &[(crate::circuit::PinId, HoleId)],
    ) -> Vec<Wire> {
        // ── 预处理: PinId → HoleId ──
        let pin_hole: HashMap<PinId, HoleId> = board
            .holes()
            .iter()
            .filter_map(|h| match occupancy.occupant_at(h.id) {
                Some(Occupant::Pin(p)) => Some((p, h.id)),
                _ => None,
            })
            .collect();

        // ── 每个 net 的 rail 汇点 (按 rail_id dedup) ──
        let mut net_pins: Vec<Vec<(i32, i32, u32)>> = vec![Vec::new(); circuit.nets().len()];
        for net in circuit.nets() {
            let mut pins: Vec<(i32, i32, u32)> = net
                .pins
                .iter()
                .filter_map(|p| {
                    let h = pin_hole.get(p)?;
                    let pos = board.hole(*h).position;
                    let rail_id = board.rail_id_of(*h);
                    Some((pos.x, pos.y, rail_id))
                })
                .collect();
            pins.sort_by_key(|&(x, y, r)| (r, x, y));
            pins.dedup_by_key(|&mut (_, _, r)| r);
            net_pins[net.id.0] = pins;
        }

        // ── 注入 bridged 元件 ──
        for &(pin_id, hole_id) in bridged_pins {
            if let Some(pin) = circuit.pins().get(pin_id.0)
                && let Some(net) = pin.net
                && (net.0) < net_pins.len()
            {
                let pos = board.hole(hole_id).position;
                let rail_id = board.rail_id_of(hole_id);
                net_pins[net.0].push((pos.x, pos.y, rail_id));
                net_pins[net.0].sort_by_key(|&(x, y, r)| (r, x, y));
                net_pins[net.0].dedup_by_key(|&mut (_, _, r)| r);
            }
        }

        // ── 注入 power rail anchor ──
        if let Some(binding) = board.power_rail_binding() {
            for (polarity, net_id) in [
                (crate::layout::Polarity::Negative, binding.negative),
                (crate::layout::Polarity::Positive, binding.positive),
            ] {
                if (net_id.0) < net_pins.len()
                    && let Some(anchor) = board.power_rail_anchor(polarity)
                {
                    let pos = board.hole(anchor).position;
                    let rail_id = board.rail_id_of(anchor);
                    net_pins[net_id.0].push((pos.x, pos.y, rail_id));
                    net_pins[net_id.0].sort_by_key(|&(x, y, r)| (r, x, y));
                    net_pins[net_id.0].dedup_by_key(|&mut (_, _, r)| r);
                }
            }
        }

        // ── 检测元件内部跳线 ──
        let mut internal_jumpers: Vec<Vec<(usize, usize)>> = vec![Vec::new(); circuit.nets().len()];
        {
            let mut comp_pin_rails: HashMap<(crate::circuit::ComponentId, String), Vec<u32>> =
                HashMap::new();
            for net in circuit.nets() {
                comp_pin_rails.clear();
                for pin_id in &net.pins {
                    let pin = &circuit.pins()[pin_id.0];
                    let Some(&hole) = pin_hole.get(pin_id) else {
                        continue;
                    };
                    let rail = board.rail_id_of(hole);
                    comp_pin_rails
                        .entry((pin.component(), pin.num().to_string()))
                        .or_default()
                        .push(rail);
                }
                for ((_cid, _pin_num), rails) in &comp_pin_rails {
                    if rails.len() < 2 {
                        continue;
                    }
                    let unique: HashSet<u32> = rails.iter().copied().collect();
                    if unique.len() < 2 {
                        continue;
                    }
                    let ur: Vec<u32> = unique.into_iter().collect();
                    for i in 0..ur.len() {
                        for j in (i + 1)..ur.len() {
                            if let (Some(ri), Some(rj)) = (
                                net_pins[net.id.0].iter().position(|&(_, _, r)| r == ur[i]),
                                net_pins[net.id.0].iter().position(|&(_, _, r)| r == ur[j]),
                            ) {
                                if ri != rj {
                                    internal_jumpers[net.id.0].push((ri, rj));
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── 单轮路由: 每个 net 独立 MST + hub 谈判 ──
        let mut all_wires = Vec::new();
        let mut next_id: usize = 0;

        for net in circuit.nets() {
            let pins = &net_pins[net.id.0];
            if pins.len() < 2 {
                continue;
            }

            let edge_pairs = build_mst(pins, board, occupancy, &internal_jumpers[net.id.0]);

            let wire_holes = assign_and_negotiate(
                &edge_pairs,
                pins,
                board,
                occupancy,
                self.max_iterations,
                self.history_increment,
            );

            for (ha, hb) in wire_holes {
                all_wires.push(Wire {
                    id: WireId(next_id),
                    net: net.id,
                    from: ha,
                    to: hb,
                });
                next_id += 1;
            }
        }

        all_wires
    }
}

// ============================================================
//  MST 构建
// ============================================================

/// Kruskal MST with capacity constraint: 每条边的端点 rail 度数不能超过其空孔数。
///
/// 先跑受限 Kruskal (skip 会超容的边), 再跑一次不限容的补漏。
fn build_mst(
    pins: &[(i32, i32, u32)],
    board: &Breadboard,
    occupancy: &Occupancy,
    internal_pairs: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let n = pins.len();
    if n < 2 {
        return Vec::new();
    }

    // 每个 rail 的容量 = 空孔数
    let capacity: Vec<usize> = pins
        .iter()
        .map(|&(_, _, rail_id)| empty_holes_in_rail(rail_id, board, occupancy).len())
        .collect();

    let zero_history = vec![0.0; board.len()];

    // 1. 候选边: O(n²), 按 ideal cost 排序
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            if let Some((cost, _, _)) = best_wire(pins[i], pins[j], board, occupancy, &zero_history)
            {
                edges.push((i, j, cost));
            }
        }
    }
    edges.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    // 2. Union-Find, 预合并内部跳线
    let mut parent: Vec<usize> = (0..n).collect();
    for &(i, j) in internal_pairs {
        let ri = find(&mut parent, i);
        let rj = find(&mut parent, j);
        if ri != rj {
            parent[ri] = rj;
        }
    }

    // 3. 第一轮 Kruskal: 只加不超容的边
    let mut degree: Vec<usize> = vec![0; n];
    let mut edge_pairs: Vec<(usize, usize)> = Vec::new();
    let mut used: HashSet<(usize, usize)> = HashSet::new();

    for (i, j, _) in &edges {
        let ri = find(&mut parent, *i);
        let rj = find(&mut parent, *j);
        if ri == rj {
            continue;
        }
        // 容量检查: 加了这条边后, 两个端点的度数都不能超
        if degree[*i] >= capacity[*i] || degree[*j] >= capacity[*j] {
            continue;
        }
        parent[ri] = rj;
        degree[*i] += 1;
        degree[*j] += 1;
        edge_pairs.push((*i, *j));
        used.insert((*i, *j));
    }

    // 4. 第二轮: 不限容补漏 (确保连通)
    for (i, j, _) in &edges {
        let ri = find(&mut parent, *i);
        let rj = find(&mut parent, *j);
        if ri == rj {
            continue;
        }
        if used.contains(&(*i, *j)) {
            continue;
        }
        parent[ri] = rj;
        edge_pairs.push((*i, *j));
    }

    edge_pairs
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

// ============================================================
//  Hub-rail 谈判: 同 net 内 wire 孔分配
// ============================================================

/// 给定 MST 边结构 + 汇点信息, 先做初始孔分配, 然后对 hub rail 上的冲突进行谈判。
/// 如果有 hub rail 空孔不够 (线比孔多), 就拉 relay 线到旁边的空列来扩展容量。
fn assign_and_negotiate(
    edge_pairs: &[(usize, usize)],
    pins: &[(i32, i32, u32)],
    board: &Breadboard,
    occupancy: &Occupancy,
    max_iters: usize,
    increment: f64,
) -> Vec<(HoleId, HoleId)> {
    let k = edge_pairs.len();
    let zero_history = vec![0.0; board.len()];

    // 初始分配: 每条边用 best_wire 选最优孔对 (不排除 used_holes)
    let mut wires: Vec<(HoleId, HoleId)> = Vec::with_capacity(k);
    for &(i, j) in edge_pairs {
        if let Some((_, ha, hb)) = best_wire(pins[i], pins[j], board, occupancy, &zero_history) {
            wires.push((ha, hb));
        }
    }

    if k < 2 {
        return wires;
    }

    // 谈判 + 扩展循环
    let mut history: Vec<f64> = vec![0.0; board.len()];
    let mut used_relay_cols: HashSet<i32> = HashSet::new();
    let mut expansion_count = 0;
    const MAX_EXPANSIONS: usize = 20;

    loop {
        // 1. 谈判所有 hub rail
        for _ in 0..max_iters {
            let hub_rails = find_hub_rails(&wires, board);
            if hub_rails.is_empty() {
                break;
            }
            let mut any_progress = false;
            for (rail_id, wire_indices) in &hub_rails {
                if negotiate_rail(
                    *rail_id,
                    wire_indices,
                    &mut wires,
                    edge_pairs.len(),
                    board,
                    occupancy,
                    &mut history,
                    increment,
                ) {
                    any_progress = true;
                }
            }
            if !any_progress {
                break;
            }
        }

        // 2. 检查是否有拥塞 (线多于空孔的 main rail)
        let congested = find_congested_main_rail(&wires, board, occupancy);
        if congested.is_none() || expansion_count >= MAX_EXPANSIONS {
            break;
        }

        // 3. 扩展: 拉 relay 线到空列, 分流到空列上
        let (rail_id, wire_indices) = congested.unwrap();
        if !expand_congested_rail(
            rail_id,
            &wire_indices,
            &mut wires,
            &mut used_relay_cols,
            board,
            occupancy,
            &history,
        ) {
            // 无法扩展 (没空列了) — 放弃, 接受现状
            break;
        }
        expansion_count += 1;

        // 扩展后需要把新的 relay 线也加到 edge_pairs 的对应里?
        // 不需要 — negotiate_rail 现在从 wire 端点直接拿 target rail/col/row,
        // 不再依赖 edge_pairs。
    }

    wires
}

/// 找出所有被 ≥2 根 wire 用作端点的 rail (hub rail)。
fn find_hub_rails(wires: &[(HoleId, HoleId)], board: &Breadboard) -> HashMap<u32, Vec<usize>> {
    let mut rail_wires: HashMap<u32, Vec<usize>> = HashMap::new();
    for (wi, &(ha, hb)) in wires.iter().enumerate() {
        rail_wires.entry(board.rail_id_of(ha)).or_default().push(wi);
        rail_wires.entry(board.rail_id_of(hb)).or_default().push(wi);
    }
    rail_wires.retain(|_, v| v.len() >= 2);
    rail_wires
}

/// 对一根 wire 在 hub rail 上的上下文: 哪个 wire, 哪端是 hub, target 节点是谁。
struct WireCtx {
    wire_idx: usize,
    target_rail: u32,
    target_col: i32,
    target_row: i32,
    is_from_hub: bool,
}

/// 对一个 hub rail 上的 k 根 wire 进行孔位谈判。
///
/// 返回 true 表示 wires 被修改了。
fn negotiate_rail(
    rail_id: u32,
    wire_indices: &[usize],
    wires: &mut [(HoleId, HoleId)],
    edge_pairs_len: usize,
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &mut [f64],
    increment: f64,
) -> bool {
    let holes = empty_holes_in_rail(rail_id, board, occupancy);

    // 区分原生线和 relay 线 (relay 下标 >= edge_pairs_len, 不参与谈判)
    let mut relay_holes: HashSet<HoleId> = HashSet::new();
    let mut native_indices: Vec<usize> = Vec::new();
    for &wi in wire_indices {
        if wi >= edge_pairs_len {
            let (ha, hb) = wires[wi];
            if board.rail_id_of(ha) == rail_id {
                relay_holes.insert(ha);
            } else {
                relay_holes.insert(hb);
            }
        } else {
            native_indices.push(wi);
        }
    }

    let k = native_indices.len();
    if k < 2 || holes.len() < k + relay_holes.len() {
        return false;
    }

    // 对每根 wire, 确定哪个端点在 hub rail 上、target 的 rail 信息
    let mut ctxs: Vec<WireCtx> = Vec::with_capacity(k);
    for &wi in &native_indices {
        let (ha, hb) = wires[wi];
        if board.rail_id_of(ha) == rail_id {
            let tother = board.hole(hb);
            ctxs.push(WireCtx {
                wire_idx: wi,
                target_rail: board.rail_id_of(hb),
                target_col: tother.position.x,
                target_row: tother.position.y,
                is_from_hub: true,
            });
        } else {
            let tother = board.hole(ha);
            ctxs.push(WireCtx {
                wire_idx: wi,
                target_rail: board.rail_id_of(ha),
                target_col: tother.position.x,
                target_row: tother.position.y,
                is_from_hub: false,
            });
        }
    }

    // 谈判迭代
    const MAX_HUB_ITERS: usize = 40;
    for _ in 0..MAX_HUB_ITERS {
        let mut best_hole_idx: Vec<Option<usize>> = vec![None; k];
        let mut best_pair: Vec<Option<(HoleId, HoleId)>> = vec![None; k];
        let mut usage: HashMap<usize, usize> = HashMap::new();

        // 每根线选自己的最优 hub 孔 (跳过被 relay 占的孔)
        for (ci, ctx) in ctxs.iter().enumerate() {
            let target_pin = (ctx.target_col, ctx.target_row, ctx.target_rail);
            let mut best: Option<(f64, usize, HoleId, HoleId)> = None;

            for (hi, &hub_hole) in holes.iter().enumerate() {
                if relay_holes.contains(&hub_hole) {
                    continue;
                }
                if let Some((cost, target_hole)) =
                    best_target_given_hub(hub_hole, target_pin, board, occupancy, history)
                {
                    let total = cost;
                    if best.is_none_or(|(c, _, _, _)| total < c) {
                        best = Some((total, hi, hub_hole, target_hole));
                    }
                }
            }

            if let Some((_, hi, hub_hole, target_hole)) = best {
                best_hole_idx[ci] = Some(hi);
                if ctx.is_from_hub {
                    best_pair[ci] = Some((hub_hole, target_hole));
                } else {
                    best_pair[ci] = Some((target_hole, hub_hole));
                }
                *usage.entry(hi).or_insert(0) += 1;
            }
        }

        // 检查是否有冲突
        let conflicts: Vec<usize> = usage
            .iter()
            .filter(|&(_, &c)| c >= 2)
            .map(|(&hi, _)| hi)
            .collect();

        if conflicts.is_empty() {
            // 无冲突 — 应用结果
            for (ci, pair) in best_pair.iter().enumerate() {
                if let Some(p) = pair {
                    wires[ctxs[ci].wire_idx] = *p;
                }
            }
            return true;
        }

        // 惩罚冲突孔
        for &hi in &conflicts {
            history[holes[hi].0] += increment;
        }
    }

    // 谈判未能收敛 — 暴搜最优分配 (跳过 relay 占的孔)
    fallback_optimal_assignment(&ctxs, &holes, &relay_holes, board, occupancy, wires);
    true
}

/// 暴搜: 枚举 C(m, k) × k! 种孔分配, 选总 cost 最低的。
/// `relay_holes` 是被 relay 线占的孔, 枚举时排除。
fn fallback_optimal_assignment(
    ctxs: &[WireCtx],
    holes: &[HoleId],
    relay_holes: &HashSet<HoleId>,
    board: &Breadboard,
    occupancy: &Occupancy,
    wires: &mut [(HoleId, HoleId)],
) {
    let k = ctxs.len();

    // 过滤掉 relay 占的孔, 只留下可用的
    let avail: Vec<(usize, HoleId)> = holes
        .iter()
        .enumerate()
        .filter(|(_, h)| !relay_holes.contains(h))
        .map(|(i, &h)| (i, h))
        .collect();
    let ma = avail.len();
    if ma < k {
        return; // 孔不够, 放弃
    }

    // 预计算: cost[wire_i][avail_j] = min total wire cost
    let zero_history = vec![0.0; board.len()];
    let mut cost: Vec<Vec<Option<(f64, HoleId)>>> = Vec::with_capacity(k);
    for ci in 0..k {
        let target_pin = (
            ctxs[ci].target_col,
            ctxs[ci].target_row,
            ctxs[ci].target_rail,
        );
        let mut row = Vec::with_capacity(ma);
        for &(_, hub_hole) in &avail {
            row.push(best_target_given_hub(
                hub_hole,
                target_pin,
                board,
                occupancy,
                &zero_history,
            ));
        }
        cost.push(row);
    }

    // 枚举: 选 k 个不同的 avail index, 排列分配给 k 根线
    let mut best_total = f64::INFINITY;
    let mut best_assign: Vec<usize> = vec![0; k];

    let mut comb = vec![0; k];
    for i in 0..k {
        comb[i] = i;
    }

    loop {
        let mut perm: Vec<usize> = (0..k).collect();
        loop {
            let total: f64 = (0..k)
                .map(|wi| {
                    cost[wi][comb[perm[wi]]]
                        .map(|(c, _)| c)
                        .unwrap_or(f64::INFINITY)
                })
                .sum();

            if total < best_total {
                best_total = total;
                best_assign.copy_from_slice(&perm);
            }

            if !next_permutation(&mut perm) {
                break;
            }
        }

        if !next_combination(&mut comb, ma) {
            break;
        }
    }

    // 应用最佳分配
    for (wi, &pi) in best_assign.iter().enumerate() {
        if let Some((_, target_hole)) = cost[wi][comb[pi]] {
            let hub_hole = avail[comb[pi]].1;
            if ctxs[wi].is_from_hub {
                wires[ctxs[wi].wire_idx] = (hub_hole, target_hole);
            } else {
                wires[ctxs[wi].wire_idx] = (target_hole, hub_hole);
            }
        }
    }
}

/// 下一组合 (lexicographic order)
fn next_combination(comb: &mut [usize], n: usize) -> bool {
    let k = comb.len();
    let mut i = k;
    while i > 0 {
        i -= 1;
        if comb[i] < n - k + i {
            comb[i] += 1;
            for j in (i + 1)..k {
                comb[j] = comb[j - 1] + 1;
            }
            return true;
        }
    }
    false
}

/// 下一排列
fn next_permutation(p: &mut [usize]) -> bool {
    let n = p.len();
    let mut i = n.wrapping_sub(2);
    loop {
        if p[i] < p[i + 1] {
            break;
        }
        if i == 0 {
            return false;
        }
        i -= 1;
    }
    let mut j = n - 1;
    while p[j] <= p[i] {
        j -= 1;
    }
    p.swap(i, j);
    p[i + 1..].reverse();
    true
}

// ============================================================
//  Hub 容量扩展: 当 rail 空孔不够时拉 relay 线到空列
// ============================================================

/// 找到一个"线比孔多"的 main rail (非 power rail)。
/// 返回 (rail_id, 用到该 rail 的所有 wire 下标)。
fn find_congested_main_rail(
    wires: &[(HoleId, HoleId)],
    board: &Breadboard,
    occupancy: &Occupancy,
) -> Option<(u32, Vec<usize>)> {
    let mut rail_wires: HashMap<u32, Vec<usize>> = HashMap::new();
    for (wi, &(ha, hb)) in wires.iter().enumerate() {
        rail_wires.entry(board.rail_id_of(ha)).or_default().push(wi);
        rail_wires.entry(board.rail_id_of(hb)).or_default().push(wi);
    }

    for (&rail_id, indices) in &rail_wires {
        if indices.len() < 2 {
            continue;
        }
        // 只处理 main rail
        let hole = board.holes().iter().find(|h| h.rail_id == rail_id)?;
        if !matches!(hole.region, super::Region::MainRail) {
            continue;
        }
        let holes = empty_holes_in_rail(rail_id, board, occupancy);
        if indices.len() > holes.len() {
            return Some((rail_id, indices.clone()));
        }
    }
    None
}

/// 对一个拥塞的 hub rail 做容量扩展: 找旁边最近的全空列, 拉一根 relay 线过去,
/// 然后把部分 wire 从 hub 改连到空列。
///
/// 返回 true 表示扩展成功; false 表示找不到可用空列 (放弃).
fn expand_congested_rail(
    rail_id: u32,
    wire_indices: &[usize],
    wires: &mut Vec<(HoleId, HoleId)>,
    used_relay_cols: &mut HashSet<i32>,
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
) -> bool {
    let holes = empty_holes_in_rail(rail_id, board, occupancy);
    let k = wire_indices.len();
    let m = holes.len();
    if k <= m {
        return false; // 不拥塞
    }

    // 拿到 hub rail 的代表性位置
    let anchor = board
        .holes()
        .iter()
        .find(|h| h.rail_id == rail_id)
        .expect("congested rail must exist");
    let hub_col = anchor.position.x;
    let hub_row = anchor.position.y;

    // 确定 hub 所在的行区间
    let (row_lo, row_hi) = section_rows_for_y(hub_row, board);

    // 找最近的全空列 (排除已用过的 relay 列)
    let Some(empty_col) =
        find_nearest_empty_column(hub_col, row_lo, row_hi, board, occupancy, used_relay_cols)
    else {
        return false; // 没空列了
    };
    used_relay_cols.insert(empty_col);

    // 空列的 rail_id
    let Some(empty_hole) = board.at(empty_col, row_lo) else {
        return false;
    };
    let empty_rail_id = board.rail_id_of(empty_hole);

    // 拉 relay 线: hub ↔ 空列
    let hub_hole = holes[0];
    let relay_hole = pick_best_hole_in_rail(empty_rail_id, hub_hole, board, occupancy, history);
    wires.push((hub_hole, relay_hole));

    // 需要从 hub 迁走多少根线: 全部迁走, hub 只留 relay 线
    // (这样 hub 只有 1 根线啦, 肯定不拥塞; 所有原 hub 线都改连到空列)
    let to_move = wire_indices.len();

    // 对每根连到 hub 的线, 算它的 target 列, 挑离空列最近的那几根迁过去
    struct MoveCand {
        wire_idx: usize,
        target_col: i32,
    }
    let mut cands: Vec<MoveCand> = Vec::new();
    for &wi in wire_indices {
        let (ha, hb) = wires[wi];
        let target_rail = if board.rail_id_of(ha) == rail_id {
            board.rail_id_of(hb)
        } else {
            board.rail_id_of(ha)
        };
        let target_col = board
            .holes()
            .iter()
            .find(|h| h.rail_id == target_rail)
            .map(|h| h.position.x)
            .unwrap_or(hub_col);
        cands.push(MoveCand {
            wire_idx: wi,
            target_col,
        });
    }
    cands.sort_by_key(|c| (c.target_col - empty_col).abs());

    for i in 0..to_move.min(cands.len()) {
        let wi = cands[i].wire_idx;
        let (ha, hb) = wires[wi];
        let is_from_hub = board.rail_id_of(ha) == rail_id;
        let other = if is_from_hub { hb } else { ha };
        let new_hole = pick_best_hole_in_rail(empty_rail_id, other, board, occupancy, history);
        if is_from_hub {
            wires[wi] = (new_hole, hb);
        } else {
            wires[wi] = (ha, new_hole);
        }
    }

    true
}

/// 确定 y 所在 section 的行区间 (closed interval)。
fn section_rows_for_y(y: i32, board: &Breadboard) -> (i32, i32) {
    for blocked in board.blocked_rows() {
        let b = blocked as i32;
        if y < b {
            // section 在 blocked 上面
            let lo =
                if let Some(&prev) = board.blocked_rows().iter().rev().find(|&&r| (r as i32) < y) {
                    (prev + 1) as i32
                } else {
                    0
                };
            return (lo, b - 1);
        }
    }
    // y 在最下面的 section
    let lo = board
        .blocked_rows()
        .iter()
        .max()
        .map(|&r| (r + 1) as i32)
        .unwrap_or(0);
    (lo, board.rows() as i32 - 1)
}

/// 在 [row_lo, row_hi] 范围内找离 col 最近的全空列 (该列所有孔都为 None),
/// 且不在 `used_relay_cols` 中。
fn find_nearest_empty_column(
    col: i32,
    row_lo: i32,
    row_hi: i32,
    board: &Breadboard,
    occupancy: &Occupancy,
    used_relay_cols: &HashSet<i32>,
) -> Option<i32> {
    let max_col = board.cols() as i32;
    for dist in 0..max_col {
        for &dir in &[-1, 1] {
            let c = col + dir * dist;
            if c < 0 || c >= max_col || used_relay_cols.contains(&c) {
                continue;
            }
            if (row_lo..=row_hi).all(|r| board.at(c, r).is_some_and(|h| occupancy.can_place_pin(h)))
            {
                return Some(c);
            }
        }
    }
    None
}

/// 在 rail 的空孔中选离 other_hole Manhattan 最近的那个。
fn pick_best_hole_in_rail(
    rail_id: u32,
    other_hole: HoleId,
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
) -> HoleId {
    let holes = empty_holes_in_rail(rail_id, board, occupancy);
    let pos_o = board.hole(other_hole).position;
    let mut best = holes[0];
    let mut best_cost = f64::INFINITY;
    for &h in &holes {
        let pos = board.hole(h).position;
        let dist = (pos.x - pos_o.x).unsigned_abs() + (pos.y - pos_o.y).unsigned_abs();
        let cost = dist as f64 + history[h.0];
        if cost < best_cost {
            best_cost = cost;
            best = h;
        }
    }
    best
}

// ============================================================
//  孔选择原语
// ============================================================

/// 给定两个 (col, row, rail_id) 汇点, 找一对空孔 (ha, hb) 让 wire cost 最小。
///
/// wire cost = Manhattan(ha, hb) + history[ha] + history[hb]
/// **关键约束**: ha 必须在 `pin_a` 所在 rail 内, hb 在 `pin_b` 所在 rail 内 —
/// 否则面包板内部连通接不到 pin, 整个 net 实际是断的。
fn best_wire(
    pin_a: (i32, i32, u32),
    pin_b: (i32, i32, u32),
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
) -> Option<(f64, HoleId, HoleId)> {
    let holes_a = empty_holes_in_rail(pin_a.2, board, occupancy);
    let holes_b = empty_holes_in_rail(pin_b.2, board, occupancy);
    if holes_a.is_empty() || holes_b.is_empty() {
        return None;
    }

    let mut best: Option<(f64, HoleId, HoleId)> = None;
    for &ha in &holes_a {
        let pos_a = board.hole(ha).position;
        for &hb in &holes_b {
            let pos_b = board.hole(hb).position;
            let dist = (pos_a.x - pos_b.x).unsigned_abs() + (pos_a.y - pos_b.y).unsigned_abs();
            let cost = dist as f64 + history[ha.0] + history[hb.0];
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, ha, hb));
            }
        }
    }
    best
}

/// 固定 hub 端孔, 找 target rail 里的最优孔。
fn best_target_given_hub(
    hub_hole: HoleId,
    target: (i32, i32, u32),
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
) -> Option<(f64, HoleId)> {
    let holes = empty_holes_in_rail(target.2, board, occupancy);
    if holes.is_empty() {
        return None;
    }

    let pos_hub = board.hole(hub_hole).position;
    let mut best: Option<(f64, HoleId)> = None;
    for &ht in &holes {
        let pos_t = board.hole(ht).position;
        let dist = (pos_hub.x - pos_t.x).unsigned_abs() + (pos_hub.y - pos_t.y).unsigned_abs();
        let cost = dist as f64 + history[hub_hole.0] + history[ht.0];
        if best.is_none_or(|(c, _)| cost < c) {
            best = Some((cost, ht));
        }
    }
    best
}

/// 一个短路集合 (vertical rail 或 power rail) 的所有空孔。
fn empty_holes_in_rail(rail_id: u32, board: &Breadboard, occupancy: &Occupancy) -> Vec<HoleId> {
    let Some(anchor) = board.holes().iter().find(|h| h.rail_id == rail_id) else {
        return Vec::new();
    };
    board
        .connected_to(anchor.id)
        .into_iter()
        .filter(|&id| occupancy.can_place_pin(id))
        .collect()
}

// ============================================================
//  测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::Layout;
    use crate::layout::breadboard::PowerRailBinding;
    use crate::layout::placement::{Placement, Rotation};

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    /// 1 个 2-pin footprint: pin1@(0,0) pin2@(1,0)
    fn res2() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "RES2".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 1, y: 0 },
                },
            ],
        }
    }

    fn linear_circuit(n: usize) -> Circuit {
        let mut components = Vec::new();
        let mut pins = Vec::new();
        for i in 0..n {
            let p1 = PinId(pins.len());
            pins.push(Pin {
                id: p1,
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(0)),
            });
            let p2 = PinId(pins.len());
            pins.push(Pin {
                id: p2,
                component: ComponentId(i),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(0)),
            });
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("R{i}"),
                kind: "R".into(),
                value: None,
                pins: vec![p1, p2],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            });
        }
        let nets = vec![Net {
            id: NetId(0),
            name: "N".into(),
            pins: pins.iter().map(|p| p.id).collect(),
        }];
        Circuit {
            components,
            pins,
            nets,
            footprints: vec![res2()],
        }
    }

    fn place_linear(layout: &mut Layout, n: usize) {
        for i in 0..n {
            layout.place(
                ComponentId(i),
                Placement::OnBoard {
                    position: Position { x: i as i32, y: 1 },
                    rotation: Rotation::R90,
                },
            );
        }
    }

    #[test]
    fn two_components_one_net() {
        let b = board();
        let circuit = linear_circuit(2);
        let mut layout = Layout::new(&circuit);
        place_linear(&mut layout, 2);
        let occ = layout.occupancy(&b).unwrap();
        let wires = PathFinderRouter::default().route(&circuit, &b, &occ, &[]);
        assert_eq!(wires.len(), 1, "2 columns → 1 wire");
        assert_eq!(wires[0].net, NetId(0));
    }

    #[test]
    fn three_components_one_net() {
        let b = board();
        let circuit = linear_circuit(3);
        let mut layout = Layout::new(&circuit);
        place_linear(&mut layout, 3);
        let occ = layout.occupancy(&b).unwrap();
        let wires = PathFinderRouter::default().route(&circuit, &b, &occ, &[]);
        assert_eq!(wires.len(), 2, "3 columns → 2 wires (tree)");
    }

    #[test]
    fn all_pins_same_column() {
        let b = board();
        let circuit = Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "R0".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            }],
            pins: vec![Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(0)),
            }],
            nets: vec![Net {
                id: NetId(0),
                name: "N".into(),
                pins: vec![PinId(0)],
            }],
            footprints: vec![res2()],
        };
        let mut layout = Layout::new(&circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        let wires = PathFinderRouter::default().route(&circuit, &b, &occ, &[]);
        assert_eq!(wires.len(), 0, "all pins in 1 column → 0 wires");
    }

    #[test]
    fn two_nets_negotiate() {
        let b = Breadboard::new(30, 10);
        let mut components = Vec::new();
        let mut pins = Vec::new();
        let net_for = |i: usize| if i < 3 { NetId(0) } else { NetId(1) };
        for i in 0..5 {
            let p1 = PinId(pins.len());
            pins.push(Pin {
                id: p1,
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(net_for(i)),
            });
            let p2 = PinId(pins.len());
            pins.push(Pin {
                id: p2,
                component: ComponentId(i),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(net_for(i)),
            });
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("R{i}"),
                kind: "R".into(),
                value: None,
                pins: vec![p1, p2],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            });
        }
        let mut net_pins: [Vec<PinId>; 2] = [Vec::new(), Vec::new()];
        for p in &pins {
            let n = p.net.unwrap().0;
            net_pins[n].push(p.id);
        }
        let nets = vec![
            Net {
                id: NetId(0),
                name: "N0".into(),
                pins: net_pins[0].clone(),
            },
            Net {
                id: NetId(1),
                name: "N1".into(),
                pins: net_pins[1].clone(),
            },
        ];
        let circuit = Circuit {
            components,
            pins,
            nets,
            footprints: vec![res2()],
        };

        let mut layout = Layout::new(&circuit);
        let positions = [(0, 1), (3, 1), (5, 1), (2, 1), (6, 4)];
        for (i, &(x, y)) in positions.iter().enumerate() {
            layout.place(
                ComponentId(i),
                Placement::OnBoard {
                    position: Position { x, y },
                    rotation: Rotation::R90,
                },
            );
        }
        let occ = layout.occupancy(&b).unwrap();

        let wires = PathFinderRouter::default().route(&circuit, &b, &occ, &[]);
        assert_eq!(wires.len(), 3, "got wires: {wires:?}");

        let mut endpoints: Vec<HoleId> = wires.iter().flat_map(|w| w.contacts()).collect();
        endpoints.sort_by_key(|h| h.0);
        let unique: std::collections::HashSet<_> = endpoints.iter().collect();
        assert_eq!(unique.len(), 6, "no shared endpoints, got {endpoints:?}");

        for w in &wires {
            layout.add_wire(w.clone());
        }
        layout
            .occupancy(&b)
            .expect("layout with routed wires must be valid");
    }

    #[test]
    fn router_avoids_blocked_holes() {
        let b = board();
        let axial = Footprint {
            id: FootprintId(0),
            name: "axial".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 3, y: 0 },
                },
            ],
        };
        let make_pin = |id: usize, comp: usize, num: &str, net: NetId, idx: usize| Pin {
            id: PinId(id),
            component: ComponentId(comp),
            num: num.into(),
            pinfunction: None,
            physical_pin_index: idx,
            net: Some(net),
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "R1".into(),
                    kind: "R".into(),
                    value: None,
                    pins: vec![PinId(0), PinId(1)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R2".into(),
                    kind: "R".into(),
                    value: None,
                    pins: vec![PinId(2)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
            ],
            pins: vec![
                make_pin(0, 0, "1", NetId(0), 0),
                make_pin(1, 0, "2", NetId(0), 1),
                make_pin(2, 1, "1", NetId(0), 0),
            ],
            nets: vec![Net {
                id: NetId(0),
                name: "N".into(),
                pins: vec![PinId(0), PinId(1), PinId(2)],
            }],
            footprints: vec![axial],
        }));
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &b, &occ, &[]);
        for w in &wires {
            for h in w.contacts() {
                let pos = b.hole(h).position;
                for blocked_pos in [(6, 2), (7, 2), (11, 2), (12, 2)] {
                    assert_ne!(
                        (pos.x, pos.y),
                        blocked_pos,
                        "wire 端点落在 Blocked 孔 {}",
                        blocked_pos.0
                    );
                }
            }
        }
        assert!(!wires.is_empty(), "应能走出一根线");
    }

    #[test]
    fn router_connects_across_rails() {
        let b = Breadboard::standard();
        let fp = Footprint {
            id: FootprintId(0),
            name: "single".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "A".into(),
                    kind: "X".into(),
                    value: None,
                    pins: vec![PinId(0)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
                Component {
                    id: ComponentId(1),
                    ref_: "B".into(),
                    kind: "X".into(),
                    value: None,
                    pins: vec![PinId(1)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: Some(NetId(0)),
                },
            ],
            nets: vec![Net {
                id: NetId(0),
                name: "N".into(),
                pins: vec![PinId(0), PinId(1)],
            }],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 0, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 5, y: 10 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &b, &occ, &[]);
        assert_eq!(wires.len(), 1, "跨 rail 跨 col 同 net → 1 根 wire");
        let w = &wires[0];
        let p1 = b.hole(w.from).position;
        let p2 = b.hole(w.to).position;
        assert_eq!(p1.x, 0);
        assert_eq!(p2.x, 5);
        assert!(p1.y < 5, "p1 应该在 上 rail, got {p1:?}");
        assert!(p2.y >= 7, "p2 应该在 下 rail (y=7..12), got {p2:?}");
    }

    #[test]
    fn router_with_binding_runs_wire_to_rail() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "G".into(),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(0)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            }],
            pins: vec![Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(0)),
            }],
            nets: vec![Net {
                id: NetId(0),
                name: "GND".into(),
                pins: vec![PinId(0)],
            }],
            footprints: vec![fp],
        }));

        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 0 },
                rotation: Rotation::R0,
            },
        );
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(999),
            negative: NetId(0),
        });
        let occ = layout.occupancy(&board).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &board, &occ, &[]);

        assert_eq!(
            wires.len(),
            1,
            "绑定 GND → 负极后, 路由必须生成 1 根 wire 连到 rail"
        );
        let w = &wires[0];
        let p1 = board.hole(w.from).position;
        let p2 = board.hole(w.to).position;
        let rail_pos = if p1.y < 0 || p1.y >= 12 {
            p1
        } else if p2.y < 0 || p2.y >= 12 {
            p2
        } else {
            panic!("wire 端点应该有一个在 power rail, 实际 p1={p1:?} p2={p2:?}");
        };
        let main_pos = if p1.x == 5 && p1.y >= 0 && p1.y < 5 {
            p1
        } else if p2.x == 5 && p2.y >= 0 && p2.y < 5 {
            p2
        } else {
            panic!("wire 端点应该有一个在 col 5 的 main rail (y 0..5), 实际 p1={p1:?} p2={p2:?}");
        };
        let dx = (rail_pos.x - main_pos.x).abs();
        let dy = (rail_pos.y - main_pos.y).abs();
        assert!(
            dx + dy <= 8,
            "jumper 长度应该合理, dx={dx} dy={dy}, main={main_pos:?} rail={rail_pos:?}"
        );
    }

    #[test]
    fn router_no_binding_no_rail_wire() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "G".into(),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(0)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            }],
            pins: vec![Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(0)),
            }],
            nets: vec![Net {
                id: NetId(0),
                name: "GND".into(),
                pins: vec![PinId(0)],
            }],
            footprints: vec![fp],
        }));

        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 0 },
                rotation: Rotation::R0,
            },
        );
        let board = Breadboard::standard();
        let occ = layout.occupancy(&board).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &board, &occ, &[]);
        assert_eq!(wires.len(), 0, "不绑定时, 1 个 pin 的 net 不用 wire (0 根)");
    }
}
