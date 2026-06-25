//! 模拟退火用的成本函数: HPWL + pin 碰撞 + 越界 + 列冲突。
//!
//! 设计要点:
//! - **HPWL_x = 一个 net 的 (max_col - min_col)**, 在"同列零成本"模型下正好等于
//!   走线 MST 长度, 所以不用算真正的 MST。
//! - 成本是各项**加权和**, 权在 [`Weights`] 里调。
//! - `SAState` 是 SA 内部状态, 只在 layout 子模块内共享; v2 起每个元件显式持有
//!   `(x, y, rotation)`, 不再由 order 推 x。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;
use crate::layout::placement::{Rotation, rotate};

/// SA 成本函数的四项权重。
///
/// 成本 = `hpwl * HPWL + pin_overlap * 碰撞数 + column_conflict * 列短路对数 + out_of_bounds * 越界 pin 数`
///
/// 默认值见 [`Weights::default`], 经验起点; 真用时按板子拥挤程度调。
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub hpwl: f64,
    pub pin_overlap: f64,
    /// 同列不同 net 的 pin 对数 (N 个 pin 同列冲突就是 N-1 + N-2 + ... + 1 = N(N-1)/2)
    pub column_conflict: f64,
    pub out_of_bounds: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            // 一根 5 孔 wire 省下 ~5 成本
            hpwl: 1.0,
            // 一次 pin 碰撞 = 让 SA 宁愿多绕 50-100 孔也不撞
            pin_overlap: 100.0,
            // 同列不同 net 的 pin 会被面包板竖向 rail 短接, 这是物理电气短路,
            // 不能让走线"治愈"。惩罚拉到 out_of_bounds 同级, 让 SA 当作硬约束。
            column_conflict: 1_000_000.0,
            // 越界基本不允许; 巨大惩罚让 SA 直接拒绝
            out_of_bounds: 1_000_000.0,
        }
    }
}

/// SA 内部状态: 每个元件显式持有 `(x, y, rotation)`。
///
/// v2 起 (显式 2D 布局), x 不再由 order 推导, SA 可以把元件放到板子任意位置。
/// order 还在, 但只用于标识; `placeable[i]` 的 i 索引对应 `x[i] / y[i] / rotation[i]`。
#[derive(Debug, Clone)]
pub struct SAState {
    pub placeable: Vec<ComponentId>,
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub rotation: Vec<Rotation>,
}

impl SAState {
    pub fn n(&self) -> usize {
        self.placeable.len()
    }

    /// 简单构造: 给定元件顺序, 全部 R0, 全部同一行, x 按顺序累加 (gap=1)。
    /// 主要给测试用——真实初始状态用 [`SAState::from_greedy`]。
    pub fn from_order(order: Vec<ComponentId>, row: i32, widths: &[i32]) -> Self {
        let n = order.len();
        let mut x = Vec::with_capacity(n);
        let mut cur = 0i32;
        for &w in widths {
            x.push(cur);
            cur += w + 1;
        }
        Self {
            placeable: order,
            x,
            y: vec![row; n],
            rotation: vec![Rotation::R0; n],
        }
    }

    /// 贪心 first-fit 初始状态: 按元件顺序, 找第一个有效 `(x, y)` (按行从上到下、
    /// 列从左到右扫)。"有效" = pin 都在板内 + 不撞已摆 pin。**不**考虑列短路——
    /// 那由 SA 后续优化。
    pub fn from_greedy(placeable: Vec<ComponentId>, circuit: &Circuit, board: &Breadboard) -> Self {
        let n = placeable.len();
        let mut x = vec![0i32; n];
        let mut y = vec![0i32; n];
        let rotation = vec![Rotation::R0; n];
        let mut occupied: HashSet<(i32, i32)> = HashSet::new();

        for idx in 0..n {
            let comp_id = placeable[idx];
            let component = &circuit.components[comp_id.0];
            let fid = component.footprint.expect("placeable 必有 footprint");
            let footprint = &circuit.footprints[fid.0];

            let pin_offsets: Vec<(i32, i32)> = footprint
                .pins()
                .iter()
                .map(|p| (p.offset.x, p.offset.y))
                .collect();

            let mut found: Option<(i32, i32)> = None;
            'outer: for try_y in 0..board.rows() as i32 {
                for try_x in 0..board.cols() as i32 {
                    let oob = pin_offsets.iter().any(|&(dx, dy)| {
                        try_x + dx < 0
                            || try_x + dx >= board.cols() as i32
                            || try_y + dy < 0
                            || try_y + dy >= board.rows() as i32
                    });
                    if oob {
                        continue;
                    }
                    let collides = pin_offsets
                        .iter()
                        .any(|&(dx, dy)| occupied.contains(&(try_x + dx, try_y + dy)));
                    if collides {
                        continue;
                    }
                    found = Some((try_x, try_y));
                    break 'outer;
                }
            }

            let (fx, fy) = found.unwrap_or_else(|| panic!("元件 {} 装不下这块板", comp_id.0));
            x[idx] = fx;
            y[idx] = fy;
            for &(dx, dy) in &pin_offsets {
                occupied.insert((fx + dx, fy + dy));
            }
        }

        Self {
            placeable,
            x,
            y,
            rotation,
        }
    }

    /// 力导向初排: connected 元件拉近 (弹簧), unconnected 推远 (库仑斥力)。
    /// 产出连续 2D 位置, 然后贪心映射到网格上 (按 FD x 顺序, 每个取离 FD 目标最近
    /// 的可用格子)。比 [`Self::from_greedy`] 好的地方: 同一网里的元件自然聚簇,
    /// SA 起点跟电路拓扑对齐。
    pub fn from_force_directed(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
        config: &FDConfig,
    ) -> Self {
        let n = placeable.len();
        if n == 0 {
            return Self {
                placeable,
                x: vec![],
                y: vec![],
                rotation: vec![],
            };
        }

        // 1. Build adjacency: weights[i][j] = 同一网的连接数 (高 = 强耦合)
        let mut weights = vec![vec![0.0f64; n]; n];
        for net in circuit.nets() {
            let mut comps: Vec<usize> = net
                .pins()
                .iter()
                .map(|&pid| circuit.pins[pid.0].component.0)
                .collect();
            comps.sort();
            comps.dedup();
            for &i in &comps {
                for &j in &comps {
                    if i != j {
                        weights[i][j] += 1.0;
                    }
                }
            }
        }

        // 2. Initial: 圆周, 后面 FD 会把它们拉开
        let cols_f = board.cols() as f64;
        let rows_f = board.rows() as f64;
        let mut pos: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let angle = i as f64 * 2.0 * std::f64::consts::PI / n as f64;
                let r = cols_f.min(rows_f) * 0.4;
                (
                    cols_f / 2.0 + r * angle.cos(),
                    rows_f / 2.0 + r * angle.sin(),
                )
            })
            .collect();

        // 3. Fruchterman-Reingold 风格 FD 迭代
        let k = config.k;
        let mut temp = config.initial_temp;
        for _ in 0..config.max_iters {
            let mut forces = vec![(0.0f64, 0.0f64); n];
            for i in 0..n {
                for j in (i + 1)..n {
                    let dx = pos[j].0 - pos[i].0;
                    let dy = pos[j].1 - pos[i].1;
                    let dist = (dx * dx + dy * dy).sqrt().max(0.01);
                    let ux = dx / dist;
                    let uy = dy / dist;

                    // 斥力: 库仑型, 永远存在
                    let f_repel = k * k / dist;
                    forces[i].0 -= ux * f_repel;
                    forces[i].1 -= uy * f_repel;
                    forces[j].0 += ux * f_repel;
                    forces[j].1 += uy * f_repel;

                    // 引力: 胡克型, 仅对连接的元件
                    let w = weights[i][j];
                    if w > 0.0 {
                        let f_attr = dist * dist / k * w;
                        forces[i].0 += ux * f_attr;
                        forces[i].1 += uy * f_attr;
                        forces[j].0 -= ux * f_attr;
                        forces[j].1 -= uy * f_attr;
                    }
                }
            }

            // 应用力, 限幅到当前温度
            for i in 0..n {
                let (fx, fy) = forces[i];
                let fmag = (fx * fx + fy * fy).sqrt();
                if fmag < 1e-9 {
                    continue;
                }
                let scale = fmag.min(temp) / fmag;
                pos[i].0 += fx * scale;
                pos[i].1 += fy * scale;
                pos[i].0 = pos[i].0.clamp(0.0, cols_f - 1.0);
                pos[i].1 = pos[i].1.clamp(0.0, rows_f - 1.0);
            }

            temp *= config.cool_rate;
            if temp < 0.05 {
                break;
            }
        }

        // 4. 贪心映射到网格: 按 FD x 排序, 每个元件取离 FD 目标最近的可用格
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            pos[a]
                .0
                .partial_cmp(&pos[b].0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut x = vec![0i32; n];
        let mut y = vec![0i32; n];
        let rotation = vec![Rotation::R0; n];
        let mut occupied: HashSet<(i32, i32)> = HashSet::new();

        for &idx in &order {
            let comp_id = placeable[idx];
            let component = &circuit.components[comp_id.0];
            let fid = component.footprint.expect("placeable 必有 footprint");
            let footprint = &circuit.footprints[fid.0];

            let pin_offsets: Vec<(i32, i32)> = footprint
                .pins()
                .iter()
                .map(|p| (p.offset.x, p.offset.y))
                .collect();

            let target_x = pos[idx].0;
            let target_y = pos[idx].1;

            // 全板扫, 选离 (target_x, target_y) 最近的可用格
            let mut best: Option<(i32, i32)> = None;
            let mut best_dist_sq = f64::INFINITY;
            for try_y in 0..board.rows() as i32 {
                for try_x in 0..board.cols() as i32 {
                    let oob = pin_offsets.iter().any(|&(dx, dy)| {
                        try_x + dx < 0
                            || try_x + dx >= board.cols() as i32
                            || try_y + dy < 0
                            || try_y + dy >= board.rows() as i32
                    });
                    if oob {
                        continue;
                    }
                    let collides = pin_offsets
                        .iter()
                        .any(|&(dx, dy)| occupied.contains(&(try_x + dx, try_y + dy)));
                    if collides {
                        continue;
                    }
                    let dx = try_x as f64 - target_x;
                    let dy = try_y as f64 - target_y;
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                        best = Some((try_x, try_y));
                    }
                }
            }

            let (fx, fy) = best.expect("板太小, 装不下所有元件");
            x[idx] = fx;
            y[idx] = fy;
            for &(dx, dy) in &pin_offsets {
                occupied.insert((fx + dx, fy + dy));
            }
        }

        Self {
            placeable,
            x,
            y,
            rotation,
        }
    }
}

/// 力导向初排参数。`Default` 对 30x5 / ~18 元件级别的电路是合理起点。
#[derive(Debug, Clone, Copy)]
pub struct FDConfig {
    /// 理想距离 k: 库仑斥力 k²/d, 胡克引力 d²/k 都用它。
    /// 经验上 ≈ sqrt(board_area / num_components)。
    pub k: f64,
    /// FD 迭代数; 通常 100-300 就收敛。
    pub max_iters: usize,
    /// 初始 "温度" (单步最大位移)。
    pub initial_temp: f64,
    /// 冷却率; T *= cool_rate per iter。
    pub cool_rate: f64,
}

impl Default for FDConfig {
    fn default() -> Self {
        Self {
            // 30x5 = 150 cells, 18 元件 → sqrt(150/18) ≈ 2.9
            k: 3.0,
            max_iters: 200,
            initial_temp: 2.0,
            cool_rate: 0.95,
        }
    }
}

/// 评估当前状态的 cost。
pub fn cost(state: &SAState, circuit: &Circuit, board: &Breadboard, w: &Weights) -> f64 {
    let cols_i = board.cols() as i32;
    let rows_i = board.rows() as i32;

    // 1. 收集所有 pin 的 (col, row) 和所属 net
    let mut holes: Vec<(i32, i32)> = Vec::new();
    let mut nets: Vec<Option<NetId>> = Vec::new();
    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.unwrap();
        let footprint = &circuit.footprints[fid.0];
        let rotation = state.rotation[idx];
        let row_y = state.y[idx];
        let px = state.x[idx];

        for &pin_id in &component.pins {
            let pin = &circuit.pins[pin_id.0];
            let physical = footprint
                .pins()
                .iter()
                .find(|pp| pp.name() == pin.num())
                .expect("footprint 缺 pin (解析阶段就该爆)");
            let r = rotate(physical.offset, rotation);
            holes.push((px + r.x, row_y + r.y));
            nets.push(pin.net);
        }
    }

    // 2. OOB
    let mut oob_count = 0;
    for &(c, r) in &holes {
        if c < 0 || c >= cols_i || r < 0 || r >= rows_i {
            oob_count += 1;
        }
    }

    // 3. Pin 碰撞: 每个被多个 pin 占用的孔, 算 N-1 次 (SA 关心 Δcost, 系数 1 vs 系数 N 等价)
    let mut coll_count = 0;
    let mut seen: HashMap<(i32, i32), ()> = HashMap::new();
    for &hole in &holes {
        if in_board(hole, cols_i, rows_i) && seen.insert(hole, ()).is_some() {
            coll_count += 1;
        }
    }

    // 4. HPWL_x: 按 net 聚合, 取 (max_col - min_col) 累加
    let mut by_net: HashMap<NetId, Vec<(i32, i32)>> = HashMap::new();
    for (i, &net_opt) in nets.iter().enumerate() {
        let hole = holes[i];
        if !in_board(hole, cols_i, rows_i) {
            continue; // OOB pin 不参与 HPWL (会被压到板内, 没意义)
        }
        if let Some(net) = net_opt {
            by_net.entry(net).or_default().push(hole);
        }
    }
    let mut hpwl_sum = 0.0;
    for pins in by_net.values() {
        if pins.len() < 2 {
            continue;
        }
        let min_col = pins.iter().map(|p| p.0).min().unwrap();
        let max_col = pins.iter().map(|p| p.0).max().unwrap();
        hpwl_sum += (max_col - min_col) as f64;
    }

    // 5. 列冲突: 按列聚合, 计数"不同 net" 的对数
    let mut by_col: HashMap<i32, Vec<Option<NetId>>> = HashMap::new();
    for (i, &net_opt) in nets.iter().enumerate() {
        let hole = holes[i];
        if !in_board(hole, cols_i, rows_i) {
            continue;
        }
        by_col.entry(hole.0).or_default().push(net_opt);
    }
    let mut col_conflict_pairs = 0usize;
    for col_owners in by_col.values() {
        if col_owners.len() < 2 {
            continue;
        }
        let base = col_owners[0];
        for i in 1..col_owners.len() {
            if col_owners[i] != base {
                col_conflict_pairs += 1;
            }
        }
    }

    w.hpwl * hpwl_sum
        + w.pin_overlap * coll_count as f64
        + w.column_conflict * col_conflict_pairs as f64
        + w.out_of_bounds * oob_count as f64
}

fn in_board(hole: (i32, i32), cols: i32, rows: i32) -> bool {
    hole.0 >= 0 && hole.0 < cols && hole.1 >= 0 && hole.1 < rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::Breadboard;

    /// 1 列宽, 1 个 pin, R0/R180 等价的 footprint (没有第二个 pin 区分方向)。
    fn one_pin_fp() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "single".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        }
    }

    /// 2 列宽, 2 个 pin 紧挨着 (典型 LED 封装)。
    fn two_pin_fp() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "two".into(),
            pins: (1..=2)
                .map(|n| PhysicalPin {
                    name: n.to_string(),
                    offset: Position { x: n - 1, y: 0 },
                })
                .collect(),
        }
    }

    /// 2-pin 元件 + footprint + 1 个 net (pin1 和 pin2 都在 net A)
    fn two_pin_in_net() -> (Circuit, ComponentId) {
        let footprint = two_pin_fp();
        let comp = Component {
            id: ComponentId(0),
            ref_: "D1".into(),
            kind: "LED".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
        };
        let pins = vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: Some("K".into()),
                net: Some(NetId(0)),
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: Some("A".into()),
                net: Some(NetId(0)),
            },
        ];
        let nets = vec![Net {
            id: NetId(0),
            name: "net-a".into(),
            pins: vec![PinId(0), PinId(1)],
        }];
        let circuit = Circuit {
            components: vec![comp],
            pins,
            nets,
            footprints: vec![footprint],
        };
        (circuit, ComponentId(0))
    }

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn empty_state_costs_zero() {
        let (circuit, _) = two_pin_in_net();
        let state = SAState::from_order(vec![], 2, &[]);
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert_eq!(c, 0.0);
    }

    #[test]
    fn one_component_same_net_hpwl_is_one() {
        // 2 pin 紧挨着 (0, 2) 和 (1, 2), 都在同一 net → HPWL = 1 - 0 = 1
        let (circuit, cid) = two_pin_in_net();
        let state = SAState::from_order(vec![cid], 2, &[2]);
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert!((c - 1.0).abs() < 1e-9, "expected 1.0, got {}", c);
    }

    #[test]
    fn pin_collision_adds_penalty() {
        // 两个 1-pin footprint, 显式 x = [0, 0] 制造 pin 撞。
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[1, 1]);
        // 不撞: x = [0, 2]
        state.x = vec![0, 2];
        let c_clean = cost(&state, &circuit, &board(), &Weights::default());
        assert_eq!(c_clean, 0.0);

        // 撞: x = [0, 0]
        state.x = vec![0, 0];
        let c_coll = cost(&state, &circuit, &board(), &Weights::default());
        assert!(
            (c_coll - Weights::default().pin_overlap).abs() < 1e-9,
            "expected collision penalty, got {}",
            c_coll
        );
    }

    /// 列冲突: 同列不同 net 的 pin 对会多扣 cost
    #[test]
    fn column_conflict_adds_penalty() {
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(i)),
            })
            .collect();
        let nets = (0..2)
            .map(|i| Net {
                id: NetId(i),
                name: format!("n{i}"),
                pins: vec![PinId(i)],
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[1, 1]);

        // 不冲突: x = [0, 2]
        let mut s = state.clone();
        s.x = vec![0, 2];
        let c_clean = cost(&s, &circuit, &board(), &Weights::default());
        assert_eq!(c_clean, 0.0);

        // 冲突: x = [0, 0] (同列, 同孔 → pin_collision + column_conflict)
        let mut s = state.clone();
        s.x = vec![0, 0];
        let c_coll = cost(&s, &circuit, &board(), &Weights::default());
        let expected = Weights::default().pin_overlap + Weights::default().column_conflict;
        assert!(
            (c_coll - expected).abs() < 1e-9,
            "expected pin_overlap + column_conflict = {}, got {}",
            expected,
            c_coll
        );

        // 只 column_conflict: 同列不同行
        let mut s = state;
        s.x = vec![0, 0];
        s.y = vec![2, 3];
        let c_col_only = cost(&s, &circuit, &board(), &Weights::default());
        assert!(
            (c_col_only - Weights::default().column_conflict).abs() < 1e-9,
            "expected only column_conflict penalty, got {}",
            c_col_only
        );
    }

    #[test]
    fn oob_adds_huge_penalty() {
        let fp = one_pin_fp();
        let comp = Component {
            id: ComponentId(0),
            ref_: "X1".into(),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(0)],
            footprint: Some(FootprintId(0)),
        };
        let pins = vec![Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: None,
        }];
        let circuit = Circuit {
            components: vec![comp],
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        state.y[0] = -5;
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert!(c >= Weights::default().out_of_bounds);
    }

    #[test]
    fn from_greedy_fits_2d() {
        // 5 个 2-pin footprint, 贪心应该能放下 (5*3 = 15 cols, 5 rows = 150 cells)
        let fp = two_pin_fp();
        let comps = (0..5)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..10)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..5).map(ComponentId).collect();
        let state = SAState::from_greedy(placeable, &circuit, &board());
        assert_eq!(state.n(), 5);
        // 所有 y 都在 [0, 4]
        for &y in &state.y {
            assert!((0..5).contains(&y), "y={} not in board", y);
        }
        // 所有 x + 1 (footprint 宽 2) < 30
        for &x in &state.x {
            assert!(x + 1 < 30, "x={} 越界", x);
        }
    }

    #[test]
    fn from_greedy_spills_to_next_row() {
        // 4 个 11-col footprint (实际只 1 pin 在用), 30 col 板 → 4*11=44 > 30, 第 4 个应
        // 溢出到 row 1
        let fp = Footprint {
            id: FootprintId(0),
            name: "wide".into(),
            pins: (0..11)
                .map(|i| PhysicalPin {
                    name: i.to_string(),
                    offset: Position { x: i, y: 0 },
                })
                .collect(),
        };
        let comps = (0..4)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("W{i}"),
                kind: "W".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..4)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "0".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..4).map(ComponentId).collect();
        let state = SAState::from_greedy(placeable, &circuit, &board());
        // 3 个 11-col 放 row 0 占 0..33 (实际放 0, 1, 12, 3 个 footprint 总跨度)
        // 第 4 个放不下 row 0 → 走 row 1
        assert_eq!(
            state.y[3], 1,
            "第 4 个应去 row 1, 实际在 row {}",
            state.y[3]
        );
    }

    /// FD 基本性质: 全连通的 3 个元件, 应该聚在一起 (距离 ≤ 3)
    #[test]
    fn from_force_directed_clusters_connected() {
        let fp = one_pin_fp();
        // 3 个元件全连同一个网
        let comps = (0..3)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..3)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(0)),
            })
            .collect();
        let nets = vec![Net {
            id: NetId(0),
            name: "all".into(),
            pins: (0..3).map(PinId).collect(),
        }];
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..3).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());
        // 3 个连通的 1-pin 元件, FD 应该把它们聚在一起 (col 间距 ≤ 3)
        let xs: Vec<i32> = state.x.iter().copied().collect();
        let x_min = *xs.iter().min().unwrap();
        let x_max = *xs.iter().max().unwrap();
        assert!(
            x_max - x_min <= 3,
            "3 个全连通的元件应聚簇, 实际 x 范围: {x_min}..{x_max}"
        );
    }

    /// FD 对无连接的元件, 应该把它们推开 (距离 ≥ 2*k)
    #[test]
    fn from_force_directed_spreads_unconnected() {
        let fp = one_pin_fp();
        // 3 个元件, 都不连
        let comps = (0..3)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..3)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None, // 无 net
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..3).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());
        // 3 个 1-pin 元件无连接, 应散开 (最远 col 间距 ≥ 2)
        let xs: Vec<i32> = state.x.iter().copied().collect();
        let x_min = *xs.iter().min().unwrap();
        let x_max = *xs.iter().max().unwrap();
        assert!(
            x_max - x_min >= 2,
            "3 个无连接元件应散开, 实际 x 范围: {x_min}..{x_max}"
        );
    }

    /// FD 输出所有元件在板内、无 pin 撞
    #[test]
    fn from_force_directed_no_oob_or_collision() {
        let fp = two_pin_fp();
        let comps = (0..5)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..10)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                net: if i % 2 == 0 {
                    Some(NetId(0))
                } else {
                    Some(NetId(1))
                },
            })
            .collect();
        let nets = vec![
            Net {
                id: NetId(0),
                name: "a".into(),
                pins: (0..5).map(|i| PinId(i * 2)).collect(),
            },
            Net {
                id: NetId(1),
                name: "b".into(),
                pins: (1..5).map(|i| PinId(i * 2 + 1)).collect(),
            },
        ];
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..5).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());
        // 所有 pin 在板内
        for idx in 0..5 {
            let footprint = &circuit.footprints[0];
            for p in footprint.pins() {
                let abs_x = state.x[idx] + p.offset.x;
                let abs_y = state.y[idx] + p.offset.y;
                assert!(
                    abs_x >= 0 && abs_x < 30,
                    "x OOB: {} from {}",
                    abs_x,
                    state.x[idx]
                );
                assert!(
                    abs_y >= 0 && abs_y < 5,
                    "y OOB: {} from {}",
                    abs_y,
                    state.y[idx]
                );
            }
        }
        // pin 不撞
        let mut holes: HashSet<(i32, i32)> = HashSet::new();
        for idx in 0..5 {
            let footprint = &circuit.footprints[0];
            for p in footprint.pins() {
                let abs = (state.x[idx] + p.offset.x, state.y[idx] + p.offset.y);
                assert!(holes.insert(abs), "pin 撞了: {:?}", abs);
            }
        }
    }
}
