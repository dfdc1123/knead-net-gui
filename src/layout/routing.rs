//! 接线: Wire (一段跳线) + Router trait + PathFinder 实现。

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
//  PathFinder-style Negotiation Router
// ============================================================
//
// 思路: 把 PathFinder (FPGA 布线算法) 改到面包板上
//
// 模型:
// - 每个 net 是一组 pin, 落在一组 pin column 上
// - 一个 net 需要的 wire 数 = |unique pin columns| - 1, 构成 spanning tree
// - 每条 wire: 两个端点必须落在**空孔**上, 端点之间不共享孔
//
// 算法 (多轮迭代 + 谈判):
// 1. 初始化 history[h] = 0 for all hole h
// 2. 对每轮 iter < max_iterations:
//    a. 对每个 net 跑 Kruskal 算 spanning tree:
//       (i)   用"理想 cost" = Manhattan(ha,hb) + history[ha] + history[hb]
//             对所有 (ha, hb) 排序 (Kruskal 只用来挑边结构; ha,hb 是当前空孔对)
//       (ii)  加边时重算一次 best_wire, 故意排除本 net 已用孔,
//             让最终选出的 wire 端点**保证**不重复占用本 net 已用孔
//             (实际成本可能略高于理想 cost, 但合法)
//    b. 统计使用 ≥ 2 的孔 → history[h] += history_increment
// 3. 收敛 (无冲突) 或达到 max_iter → 结束; 若还未收敛则返回历史上冲突最少的方案
//
// 历史代价单调非减, 跑足够多轮后, "对距离不敏感"的 net 会自动让出
// 拥堵孔, 退到旁边的空列, 最终实现无短路布线。
//
// 跟当前 Occupancy 严格模型 (1 孔 ≤ 1 occupant) 兼容:
// 端点共享 = 冲突 = 加 history 惩罚, 逼着 net 换孔。

/// 简化版 PathFinder 路由器。
///
/// 调参建议:
/// - 板子小 / net 少: 默认参数 (`max_iterations = 50`) 足够
/// - 板子挤 / 拥堵: 加大 `max_iterations`, 适当调高 `history_increment` 让
///   让位过程更激进
/// - 始终不收敛: 返回 "历史最佳" (冲突最少) 方案, 由调用方决定怎么办
pub struct PathFinderRouter {
    /// 最多跑多少轮
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
        // PinId → HoleId 反查 (从 occupancy 派生, 包含 pin 和已存在的 wire)
        let pin_hole: HashMap<PinId, HoleId> = board
            .holes()
            .iter()
            .filter_map(|h| match occupancy.occupant_at(h.id) {
                Some(Occupant::Pin(p)) => Some((p, h.id)),
                _ => None,
            })
            .collect();

        // 每个 net 的 unique pin (col, row, rail_id) 汇点, 按 rail_id dedup。
        // 引入 power rail 后短路集合不再是"同列同 rail_top", 而是"同 rail_id"
        // (不管 vertical 还是 power)。同 rail 合并 = 面包板已内部连通, 不需 wire。
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

        // 注入 bridged 元件的 pin: 这些 pin 已经在物理上连好了 (腿到腿)。
        // 跟 OnBoard pin 一样进 net 的 pin 列表, 经过 dedup_by_key 后会跟同 rail
        // 的其他 pin 合并 (因为 rail 内部 shorted)。
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

        // 注入 power rail 虚拟 pin: 每个 bound rail 加一个 anchor 位置的 pin,
        // 挂在绑定的 net 上。这样路由会强制生成一根 wire 把主区 pin 连到 rail。
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
                // net_id 越界 (不在 circuit 里) 静默忽略
            }
        }

        // 检测元件内部跳线: 同 component 同 num 的 pin 电气短路 (SW 上下同名 pad)。
        // 为每个 net 记录 pre-connected 的 rail-index 对, 在 MST 中插零 cost 边。
        // **必须在 bridged / power rail 注入之后计算**, 否则注入会改变 net_pins
        // 索引顺序 (power rail anchor 的 rail_id=0 插入到最前面), 导致跳线索引
        // 指到错误的 rail。
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

        let mut history: Vec<f64> = vec![0.0; board.len()];
        let mut best_solution: Vec<Wire> = Vec::new();
        let mut best_conflicts = usize::MAX;
        let mut next_id: usize = 0;

        for _ in 0..self.max_iterations {
            let (wires, conflicts) = route_iteration(
                circuit,
                board,
                occupancy,
                &net_pins,
                &internal_jumpers,
                &history,
                &mut next_id,
            );

            if conflicts == 0 {
                return rewire_ids(wires);
            }

            if conflicts < best_conflicts {
                best_conflicts = conflicts;
                best_solution = wires.clone();
            }

            update_history(&mut history, &wires, self.history_increment);
        }

        rewire_ids(best_solution)
    }
}

/// 跑一轮: 给每个 net 算 spanning tree, 返回 (所有 wire, 冲突孔数)
fn route_iteration(
    circuit: &Circuit,
    board: &Breadboard,
    occupancy: &Occupancy,
    net_pins: &[Vec<(i32, i32, u32)>],
    internal_jumpers: &[Vec<(usize, usize)>],
    history: &[f64],
    next_id: &mut usize,
) -> (Vec<Wire>, usize) {
    let mut all_wires = Vec::new();
    let mut usage: HashMap<HoleId, usize> = HashMap::new();

    for net in circuit.nets() {
        let pins = &net_pins[net.id.0];
        if pins.len() < 2 {
            // 0 或 1 个 rail 汇点: 不需要 wire (同 rail 内已连通, 或没东西)
            continue;
        }

        for (from, to) in mst_wires(pins, board, occupancy, history, &internal_jumpers[net.id.0]) {
            *usage.entry(from).or_insert(0) += 1;
            *usage.entry(to).or_insert(0) += 1;
            all_wires.push(Wire {
                id: WireId(*next_id),
                net: net.id,
                from,
                to,
            });
            *next_id += 1;
        }
    }

    // 冲突 = usage 计数 ≥ 2 的孔数
    let conflicts = usage.values().filter(|&&c| c >= 2).count();
    (all_wires, conflicts)
}

/// 给一个 net 的 (col, row, rail_id) 汇点算最小生成树 (Kruskal), 返回 tree 的边 (端点对)
///
/// 关键: Kruskal 每加一条边, 重算 best wire 时刻意排除本 net 已用的孔,
/// 避免 net 内部两根 wire 撞同一个孔 (尤其是度 >= 2 的 hub column)。
///
/// edge cost 排序是按"理想" cost (不排除已用孔) — 只用来选 MST 结构;
/// 最终的孔选择是加边时重算的, 会略高于理想 cost, 但合法。
fn mst_wires(
    pins: &[(i32, i32, u32)],
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
    internal_pairs: &[(usize, usize)],
) -> Vec<(HoleId, HoleId)> {
    let n = pins.len();
    if n < 2 {
        return Vec::new();
    }

    // 1. 算所有 O(n^2) 条候选 edge 的"理想" cost, 只用来挑 MST 结构
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            if let Some((cost, _, _)) = best_wire(pins[i], pins[j], board, occupancy, history) {
                edges.push((i, j, cost));
            }
        }
    }
    edges.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    // 3. 预合并内部跳线: 同元件同名 pin 在 MST 中视为已连通, 不产生 wire
    let mut parent: Vec<usize> = (0..n).collect();
    for &(i, j) in internal_pairs {
        let ri = find(&mut parent, i);
        let rj = find(&mut parent, j);
        if ri != rj {
            parent[ri] = rj;
        }
    }

    // 4. Kruskal 加边
    let mut wires: Vec<(HoleId, HoleId)> = Vec::new();
    let mut used_holes: HashSet<HoleId> = HashSet::new();

    for (i, j, _) in edges {
        let ri = find(&mut parent, i);
        let rj = find(&mut parent, j);
        if ri == rj {
            continue;
        }
        if let Some((_, ha, hb)) =
            best_wire_avoiding(pins[i], pins[j], board, occupancy, history, &used_holes)
        {
            parent[ri] = rj;
            used_holes.insert(ha);
            used_holes.insert(hb);
            wires.push((ha, hb));
        }
    }

    wires
}

/// Union-Find find, 带路径压缩
fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// 给定两个 (col, row, rail_id) 汇点, 找一对空孔 (ha, hb) 让 wire cost 最小
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
    best_wire_avoiding(pin_a, pin_b, board, occupancy, history, &HashSet::new())
}

/// 同 `best_wire`, 但排除 `used` 里的孔 (本 net 内部已占的孔)
fn best_wire_avoiding(
    pin_a: (i32, i32, u32),
    pin_b: (i32, i32, u32),
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
    used: &HashSet<HoleId>,
) -> Option<(f64, HoleId, HoleId)> {
    let holes_a = empty_holes_in_rail(pin_a.2, board, occupancy);
    let holes_b = empty_holes_in_rail(pin_b.2, board, occupancy);
    if holes_a.is_empty() || holes_b.is_empty() {
        return None;
    }

    let mut best: Option<(f64, HoleId, HoleId)> = None;
    for &ha in &holes_a {
        if used.contains(&ha) {
            continue;
        }
        let pos_a = board.hole(ha).position;
        for &hb in &holes_b {
            if used.contains(&hb) {
                continue;
            }
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

/// 一个短路集合 (vertical rail 或 power rail 行) 的所有空孔。
/// `board.connected_to` 拿到所有内部短接的孔, 然后过滤掉已被占用 / 已用过的。
fn empty_holes_in_rail(rail_id: u32, board: &Breadboard, occupancy: &Occupancy) -> Vec<HoleId> {
    // 找到 rail_id 对应的任一 HoleId, 用 connected_to 拿整个 rail
    // power rail 的 rail_id 唯一确定一行 (因为 top + bottom 同极性合并)
    // main rail 的 rail_id 唯一确定 (col, vertical_rail), connected_to 返回 5 个孔
    let Some(anchor) = board.holes().iter().find(|h| h.rail_id == rail_id) else {
        return Vec::new();
    };
    board
        .connected_to(anchor.id)
        .into_iter()
        .filter(|&id| occupancy.can_place_pin(id))
        .collect()
}

/// 给所有"被 2+ net 用了"的孔 +1 history
fn update_history(history: &mut [f64], wires: &[Wire], increment: f64) {
    let mut usage: HashMap<HoleId, usize> = HashMap::new();
    for w in wires {
        *usage.entry(w.from).or_insert(0) += 1;
        *usage.entry(w.to).or_insert(0) += 1;
    }
    for (hole, count) in usage {
        if count >= 2 {
            history[hole.0] += increment;
        }
    }
}

/// 把 WireId 重新从 0 开始编, 避免迭代过程产生空洞
fn rewire_ids(wires: Vec<Wire>) -> Vec<Wire> {
    wires
        .into_iter()
        .enumerate()
        .map(|(i, mut w)| {
            w.id = WireId(i);
            w
        })
        .collect()
}

// ============================================================
//  tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
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

    /// 构造一个 circuit: N 个 component (每个 2 pin), 1 个 net 把所有 pin 串起来
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

    /// 把 R_i 放在 (i, 2) R90 方向
    /// R90 后 res2 footprint 的 2 个 pin 都在同一列: pin1 @ (i, 2), pin2 @ (i, 3)
    /// 所以 N 个 component 占 N 列
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

    use crate::layout::Layout;

    /// 验证: 1 个 net 在 2 个 column → 1 根 wire
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

    /// 验证: 1 个 net 在 3 个 column → 2 根 wire (spanning tree)
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

    /// 验证: net 只有 1 个 pin → 0 根 wire (一条短线在 spanning tree 里退化成 0 边)。
    /// (此测试原意是 "all pins same column"; 改用 1 pin 的 net 简化, 故 doc 跟上更新。)
    #[test]
    fn all_pins_same_column() {
        let b = board();
        // 1 个 component 2 个 pin, footprint 第 1, 2 pin 都落在 col 5
        // (TO92 在 (5, 2) R0: pin1→(5,2), pin2→(6,2), 不在同一列, 改用 1 pin 的 net)
        //
        // 改: 1 个 component 1 个 pin, 1 个 net 把它自己连起来 → columns 数量 = 1 → 0 wire
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

    /// 验证: 2 个 net, 板子宽, 谈判后能分到不冲突的位置
    #[test]
    fn two_nets_negotiate() {
        // 30 × 10 板, 5 个 component, 2 个 net
        // 全部 R90 (pin 在同一列, 每 component 占 1 列)
        //
        //   col:  0  1  2  3  4  5  6  7  8  9 ...
        //   R0        R0                    (net 0, col 0)
        //   R1                 R1           (net 0, col 3)
        //   R2                       R2     (net 0, col 5)
        //   R3        R3                    (net 1, col 2)
        //   R4                          R4  (net 1, col 6)
        //
        // net 0 跨 col 0, 3, 5 → 2 wires
        // net 1 跨 col 2, 6   → 1 wire
        // 原本 col 5 被 2 个 net 都需要, 后改 R4 到 col 6 避免列短路
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
        // 收集每个 net 的 pin
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
        // 3 个 net 0 component 在 (0,1), (3,1), (5,1) R90
        // 2 个 net 1 component 在 (2,1), (6,4) R90
        // R90: pin1 @ (x, y), pin2 @ (x, y+1)
        // (原本 R4 在 (5,4) 跟 R2 同列不同 row, 被 col 5 rail 短路, 改 6 隔开)
        let positions = [
            (0, 1), // R0
            (3, 1), // R1
            (5, 1), // R2
            (2, 1), // R3
            (6, 4), // R4
        ];
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
        // net 0: 3 cols → 2 wires; net 1: 2 cols → 1 wire; total 3
        assert_eq!(wires.len(), 3, "got wires: {wires:?}");

        // 3 根 wire 的 6 个端点必须互不相同 (0 conflict)
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

    /// 验证: routing 不会把 wire 端点掉进 Blocked 单元 (被元件本体占据的格子)。
    /// 用一个跨 4 col 的 axial footprint (pin at 0 和 3, 中间 (1,0)(2,0) 是本体)。
    /// 另一个元件摆在 (10, 2) 与第一元件 pin 远端同列, 要求连到同一 net; 避免的
    /// Blocked 是 R1 的 (6,2)(7,2) 和 R2 的 (11,2)(12,2)。
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
        // R1 摆在 (5, 2): bbox (5..=8, 2..=2), pin 在 (5,2)(8,2). blocked: (6,2)(7,2).
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // R2 摆在 (10, 2) R0: bbox (10..=13, 2..=2), pin 在 (10,2)(13,2). blocked: (11,2)(12,2).
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &b, &occ, &[]);
        // wire 端点 必须在空孔上, 不能落在 (6,2)(7,2)(11,2)(12,2) 这些 Blocked 孔上
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
        // 至少走出一根 wire (三个 pin 跨 3 个 column)
        assert!(!wires.is_empty(), "应能走出一根线");
    }

    /// 验证: 在标准板 (30×12) 上, pin 分别落在**上下 rail 且跨了不同 col**
    /// (上 rail (0,2), 下 rail (5,10))。跨 rail 不能靠面包板内部连通, 跨 col
    /// 不在同一 rail, 路由器必须走 1 根 wire 把它们连起来。
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
        // A 在 (0, 2) 上 rail, B 在 (5, 10) 下 rail — 跨 rail 又跨 col
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
        // 端点应分别在 (0, 2) 和 (5, 10) 附近 (具体 y 不定, 但必须不落在 blocked row)
        assert_eq!(p1.x, 0);
        assert_eq!(p2.x, 5);
        assert!(p1.y < 5, "p1 应该在 上 rail, got {p1:?}");
        assert!(p2.y >= 7, "p2 应该在 下 rail (y=7..12), got {p2:?}");
    }

    // ============================================================
    //  PowerRailBinding 路由测试
    // ============================================================

    /// 绑定 GND → 负极: 1 个 GND pin 在主区 (5, 0)。路由器必须生成 1 根 wire
    /// 把这个 pin 连到负极轨 (y=-4 或 y=14, 同 rail_id)。
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
            // positive 用越界 NetId, 路由会跳过 (该 net 不存在)
            positive: NetId(999),
            negative: NetId(0),
        });
        let occ = layout.occupancy(&board).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &board, &occ, &[]);

        // 应该有 1 根 wire (pin → rail)
        assert_eq!(
            wires.len(),
            1,
            "绑定 GND → 负极后, 路由必须生成 1 根 wire 连到 rail"
        );
        let w = &wires[0];
        // 端点 1: 某个 power rail (y=-4 或 y=14, 同 rail_id)
        // 端点 2: 某个 col 5 的 main rail 孔 (y in 0..5)
        // 注意: wire 端点不能在 (5, 0) (被 component pin 占), 所以同 rail 的
        // 其他孔 (y in 1..5) 都行 — rail 短接, 电气上等效。
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
        // jumper 长度: |Δcol| + |Δrow| (col 5 跟 rail col 0..28 里的某个 x 都可以)
        let dx = (rail_pos.x - main_pos.x).abs();
        let dy = (rail_pos.y - main_pos.y).abs();
        assert!(
            dx + dy <= 8,
            "jumper 长度应该合理, dx={dx} dy={dy}, main={main_pos:?} rail={rail_pos:?}"
        );
    }

    /// 不绑定时, GND net 1 个 pin 不用 wire (没东西要连)
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
        let board = Breadboard::standard(); // 不绑定
        let occ = layout.occupancy(&board).unwrap();
        let wires = PathFinderRouter::default().route(circuit, &board, &occ, &[]);
        assert_eq!(wires.len(), 0, "不绑定时, 1 个 pin 的 net 不用 wire (0 根)");
    }
}
