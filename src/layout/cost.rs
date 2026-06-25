//! 模拟退火用的成本函数: HPWL + pin 碰撞 + 越界。
//!
//! 设计要点:
//! - **HPWL_x = 一个 net 的 (max_col - min_col)**, 在"同列零成本"模型下正好等于
//!   走线 MST 长度, 所以不用算真正的 MST。
//! - 成本是各项**加权和**, 权在 [`Weights`] 里调。
//! - `SAState` 是 SA 内部状态, 只在 layout 子模块内共享。

use std::collections::HashMap;

use crate::circuit::{Circuit, ComponentId, Footprint, NetId};
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
            // 一次列短路对: 比 pin_overlap 略低, 因为"退而求其次"允许少量列冲突
            // (后期能靠走线 + 列上的同 net pin "治愈"——不是真正的电气短路, 只是
            // 跳线要迂回)
            column_conflict: 50.0,
            // 越界基本不允许; 巨大惩罚让 SA 直接拒绝
            out_of_bounds: 1_000_000.0,
        }
    }
}

/// SA 内部状态, 由 [`super::sa`] 拥有, [`cost`] 读取。
#[derive(Debug, Clone)]
pub struct SAState {
    /// 按"摆放顺序"排列的元件 id 列表; 第 0 个最左, 最后那个最右。
    pub placeable: Vec<ComponentId>,
    /// 每个元件的旋转, v1 只用 [`Rotation::R0`] / [`Rotation::R180`], 其他值会 panic。
    pub rotation: Vec<Rotation>,
    /// 每个元件所在行。
    pub row: Vec<i32>,
}

impl SAState {
    /// 从给定的元件顺序构造初始状态: 全部 R0, 全部同一行。
    pub fn from_order(order: Vec<ComponentId>, row: i32) -> Self {
        let n = order.len();
        Self {
            placeable: order,
            rotation: vec![Rotation::R0; n],
            row: vec![row; n],
        }
    }

    pub fn n(&self) -> usize {
        self.placeable.len()
    }
}

/// R0 方向下 footprint 占多少列 (`max_x - min_x + 1`)。
///
/// 跟 [`super::footprint_horizontal_width`] 重复: 那个是 `Layout` 私有的,
/// 这个公开给 `cost` / `sa` 用。逻辑保持一致。
pub fn footprint_horizontal_width(footprint: &Footprint) -> i32 {
    if footprint.pins.is_empty() {
        return 1;
    }
    let min_x = footprint.pins.iter().map(|p| p.offset.x).min().unwrap();
    let max_x = footprint.pins.iter().map(|p| p.offset.x).max().unwrap();
    max_x - min_x + 1
}

/// 从"摆放顺序"派生每个元件的 x 坐标 (从 x=0 起, 元件间留 1 空列)。
pub fn derive_x(state: &SAState, circuit: &Circuit) -> Vec<i32> {
    derive_x_with_gap(state, circuit, 1)
}

/// 同 [`derive_x`], 但元件间的 gap 可调 (`0` = 贴紧, 负数 = 重叠)。
/// 主要给测试用——验证 pin 碰撞代价需要让两个元件落同一格。
pub fn derive_x_with_gap(state: &SAState, circuit: &Circuit, gap: i32) -> Vec<i32> {
    let mut x = Vec::with_capacity(state.n());
    let mut cur = 0i32;
    for &comp_id in &state.placeable {
        let fid = circuit.components[comp_id.0]
            .footprint
            .expect("placeable 必有 footprint");
        let footprint = &circuit.footprints[fid.0];
        let width = footprint_horizontal_width(footprint);
        x.push(cur);
        cur += width + gap;
    }
    x
}

/// 评估当前状态的 cost。
///
/// 各项**独立计算**然后加权求和; 没做增量, 因为 `n` 小。
/// 如果以后 `n` 大了, 改成只重算受影响的 net / hole 即可。
pub fn cost(state: &SAState, circuit: &Circuit, board: &Breadboard, w: &Weights) -> f64 {
    let x = derive_x(state, circuit);
    cost_with_x(state, circuit, board, w, &x)
}

/// 跟 [`cost`] 一样, 但用调用者提供的 x 坐标 (避免被 `derive_x` 默认 gap=1 限制)。
/// 测试需要让两个元件落同一格时用这个。
pub fn cost_with_x(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    w: &Weights,
    x: &[i32],
) -> f64 {
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
        let row_y = state.row[idx];
        let px = x[idx];

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
    // (一对两个 pin 在同列, 但 net 不同, 计 1)
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
        // 选第一个作为"基准", 数后面有几个跟它不同 (None != Some 也算不同)
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
        let state = SAState::from_order(vec![], 2);
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert_eq!(c, 0.0);
    }

    #[test]
    fn one_component_same_net_hpwl_is_zero() {
        // 2 pin 紧挨着 (0, 2) 和 (1, 2), 都在同一 net → HPWL = 1 - 0 = 1
        let (circuit, cid) = two_pin_in_net();
        let state = SAState::from_order(vec![cid], 2);
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert!(c > 0.0, "HPWL 应该非零: {}", c);
        // 验证不超预期: hpwl(1) + 0 collision + 0 oob
        assert!((c - 1.0).abs() < 1e-9, "expected 1.0, got {}", c);
    }

    #[test]
    fn pin_collision_adds_penalty() {
        // 两个 1-pin footprint, 用 `cost_with_x` 直接给 x = [0, 0] 制造 pin 撞。
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
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2);

        // 不撞: x = [0, 2]
        let c_clean = cost_with_x(&state, &circuit, &board(), &Weights::default(), &[0, 2]);
        assert_eq!(c_clean, 0.0);

        // 撞: x = [0, 0]
        let c_coll = cost_with_x(&state, &circuit, &board(), &Weights::default(), &[0, 0]);
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
                net: Some(NetId(i)), // 不同 net
            })
            .collect();
        let nets = (0..2)
            .map(|i| crate::circuit::Net {
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
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2);

        // 不冲突: x = [0, 2]
        let c_clean = cost_with_x(&state, &circuit, &board(), &Weights::default(), &[0, 2]);
        assert_eq!(c_clean, 0.0);

        // 冲突: x = [0, 0] (同列, 同时也是同孔 → pin_collision + column_conflict)
        let c_coll = cost_with_x(&state, &circuit, &board(), &Weights::default(), &[0, 0]);
        // pin_overlap(100) + column_conflict(50) = 150
        let expected = Weights::default().pin_overlap + Weights::default().column_conflict;
        assert!(
            (c_coll - expected).abs() < 1e-9,
            "expected pin_overlap + column_conflict = {}, got {}",
            expected,
            c_coll
        );

        // 只计 column_conflict: 同列不同行 (不撞 pin, 因为不同孔)
        // X0 在 row=2, X1 在 row=3, x 都是 0
        // 两个 pin 都在 col 0, 不同 row → 只 column_conflict, 不 pin_collision
        let mut state2 = state;
        state2.row[1] = 3;
        let c_col_only = cost_with_x(&state2, &circuit, &board(), &Weights::default(), &[0, 0]);
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
        let state = SAState::from_order(vec![ComponentId(0)], -5);
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert!(c >= Weights::default().out_of_bounds);
    }
}
