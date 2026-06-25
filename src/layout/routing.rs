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
pub trait Router {
    fn route(&self, circuit: &Circuit, board: &Breadboard, occupancy: &Occupancy) -> Vec<Wire>;
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
//    a. 对每个 net 算一个最小代价 spanning tree (Kruskal):
//       edge cost = 两端点 Manhattan 距离 + history[from] + history[to]
//       选代价最低的 (ha, hb) 端点对
//    b. 检查冲突: 2+ net 用了同一孔 → 该孔 history += history_increment
// 3. 收敛 (无冲突) 或达到 max_iter → 结束
//
// 历史代价单调递增, 跑足够多轮后, "对距离不敏感"的 net 会自动让出
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
    fn route(&self, circuit: &Circuit, board: &Breadboard, occupancy: &Occupancy) -> Vec<Wire> {
        // PinId → HoleId 反查 (从 occupancy 派生, 包含 pin 和已存在的 wire)
        let pin_hole: HashMap<PinId, HoleId> = board
            .holes()
            .iter()
            .filter_map(|h| match occupancy.occupant_at(h.id) {
                Some(Occupant::Pin(p)) => Some((p, h.id)),
                _ => None,
            })
            .collect();

        // 每个 net 的 unique pin columns
        let mut net_columns: Vec<Vec<i32>> = vec![Vec::new(); circuit.nets().len()];
        for net in circuit.nets() {
            let mut cols: Vec<i32> = net
                .pins
                .iter()
                .filter_map(|p| pin_hole.get(p).map(|h| board.hole(*h).position.x))
                .collect();
            cols.sort();
            cols.dedup();
            net_columns[net.id.0] = cols;
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
                &net_columns,
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
    net_columns: &[Vec<i32>],
    history: &[f64],
    next_id: &mut usize,
) -> (Vec<Wire>, usize) {
    let mut all_wires = Vec::new();
    let mut usage: HashMap<HoleId, usize> = HashMap::new();

    for net in circuit.nets() {
        let columns = &net_columns[net.id.0];
        if columns.len() < 2 {
            // 0 或 1 个 column: 不需要 wire (列内已连通, 或没东西)
            continue;
        }

        for (from, to) in mst_wires(columns, board, occupancy, history) {
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

/// 给一个 net 的 columns 算最小生成树 (Kruskal), 返回 tree 的边 (端点对)
///
/// 关键: Kruskal 每加一条边, 重算 best wire 时刻意排除本 net 已用的孔,
/// 避免 net 内部两根 wire 撞同一个孔 (尤其是度 >= 2 的 hub column)。
///
/// edge cost 排序是按"理想" cost (不排除已用孔) — 只用来选 MST 结构;
/// 最终的孔选择是加边时重算的, 会略高于理想 cost, 但合法。
fn mst_wires(
    columns: &[i32],
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
) -> Vec<(HoleId, HoleId)> {
    let n = columns.len();
    if n < 2 {
        return Vec::new();
    }

    // 1. 算所有 O(n^2) 条候选 edge 的"理想" cost, 只用来挑 MST 结构
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            if let Some((cost, _, _)) = best_wire(columns[i], columns[j], board, occupancy, history)
            {
                edges.push((i, j, cost));
            }
        }
    }
    edges.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    // 2. Kruskal 加边, 带上本 net 已用孔的约束
    let mut parent: Vec<usize> = (0..n).collect();
    let mut wires: Vec<(HoleId, HoleId)> = Vec::new();
    let mut used_holes: HashSet<HoleId> = HashSet::new();

    for (i, j, _) in edges {
        let ri = find(&mut parent, i);
        let rj = find(&mut parent, j);
        if ri == rj {
            continue;
        }
        // 重算 best wire, 排除本 net 已用孔
        if let Some((_, ha, hb)) = best_wire_avoiding(
            columns[i],
            columns[j],
            board,
            occupancy,
            history,
            &used_holes,
        ) {
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

/// 给定两列, 找一对空孔 (ha, hb) 让 wire cost 最小
///
/// wire cost = Manhattan(ha, hb) + history[ha] + history[hb]
fn best_wire(
    col_a: i32,
    col_b: i32,
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
) -> Option<(f64, HoleId, HoleId)> {
    best_wire_avoiding(col_a, col_b, board, occupancy, history, &HashSet::new())
}

/// 同 `best_wire`, 但排除 `used` 里的孔 (本 net 内部已占的孔)
fn best_wire_avoiding(
    col_a: i32,
    col_b: i32,
    board: &Breadboard,
    occupancy: &Occupancy,
    history: &[f64],
    used: &HashSet<HoleId>,
) -> Option<(f64, HoleId, HoleId)> {
    let holes_a = empty_holes_in_column(col_a, board, occupancy);
    let holes_b = empty_holes_in_column(col_b, board, occupancy);
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
            if best.map_or(true, |(c, _, _)| cost < c) {
                best = Some((cost, ha, hb));
            }
        }
    }
    best
}

/// 一列上所有空孔
fn empty_holes_in_column(col: i32, board: &Breadboard, occupancy: &Occupancy) -> Vec<HoleId> {
    (0..board.rows() as i32)
        .filter_map(|row| board.at(col, row))
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
                net: Some(NetId(0)),
            });
            let p2 = PinId(pins.len());
            pins.push(Pin {
                id: p2,
                component: ComponentId(i),
                num: "2".into(),
                pinfunction: None,
                net: Some(NetId(0)),
            });
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("R{i}"),
                kind: "R".into(),
                value: None,
                pins: vec![p1, p2],
                footprint: Some(FootprintId(0)),
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
                Placement {
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

        let wires = PathFinderRouter::default().route(&circuit, &b, &occ);
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

        let wires = PathFinderRouter::default().route(&circuit, &b, &occ);
        assert_eq!(wires.len(), 2, "3 columns → 2 wires (tree)");
    }

    /// 验证: 1 个 net 全部 pin 在同一列 → 0 根 wire (列内已连通)
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
            }],
            pins: vec![Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
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
            Placement {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();

        let wires = PathFinderRouter::default().route(&circuit, &b, &occ);
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
                net: Some(net_for(i)),
            });
            let p2 = PinId(pins.len());
            pins.push(Pin {
                id: p2,
                component: ComponentId(i),
                num: "2".into(),
                pinfunction: None,
                net: Some(net_for(i)),
            });
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("R{i}"),
                kind: "R".into(),
                value: None,
                pins: vec![p1, p2],
                footprint: Some(FootprintId(0)),
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
                Placement {
                    position: Position { x, y },
                    rotation: Rotation::R90,
                },
            );
        }
        let occ = layout.occupancy(&b).unwrap();

        let wires = PathFinderRouter::default().route(&circuit, &b, &occ);
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
}
