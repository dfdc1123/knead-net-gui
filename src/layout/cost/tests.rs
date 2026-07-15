//! cost 公共 API 的集成测试 (原本 inline 在 cost.rs 末尾的 `#[cfg(test)] mod tests`)。

use super::*;

use crate::circuit::{
    Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin, PinId,
    Position,
};
use crate::layout::Breadboard;
use crate::layout::breadboard::{PowerRailBinding, Region, standard_power_rails};
use crate::layout::cost::bridge::collect_matching_rail_ids;
use crate::layout::placement::{Placement, Rotation};

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
        bridgeable: false,
    };
    let pins = vec![
        Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: Some("K".into()),
            physical_pin_index: 0,
            net: Some(NetId(0)),
        },
        Pin {
            id: PinId(1),
            component: ComponentId(0),
            num: "2".into(),
            pinfunction: Some("A".into()),
            physical_pin_index: 1,
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

/// 只关心 MST / pin / bbox / column 各项的测试用, 屏蔽新加的紧凑度和跨 rail 惩罚。
/// 不想让"layout 跨几行" 之类的全局性质混入到孤立某项成本的断言里。
/// 显式 mst=1.0 让 "1 cell MST → cost 1.0" 这种简单算术在测试里成立
/// (默认 mst=5.0 是给 SA 跑的; 测试要看的不是权重而是公式结构)。
fn weights_legacy() -> Weights {
    Weights {
        mst: 1.0,
        compactness: 0.0,
        rail_crossing: 0.0,
        row_squash: 0.0,
        ..Weights::default()
    }
}

#[test]
fn empty_state_costs_zero() {
    let (circuit, _) = two_pin_in_net();
    let state = SAState::from_order(vec![], 2, &[]);
    let c = cost(&state, &circuit, &board(), &[], &Weights::default());
    assert_eq!(c, 0.0);
}

#[test]
fn one_component_same_net_mst_is_one() {
    // 2 pin 紧挨着 (0, 2) 和 (1, 2), 都在同一 net → MST = |1-0| = 1
    let (circuit, cid) = two_pin_in_net();
    let state = SAState::from_order(vec![cid], 2, &[2]);
    let c = cost(&state, &circuit, &board(), &[], &weights_legacy());
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
            bridgeable: false,
        })
        .collect();
    let pins = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
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
    let c_clean = cost(&state, &circuit, &board(), &[], &weights_legacy());
    assert_eq!(c_clean, 0.0);

    // 撞: x = [0, 0]
    state.x = vec![0, 0];
    let c_coll = cost(&state, &circuit, &board(), &[], &weights_legacy());
    let expected = weights_legacy().pin_overlap + weights_legacy().b_box_overlap;
    assert!(
        (c_coll - expected).abs() < 1e-9,
        "expected pin_overlap + b_box_overlap = {}, got {}",
        expected,
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
            bridgeable: false,
        })
        .collect();
    let pins = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
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
    let c_clean = cost(&s, &circuit, &board(), &[], &weights_legacy());
    assert_eq!(c_clean, 0.0);

    // 冲突: x = [0, 0] (同列, 同孔 → pin_collision + bbox_collision + column_conflict)
    let mut s = state.clone();
    s.x = vec![0, 0];
    let c_coll = cost(&s, &circuit, &board(), &[], &weights_legacy());
    let expected = weights_legacy().pin_overlap
        + weights_legacy().b_box_overlap
        + weights_legacy().column_conflict;
    assert!(
        (c_coll - expected).abs() < 1e-9,
        "expected pin_overlap + b_box_overlap + column_conflict = {}, got {}",
        expected,
        c_coll
    );

    // 只 column_conflict: 同列不同行
    let mut s = state;
    s.x = vec![0, 0];
    s.y = vec![2, 3];
    let c_col_only = cost(&s, &circuit, &board(), &[], &weights_legacy());
    assert!(
        (c_col_only - weights_legacy().column_conflict).abs() < 1e-9,
        "expected only column_conflict penalty, got {}",
        c_col_only
    );
}

/// 标准板上, 同列不同 rail 的不同 net pin 不该被记为列冲突。
#[test]
fn column_conflict_ignores_different_rails_in_cost() {
    let board = crate::layout::Breadboard::standard();
    let fp = one_pin_fp();
    let comps = (0..2)
        .map(|i| Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(i)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        })
        .collect();
    let pins = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
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
    let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 0, &[1, 1]);
    // 同 col 0, C0 在上 rail (y=2), C1 在下 rail (y=10) — 物理不连通
    state.x = vec![0, 0];
    state.y = vec![2, 10];
    let c = cost(&state, &circuit, &board, &[], &weights_legacy());
    assert_eq!(
        c, 0.0,
        "上下 rail 同列不同 net 不该被 cost 记为冲突, got {c}"
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
        bridgeable: false,
    };
    let pins = vec![Pin {
        id: PinId(0),
        component: ComponentId(0),
        num: "1".into(),
        pinfunction: None,
        physical_pin_index: 0,
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
    let c = cost(&state, &circuit, &board(), &[], &Weights::default());
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
            bridgeable: false,
        })
        .collect();
    let pins = (0..10)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i / 2),
            num: ((i % 2) + 1).to_string(),
            pinfunction: None,
            physical_pin_index: 0,
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
    let state = SAState::from_greedy(
        placeable,
        &circuit,
        &board(),
        &crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        },
        &crate::layout::problem::AnnealProblem::default(),
    );
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
            bridgeable: false,
        })
        .collect();
    let pins = (0..4)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "0".into(),
            pinfunction: None,
            physical_pin_index: 0,
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
    let state = SAState::from_greedy(
        placeable,
        &circuit,
        &board(),
        &crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        },
        &crate::layout::problem::AnnealProblem::default(),
    );
    // 3 个 11-col 放 row 0 占 0..33 (实际放 0, 1, 12, 3 个 footprint 总跨度)
    // 第 4 个放不下 row 0 → 走 row 1
    assert_eq!(
        state.y[3], 1,
        "第 4 个应去 row 1, 实际在 row {}",
        state.y[3]
    );
}

// ============================================================
//  MST cost 测试
// ============================================================

/// MST 边距: 同 rail_id = 0 (不管是 vertical 还是 power rail)
#[test]
fn mst_same_col_same_rail_is_zero() {
    let b = Breadboard::new(30, 5);
    let len = mst_wire_length(&[pin(&b, 0, 0), pin(&b, 0, 2)]);
    assert_eq!(len, 0.0, "同列同 rail 应该 rail 短接, MST = 0");
}

/// MST 边距: 同 rail 不同 col = |Δcol| (jumper)
#[test]
fn mst_same_rail_different_col_is_abs_col_delta() {
    let b = Breadboard::new(30, 5);
    let len = mst_wire_length(&[pin(&b, 0, 2), pin(&b, 3, 2)]);
    assert_eq!(len, 3.0, "同 rail 不同 col = |Δcol| = 3");
}

/// MST 边距: 不同 rail (跨中央通道) = Manhattan
#[test]
fn mst_same_col_different_rail_is_abs_row_delta() {
    let b = Breadboard::standard(); // rows 5, 6 blocked
    let len = mst_wire_length(&[pin(&b, 5, 0), pin(&b, 5, 8)]);
    assert_eq!(len, 8.0, "同 col 跨 rail = |Δrow| = 8");
}

/// MST 边距: 不同 col 不同 rail = Manhattan
#[test]
fn mst_different_col_different_rail_is_manhattan() {
    // row 2 blocked → (0, 0) 在 rail 0 (row 0..1), (3, 4) 在 rail 1 (row 3..11)
    let b = Breadboard::with_blocked_rows(30, 12, [2]);
    let len = mst_wire_length(&[pin(&b, 0, 0), pin(&b, 3, 4)]);
    assert_eq!(len, 7.0, "不同 col 不同 rail = 3 + 4 = 7");
}

/// MST: 3 pin 的 net, 三角形走最短 (2 条边)
#[test]
fn mst_three_pins_picks_two_shortest_edges() {
    let b = Breadboard::new(30, 5);
    // 3 pin: (0,0), (1,0), (5,0) — 都在同一 rail
    // 边: 0-1 (1), 0-5 (5), 1-5 (4) → MST 取 0-1 + 1-5 = 5
    let len = mst_wire_length(&[pin(&b, 0, 0), pin(&b, 1, 0), pin(&b, 5, 0)]);
    assert_eq!(len, 5.0);
}

/// helper: 从 board 反查 (x, y) 对应的 (x, y, rail_id) 测试输入
fn pin(b: &Breadboard, x: i32, y: i32) -> (i32, i32, u32) {
    let id = b
        .at(x, y)
        .unwrap_or_else(|| panic!("测试 pin ({x},{y}) 不在板上"));
    (x, y, b.effective_rail_id_of(id))
}

// ============================================================
//  Power rail 短路 测试
// ============================================================

/// 同 power rail 行内 (不同 col) → MST = 0 (横向短接)
#[test]
fn mst_same_power_rail_row_is_zero() {
    let b = Breadboard::standard();
    // top negative y=-4, col 0 和 col 10
    let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 10, -4)]);
    assert_eq!(len, 0.0, "同 power rail 行内应该 shorted, MST = 0");
}

/// preset 用显式 RailTie 连接同极性 top + bottom → MST = 0
#[test]
fn mst_top_and_bottom_same_polarity_is_zero() {
    let b = Breadboard::standard();
    let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 0, 14)]);
    assert_eq!(len, 0.0, "上下两条同极性应该 shorted, MST = 0");
}

#[test]
fn mst_top_and_bottom_without_tie_uses_physical_distance() {
    let b = Breadboard::with_power_rails(30, 12, [5, 6], standard_power_rails(30));
    let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 0, 14)]);
    assert_eq!(
        len, 18.0,
        "没有 RailTie 时 top/bottom 必须按两个独立 islands 计算"
    );
}

#[test]
fn bound_power_net_matches_each_untied_power_row() {
    let raw = Breadboard::with_power_rails(30, 12, [5, 6], standard_power_rails(30))
        .with_power_rail_binding(PowerRailBinding {
            positive: Some(NetId(0)),
            negative: None,
        });
    let preset = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: None,
    });

    assert_eq!(collect_matching_rail_ids(&raw, Some(NetId(0))).len(), 2);
    assert_eq!(
        collect_matching_rail_ids(&preset, Some(NetId(0))).len(),
        1,
        "preset tie 应把上下轨归入同一 effective component"
    );
}

/// 正负极 → MST = Manhattan (不短接)
#[test]
fn mst_positive_and_negative_is_manhattan() {
    let b = Breadboard::standard();
    // (0, -4) negative, (6, -3) positive → |6| + |1| = 7
    // (6 是 group 第二个的开始: cols 6..10)
    let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 6, -3)]);
    assert_eq!(len, 7.0, "正负极不短接, MST = Manhattan");
}

/// Power rail 跟 main board → MST = Manhattan (rail_id 不同)
#[test]
fn mst_power_rail_to_main_is_manhattan() {
    let b = Breadboard::standard();
    // top negative (0, -4) 跟 main upper (0, 0): |0| + |4| = 4
    let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 0, 0)]);
    assert_eq!(len, 4.0);
}

// ============================================================
//  PowerRailBinding 虚拟 pin
// ============================================================

/// 绑定 GND 到负极: 1 个 GND pin 在 (10, 0), 加上虚拟 pin 在 (0, -4)。
/// MST 距离 = |10| + |4| = 14 (主区到 rail 的 jumper 长度)。
/// 不绑定时, 那个 pin 单独一个节点, MST = 0。
/// 用 delta 检验: cost(绑定) - cost(不绑定) 应该 = 14 * mst_weight。
#[test]
fn cost_with_binding_reflects_rail_jumper() {
    use crate::circuit::{ComponentId, FootprintId, NetId, PinId};
    use crate::layout::cost::{SAState, Weights};

    let footprint = crate::circuit::Footprint {
        id: FootprintId(0),
        name: "1p".into(),
        pins: vec![crate::circuit::PhysicalPin {
            name: "1".into(),
            offset: crate::circuit::Position { x: 0, y: 0 },
        }],
    };
    let component = crate::circuit::Component {
        id: ComponentId(0),
        ref_: "R1".into(),
        kind: "R".into(),
        value: None,
        pins: vec![PinId(0)],
        footprint: Some(FootprintId(0)),
        bridgeable: false,
    };
    let pin = crate::circuit::Pin {
        id: PinId(0),
        component: ComponentId(0),
        num: "1".into(),
        pinfunction: None,
        physical_pin_index: 0,
        net: Some(NetId(0)),
    };
    let net = crate::circuit::Net {
        id: NetId(0),
        name: "GND".into(),
        pins: vec![PinId(0)],
    };
    let circuit = crate::circuit::Circuit {
        components: vec![component],
        pins: vec![pin],
        nets: vec![net],
        footprints: vec![footprint],
    };
    let mut state = SAState::from_order(vec![ComponentId(0)], 1, &[1]);
    state.x[0] = 10;
    state.y[0] = 0;
    let w = Weights::default();

    // 不绑定
    let board_no_bind = Breadboard::standard();
    let cost_no = cost(&state, &circuit, &board_no_bind, &[], &w);

    // 绑定: 虚拟 pin (0, -4) 加入 net, MST = |10| + |4| = 14
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(0)),
    });
    let cost_with = cost(&state, &circuit, &board, &[], &w);

    let delta = cost_with - cost_no;
    let expected_delta = 14.0 * w.mst; // 纯 MST 增量
    assert!(
        (delta - expected_delta).abs() < 0.01,
        "绑定后 cost 增量 = MST 14, 实际 delta = {delta}, 期望 = {expected_delta}"
    );
}

/// 不绑定时, 成本跟以前完全一样 (虚拟 pin 0 个)。
/// 上面那个测试的不绑定部分已覆盖, 这里再加个明显不动的检查: 0 元件 0 pin。
#[test]
fn cost_no_binding_no_rail_pins() {
    use crate::circuit::{ComponentId, FootprintId, NetId, PinId};

    // 2-pin 元件, 2 个 pin 都在同一 rail, 同 net → MST = 0
    // 不绑定: 0 虚拟 pin, 跟以前一样
    let footprint = crate::circuit::Footprint {
        id: FootprintId(0),
        name: "2p".into(),
        pins: vec![
            crate::circuit::PhysicalPin {
                name: "1".into(),
                offset: crate::circuit::Position { x: 0, y: 0 },
            },
            crate::circuit::PhysicalPin {
                name: "2".into(),
                offset: crate::circuit::Position { x: 1, y: 0 },
            },
        ],
    };
    let component = crate::circuit::Component {
        id: ComponentId(0),
        ref_: "R1".into(),
        kind: "R".into(),
        value: None,
        pins: vec![PinId(0), PinId(1)],
        footprint: Some(FootprintId(0)),
        bridgeable: false,
    };
    let pins = vec![
        crate::circuit::Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: Some(NetId(0)),
        },
        crate::circuit::Pin {
            id: PinId(1),
            component: ComponentId(0),
            num: "2".into(),
            pinfunction: None,
            physical_pin_index: 1,
            net: Some(NetId(0)),
        },
    ];
    let net = crate::circuit::Net {
        id: NetId(0),
        name: "n".into(),
        pins: vec![PinId(0), PinId(1)],
    };
    let circuit = crate::circuit::Circuit {
        components: vec![component],
        pins,
        nets: vec![net],
        footprints: vec![footprint],
    };
    let state = crate::layout::cost::SAState::from_order(vec![ComponentId(0)], 1, &[2]);
    let board = Breadboard::standard();
    let c = cost(
        &state,
        &circuit,
        &board,
        &[],
        &crate::layout::cost::Weights::default(),
    );
    // cost = MST 1 (同 rail 不同 col, |Δcol|=1) × 5.0 (默认 mst 权重)
    //     + compactness 1.0 (2×1×0.5) = 6.0
    // 验证不绑定时, 没注入虚拟 pin 进去 (否则 cost 会更高)
    assert_eq!(
        c, 6.0,
        "不绑定, 同 rail 同 net, cost = MST 1 × mst 5.0 + compactness 1.0 = 6.0"
    );
}

/// 成本函数走 MST 而非 HPWL:
/// 同列不同 row (同 rail) → cost = 0 (零跳线); 而 2D HPWL 会算 = Δrow
#[test]
fn cost_zero_jumper_layout_costs_zero() {
    // 2 个 1-pin 元件, 都在 col 0, 不同 row, 同 net
    // → MST 距离 = 0 (rail 短接)
    let fp = one_pin_fp();
    let comps: Vec<Component> = (0..2)
        .map(|i| Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(i)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        })
        .collect();
    let pins: Vec<Pin> = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: Some(NetId(0)),
        })
        .collect();
    let nets = vec![Net {
        id: NetId(0),
        name: "n".into(),
        pins: vec![PinId(0), PinId(1)],
    }];
    let circuit = Circuit {
        components: comps,
        pins,
        nets,
        footprints: vec![fp],
    };
    let state = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 0],
        y: vec![0, 1],
        rotation: vec![Rotation::R0, Rotation::R0],
        ..SAState::no_bridging(2)
    };
    let c = cost(&state, &circuit, &board(), &[], &weights_legacy());
    assert!(c.abs() < 1e-9, "零跳线布局应该 cost = 0, got {}", c);
}

// ============================================================
//  紧凑度 + 跨 rail 惩罚
// ============================================================

/// 同样 2 个 1-pin 元件, 都同 rail 单行: cost 应随水平跨度线性增长, 垂直 y 不变不增加
/// (x 和 y 等同计入, 但仅以 1 个 dimension 变化时只有那一项 +1)。
#[test]
fn compactness_penalizes_horizontal_spread() {
    let fp = one_pin_fp();
    let comps = (0..2)
        .map(|i| Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(i)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        })
        .collect();
    let pins = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        })
        .collect();
    let circuit = Circuit {
        components: comps,
        pins,
        nets: vec![],
        footprints: vec![fp],
    };
    // 屏蔽 MST / pin / bbox / column / row_squash, 只看 compactness
    let w = Weights {
        mst: 0.0,
        pin_overlap: 0.0,
        b_box_overlap: 0.0,
        column_conflict: 0.0,
        row_squash: 0.0,
        ..Weights::default()
    };
    // 都同 row 2, x 贴在一起 (但不同 col, 不撞 pin)
    let s_tight = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 1],
        y: vec![2, 2],
        rotation: vec![Rotation::R0; 2],
        ..SAState::no_bridging(2)
    };
    let c_tight = cost(&s_tight, &circuit, &board(), &[], &w);
    // 同 row 2, x 拉开 (0, 5) → bbox 6 × 1 = 6 → cost 3.0
    let s_wide = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 5],
        y: vec![2, 2],
        rotation: vec![Rotation::R0; 2],
        ..SAState::no_bridging(2)
    };
    let c_wide = cost(&s_wide, &circuit, &board(), &[], &w);
    // 贴一起: bbox 2×1 = 2 → 1.0
    // 拉开 5 列: bbox 6×1 = 6 → 3.0
    assert!(
        (c_tight - 1.0).abs() < 1e-9,
        "贴一起 (x 0..1) 应 cost = 0.5 * 2 = 1.0, got {c_tight}"
    );
    assert!(
        (c_wide - 3.0).abs() < 1e-9,
        "拉开 5 列 (x 0..5) 应 cost = 0.5 * 6 = 3.0, got {c_wide}"
    );
    assert!(c_wide > c_tight);
}

/// 同样 2 个 1-pin 元件, 同列: cost 随垂直跨度增长, 跟水平等价 (x / y 平等)。
/// 水平拉开应加紧凑度 cost, 垂直拉开不应加 (紧凑度只算 x)。
#[test]
fn compactness_only_x_not_y() {
    let fp = one_pin_fp();
    let comps = (0..2)
        .map(|i| Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(i)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        })
        .collect();
    let pins = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        })
        .collect();
    let circuit = Circuit {
        components: comps,
        pins,
        nets: vec![],
        footprints: vec![fp],
    };
    let w = Weights {
        mst: 0.0,
        pin_overlap: 0.0,
        b_box_overlap: 0.0,
        column_conflict: 0.0,
        row_squash: 0.0,
        ..Weights::default()
    };

    // 水平拉开 5 cells (x 0..4, width 5) → compactness = 0.5 * 5 = 2.5
    let s_horiz = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 4],
        y: vec![2, 2],
        rotation: vec![Rotation::R0; 2],
        ..SAState::no_bridging(2)
    };
    let c_horiz = cost(&s_horiz, &circuit, &board(), &[], &w);
    assert!(
        (c_horiz - 2.5).abs() < 1e-9,
        "水平拉开 5 cells → compactness=2.5, got {c_horiz}"
    );
    let c_horiz = cost(&s_horiz, &circuit, &board(), &[], &w);
    assert!(
        (c_horiz - 2.5).abs() < 1e-9,
        "水平拉开 5 cells → compactness=2.5, got {c_horiz}"
    );

    // 垂直拉开 5 cells (y 0..4, x 相同) → compactness = 0.5 * 1 = 0.5 (x 跨度仅 1)
    let s_vert = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 0],
        y: vec![0, 4],
        rotation: vec![Rotation::R0; 2],
        ..SAState::no_bridging(2)
    };
    let c_vert = cost(&s_vert, &circuit, &board(), &[], &w);
    assert!(
        (c_vert - 0.5).abs() < 1e-9,
        "垂直拉开不应加 compactness, got {c_vert}"
    );
}

/// 跨 rail (中央通道上下都放) 应该加一个 rail_crossing 固定项。
#[test]
fn compactness_rail_crossing_penalty() {
    let board = crate::layout::Breadboard::standard();
    let fp = one_pin_fp();
    let comps = (0..2)
        .map(|i| Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(i)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        })
        .collect();
    let pins = (0..2)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        })
        .collect();
    let circuit = Circuit {
        components: comps,
        pins,
        nets: vec![],
        footprints: vec![fp],
    };
    let w = Weights {
        mst: 0.0,
        pin_overlap: 0.0,
        b_box_overlap: 0.0,
        column_conflict: 0.0,
        ..Weights::default()
    };

    // 同 rail: 无 rail_crossing, compactness = 0.5*1 = 0.5
    let s_same = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 0],
        y: vec![0, 1], // 都是上 rail
        rotation: vec![Rotation::R0; 2],
        ..SAState::no_bridging(2)
    };
    let c_same = cost(&s_same, &circuit, &board, &[], &w);

    // 跨 rail (中央通道两侧): compactness 上下各 0.5, 加 rail_crossing 5.0 = 6.0
    let s_cross = SAState {
        placeable: vec![ComponentId(0), ComponentId(1)],
        x: vec![0, 0],
        y: vec![0, 10], // 上 + 下
        rotation: vec![Rotation::R0; 2],
        ..SAState::no_bridging(2)
    };
    let c_cross = cost(&s_cross, &circuit, &board, &[], &w);

    let expected_delta = 0.5 + w.rail_crossing; // compactness diff + rail_crossing
    assert!(
        (c_cross - c_same - expected_delta).abs() < 1e-9,
        "跨 rail 应多出 compactness_diff(0.5) + rail_crossing({}) = {expected_delta}, got {}",
        w.rail_crossing,
        c_cross - c_same
    );
}

/// 按 rail 分组: 跨中央通道不应被算成"垂直跨度 7 行"让 area 虚胖。
/// 也就是说, 上 rail 内 bbox 和下 rail 内 bbox 各自算, 不拼接。
#[test]
fn compactness_rail_split_avoids_central_channel_inflation() {
    let board = crate::layout::Breadboard::standard();
    let fp = one_pin_fp();
    // 3 个 1-pin 元件: 2 个上 rail (y=0, 1), 1 个下 rail (y=10)
    let comps = (0..3)
        .map(|i| Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(i)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        })
        .collect();
    let pins = (0..3)
        .map(|i| Pin {
            id: PinId(i),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        })
        .collect();
    let circuit = Circuit {
        components: comps,
        pins,
        nets: vec![],
        footprints: vec![fp],
    };
    let w = Weights {
        mst: 0.0,
        pin_overlap: 0.0,
        b_box_overlap: 0.0,
        column_conflict: 0.0,
        ..Weights::default()
    };

    // 同样 3 个元件, 都堆在上 rail, x 拉开避免 pin 撞
    let s_all_upper = SAState {
        placeable: vec![ComponentId(0), ComponentId(1), ComponentId(2)],
        x: vec![0, 1, 2],
        y: vec![0, 1, 2],
        rotation: vec![Rotation::R0; 3],
        ..SAState::no_bridging(3)
    };
    let c_all_upper = cost(&s_all_upper, &circuit, &board, &[], &w);

    // 1 个下 rail (y=10), 2 个上 rail (y=0, 1)
    let s_split = SAState {
        placeable: vec![ComponentId(0), ComponentId(1), ComponentId(2)],
        x: vec![0, 1, 2],
        y: vec![0, 1, 10],
        rotation: vec![Rotation::R0; 3],
        ..SAState::no_bridging(3)
    };
    let c_split = cost(&s_split, &circuit, &board, &[], &w);

    // 都上 rail: x=0..2, width=3; compactness = 0.5*3 = 1.5; row_squash: 3 comps 3 rows → 0
    assert!(
        (c_all_upper - 1.5).abs() < 1e-9,
        "全上 rail 应 cost = 0.5 * 3 = 1.5, got {c_all_upper}"
    );
    // split: 上 rail x=0..1 width=2 compactness=1.0; 下 rail x=2 width=1 compactness=0.5;
    //        总 compactness = 1.5; 加 rail_crossing 5 = 6.5
    assert!(
        (c_split - (0.5 * 2.0 + 0.5 * 1.0 + w.rail_crossing)).abs() < 1e-9,
        "split 布局应 cost = 1.5 + 5.0 = 6.5, got {c_split}"
    );
}

// ============================================================
//  桥接路径
// ============================================================

/// 1 个 2-pin 水平电阻 (pin offset Δ=(3,0)) + power net 绑定到 top positive rail。
/// 启发式应该: power pin 落 top positive rail 第一个 hole, signal pin 经 R90
/// 旋转后落 (col, row 0) main rail。返回 Some 且结构合法。
#[test]
fn propose_bridged_pair_uses_r90_for_horizontal_resistor() {
    use crate::circuit::{Footprint, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    let fp = Footprint {
        id: FootprintId(0),
        name: "R".into(),
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
    // pin 0 走 +12V (power), pin 1 走 SIG (signal)
    let circuit = crate::circuit::Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: true,
        }],
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
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(1)),
            },
        ],
        nets: vec![
            crate::circuit::Net {
                id: NetId(0),
                name: "+12V".into(),
                pins: vec![PinId(0)],
            },
            crate::circuit::Net {
                id: NetId(1),
                name: "SIG".into(),
                pins: vec![PinId(1)],
            },
        ],
        footprints: vec![fp],
    };
    // 绑定: top/bottom positive rail  ← +12V, top/bottom negative rail ← SIG
    // (信号名号无所谓, 只曹 power_net_ids 包含 NetId(0) 即可)
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)),
    });
    let comp = &circuit.components[0];
    let pair = propose_bridged_pair(comp, &circuit, &board, &[NetId(0), NetId(1)]);
    let pair = pair.expect("启发式应该能找一对合法桥接");
    let (h_power, pin_power) = pair[0];
    let (h_signal, pin_signal) = pair[1];
    // power 必须是 power rail
    assert_eq!(
        board.region_of(h_power),
        Region::PowerRail,
        "power 腿应落 power rail"
    );
    // signal 必须是 main rail
    assert_eq!(
        board.region_of(h_signal),
        Region::MainRail,
        "signal 腿应落 main rail"
    );
    // pin 标识要反映 power / signal 分工
    assert_eq!(pin_power, PinId(0), "pin 0 (net=+12V) 应是 power");
    assert_eq!(pin_signal, PinId(1), "pin 1 (net=SIG) 应是 signal");
    // body 方向: 两孔 x 差 == 0 (R90 后), y 差 == 3。证实是 R90 不是 R0 / R180。
    let p_p = board.hole(h_power).position;
    let p_s = board.hole(h_signal).position;
    assert_eq!(
        p_s.x - p_p.x,
        0,
        "R90 后 Δx 应 = 0 (body 竖直), got Δx = {}",
        p_s.x - p_p.x
    );
    assert_eq!(
        p_s.y - p_p.y,
        3,
        "R90 后 Δy 应 = 3 (footprint 跨度), got Δy = {}",
        p_s.y - p_p.y
    );
}

/// 没绑 power rail → power_net_ids 为空 → 启发式返 None。
#[test]
fn propose_bridged_pair_returns_none_without_power_rail_binding() {
    use crate::circuit::{Footprint, PhysicalPin};
    let fp = Footprint {
        id: FootprintId(0),
        name: "R".into(),
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
    let circuit = crate::circuit::Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: true,
        }],
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
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(1)),
            },
        ],
        nets: vec![
            crate::circuit::Net {
                id: NetId(0),
                name: "P".into(),
                pins: vec![PinId(0)],
            },
            crate::circuit::Net {
                id: NetId(1),
                name: "S".into(),
                pins: vec![PinId(1)],
            },
        ],
        footprints: vec![fp],
    };
    let board = Breadboard::standard(); // 不绑 power rail
    let comp = &circuit.components[0];
    // power_net_ids 为空 → power 腿找不到匹配 → 返 None
    let pair = propose_bridged_pair(comp, &circuit, &board, &[]);
    assert!(pair.is_none(), "无 power rail 时启发式应返 None");
}

/// bridged 状态的 2-pin 元件: 算 cost 时 pin 走 bridged_pin_pair, 不走 (x, y, rotation),
/// 不计 bbox, 不计 OOB (因为启发式保证两个孔都是合法的)。
/// 对比: 同一 bridgeable 元件, OnBoard 走 OOB 区域 vs Bridged 走启发式合法位,
/// 后者的 cost 远低于前者 (无 OOB 巨罚, 也无越界让 cost 龲起)。
#[test]
fn cost_bridged_uses_heuristic_pair_and_skips_bbox() {
    use crate::circuit::{Footprint, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    let fp = Footprint {
        id: FootprintId(0),
        name: "R".into(),
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
    let circuit = crate::circuit::Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: true,
        }],
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
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(1)),
            },
        ],
        nets: vec![
            crate::circuit::Net {
                id: NetId(0),
                name: "P".into(),
                pins: vec![PinId(0)],
            },
            crate::circuit::Net {
                id: NetId(1),
                name: "S".into(),
                pins: vec![PinId(1)],
            },
        ],
        footprints: vec![fp],
    };
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)),
    });

    // OnBoard 状态: (0, 0) 放, 令 pin 1 跨进 gap (y=-1) → OOB 龲高 cost
    let on_board = SAState {
        placeable: vec![ComponentId(0)],
        x: vec![0],
        y: vec![0],
        rotation: vec![Rotation::R0],
        ..SAState::no_bridging(1)
    };
    let w = Weights::default();
    let c_on = cost(&on_board, &circuit, &board, &[], &w);

    // Bridged 状态: 走启发式合法对
    let pair = propose_bridged_pair(
        &circuit.components[0],
        &circuit,
        &board,
        &[NetId(0), NetId(1)],
    )
    .unwrap();
    let mut bridged = on_board.clone();
    bridged.is_bridgeable = vec![true];
    bridged.bridged = vec![true];
    bridged.bridged_pin_pairs = vec![vec![pair]];
    bridged.active_bridge_idx = vec![0];
    let c_bridge = cost(&bridged, &circuit, &board, &[], &w);

    // Bridged cost 应远小于 OnBoard (启发式选中两孔都在板上 + 有 rail, MST = 0)
    assert!(
        c_bridge < c_on,
        "Bridged 走启发式合法位应比 OnBoard 跨 gap OOB 便宜: on={c_on} bridge={c_bridge}"
    );
}

/// 验证 bridged 元件的 body bbox 参与碰撞检查:
/// 一个 2-pin 电阻的启发式把 power 落在 top positive rail (col=0, y=-3),
/// signal 落在 main rail (col=0, y=0) (R90 旋转)。body 走 col 0 rows -3..0。
/// 另外一个 1-pin 元件放在 col 0, row 0 (在 bridged body 上), 成本应包含 bbox 碰撞。
/// 同一个 1-pin 元件放在 col 5, row 0 (避开 body), 成本应不含 bbox 碰撞。
#[test]
fn cost_bridged_body_bbox_blocks_on_board_components() {
    use crate::circuit::{Footprint, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    // 1 个 2-pin 水平电阻 (Δ=4)
    let fp_r = Footprint {
        id: FootprintId(0),
        name: "R".into(),
        pins: vec![
            PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            },
            PhysicalPin {
                name: "2".into(),
                offset: Position { x: 4, y: 0 },
            },
        ],
    };
    // 1 个 1-pin 元件 (后面 跟 resistor 独立)
    let fp_x = Footprint {
        id: FootprintId(1),
        name: "X".into(),
        pins: vec![PhysicalPin {
            name: "1".into(),
            offset: Position { x: 0, y: 0 },
        }],
    };
    let circuit = crate::circuit::Circuit {
        components: vec![
            Component {
                id: ComponentId(0), // R, bridgeable
                ref_: "R1".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0), PinId(1)],
                footprint: Some(FootprintId(0)),
                bridgeable: true,
            },
            Component {
                id: ComponentId(1), // X, 非 bridgeable
                ref_: "X1".into(),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(2)],
                footprint: Some(FootprintId(1)),
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
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(1)),
            },
            Pin {
                id: PinId(2),
                component: ComponentId(1),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(1)),
            },
        ],
        nets: vec![
            crate::circuit::Net {
                id: NetId(0),
                name: "P".into(),
                pins: vec![PinId(0)],
            },
            crate::circuit::Net {
                id: NetId(1),
                name: "S".into(),
                pins: vec![PinId(1), PinId(2)],
            },
        ],
        footprints: vec![fp_r, fp_x],
    };
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)),
    });

    // 拿启发式生成的 R1 bridged pair
    let pair = propose_bridged_pair(
        &circuit.components[0],
        &circuit,
        &board,
        &[NetId(0), NetId(1)],
    )
    .expect("启发式应返 pair");

    let placed = Placement::Bridged { pin_holes: pair }
        .apply(
            &circuit.components[0],
            &circuit.footprints[0],
            &board,
            &circuit.pins,
        )
        .expect("候选应可应用");
    let pin_holes: std::collections::HashSet<_> =
        placed.pin_holes.iter().map(|pin| pin.hole).collect();
    let overlap_pos = placed
        .bbox
        .expect("Bridged body 应有 bbox")
        .iter_cells()
        .find(|position| {
            position.y >= 0
                && board
                    .at(position.x, position.y)
                    .is_some_and(|hole| !pin_holes.contains(&hole))
        })
        .expect("bridge body 应覆盖至少一个 main-board 非 pin 孔");
    let clear_x = (overlap_pos.x + 10).min(board.cols() as i32 - 1);

    // X1 放到实际 bridged body cell 上。
    let state_overlap = SAState {
        r90_only: vec![false; 4],
        y_locked: vec![None; 4],
        placeable: vec![ComponentId(0), ComponentId(1)],
        is_bridgeable: vec![true, false],
        bridged: vec![true, false],
        bridged_pin_pairs: vec![vec![pair], Vec::new()],
        active_bridge_idx: vec![0, 0],
        x: vec![0, overlap_pos.x],
        y: vec![0, overlap_pos.y],
        rotation: vec![Rotation::R0, Rotation::R0],
    };

    // X1 向右移开 bridged body。
    let state_clear = SAState {
        r90_only: vec![false; 4],
        y_locked: vec![None; 4],
        placeable: vec![ComponentId(0), ComponentId(1)],
        is_bridgeable: vec![true, false],
        bridged: vec![true, false],
        bridged_pin_pairs: vec![vec![pair], Vec::new()],
        active_bridge_idx: vec![0, 0],
        x: vec![0, clear_x],
        y: vec![0, overlap_pos.y],
        rotation: vec![Rotation::R0, Rotation::R0],
    };

    let w = Weights::default();
    let c_overlap = cost(&state_overlap, &circuit, &board, &[], &w);
    let c_clear = cost(&state_clear, &circuit, &board, &[], &w);

    // 重叠的 cost 应比不重叠的高, 高出部分 ≈ bbox 碰撞 (100 per cell)
    let delta = c_overlap - c_clear;
    assert!(
        delta > 1.0,
        "X1 摆 bridged body 上应比避开贵, 但 delta = {delta} (c_overlap={c_overlap}, c_clear={c_clear})"
    );
}

// ============================================================
//  populate_bridgeable_info: top-rail tiebreaker
// ============================================================

/// populate_bridgeable_info 的 cache 排序 tiebreaker: 同样 signal 距离
/// 下, power pin 在 top rail (y < 0) 的 pair 排在前面。借此让"靠上"生效。
#[test]
fn populate_bridgeable_info_top_rail_tiebreaker() {
    use crate::circuit::{Footprint, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    let fp = Footprint {
        id: FootprintId(0),
        name: "R".into(),
        pins: vec![
            PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            },
            PhysicalPin {
                name: "2".into(),
                offset: Position { x: 4, y: 0 },
            },
        ],
    };
    // 构造一个 2 pin + bridgeable 元件, 把多个元件 控在一个中间位置让
    // net center 独立, 以便 cache 排序能 fill 在几个不同 rail row 位置。
    let mut comps: Vec<Component> = Vec::new();
    let mut pins: Vec<Pin> = Vec::new();
    // bridgeable 元件
    comps.push(Component {
        id: ComponentId(0),
        ref_: "R1".into(),
        kind: "R".into(),
        value: None,
        pins: vec![PinId(0), PinId(1)],
        footprint: Some(FootprintId(0)),
        bridgeable: true,
    });
    pins.push(Pin {
        id: PinId(0),
        component: ComponentId(0),
        num: "1".into(),
        pinfunction: None,
        physical_pin_index: 0,
        net: Some(NetId(0)),
    });
    pins.push(Pin {
        id: PinId(1),
        component: ComponentId(0),
        num: "2".into(),
        pinfunction: None,
        physical_pin_index: 1,
        net: Some(NetId(1)),
    });
    // 构造大量非 bridgeable 1-pin 元件 阻隔主区的位置以免从信号 net 的
    // center 变化走样的测试; 这里我们 4 个, 都连同一个 power net 上
    // (只是补充信号 net的位置广).
    for i in 1..=4usize {
        comps.push(Component {
            id: ComponentId(i),
            ref_: format!("X{i}"),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(2 + i - 1)],
            footprint: Some(FootprintId(i)),
            bridgeable: false,
        });
        pins.push(Pin {
            id: PinId(2 + i - 1),
            component: ComponentId(i),
            num: "1".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: Some(NetId(1)),
        });
    }

    // footprints 列表
    let mut fps = vec![fp];
    for i in 1..=4usize {
        fps.push(Footprint {
            id: FootprintId(i),
            name: "X".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        });
    }

    let circuit = Circuit {
        components: comps,
        pins,
        nets: vec![
            Net {
                id: NetId(0),
                name: "+12V".into(),
                pins: vec![PinId(0)],
            },
            Net {
                id: NetId(1),
                name: "SIG".into(),
                pins: vec![PinId(1), PinId(2), PinId(3), PinId(4), PinId(5)],
            },
        ],
        footprints: fps,
    };
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)), // SIG 也在 power 的 net_ids 里 -> bridgeable 启发式返 2 边的 rail
    });

    // 从 greedy 造 state, populate_bridgeable_info
    let placeable: Vec<ComponentId> = (0..circuit.components.len()).map(ComponentId).collect();
    let mut state = SAState::from_greedy(
        placeable,
        &circuit,
        &board,
        &crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        },
        &crate::layout::problem::AnnealProblem::default(),
    );
    populate_bridgeable_info(&mut state, &circuit, &board, &[NetId(0), NetId(1)]);

    // 验证 cache 不为空 且 cache[0] 的 power_pin 在 top rail (y < 0)
    assert!(!state.bridged_pin_pairs[0].is_empty(), "cache 应非空");
    let cache0 = state.bridged_pin_pairs[0][0];
    let power_y = board.hole(cache0[0].0).position.y;
    assert!(
        power_y < 0,
        "cache[0] 必须在 top rail (y < 0), 实际 y = {power_y}"
    );

    // 进一步验证: 如果 cache 里有多个同 signal 距离的 pair,
    // 其中 top rail 的应排在 bottom rail 的前面。
    // 我们检查前 3 个 cache 项, 看是否有任何一对中 "后面的是 bottom 且
    // 前面的是 top" — 这种情况 tiebreaker 生效。
    let mut saw_top_before_bottom = false;
    for w in state.bridged_pin_pairs[0].windows(2) {
        let prev_top = board.hole(w[0][0].0).position.y < 0;
        let next_top = board.hole(w[1][0].0).position.y < 0;
        if prev_top && !next_top {
            saw_top_before_bottom = true;
            break;
        }
    }
    // 这不是 strict 要求 (可能是都 top 或 都 bottom), 但只要 cache 里有
    // top 和 bottom 混合, tiebreaker 应让 top 排在前面。
    let has_mix = state.bridged_pin_pairs[0].iter().any(|pair| {
        let y = board.hole(pair[0].0).position.y;
        y < 0
    }) && state.bridged_pin_pairs[0].iter().any(|pair| {
        let y = board.hole(pair[0].0).position.y;
        y > 7 // bottom rail y >= 14
    });
    if has_mix {
        assert!(
            saw_top_before_bottom,
            "cache 中若 top + bottom 混合, top 应排在前面"
        );
    }
}

// ============================================================
//  init_bridgeable_to_bridged: aggressive bridge default
// ============================================================

/// 验证 `init_bridgeable_to_bridged` 把所有 is_bridgeable 元件默认到 bridged。
#[test]
fn init_bridgeable_to_bridged_flips_all() {
    // 多个 bridgeable 元件, 都需要 bridge。
    let (circuit, board) = bridgeable_two_pin_circuit();
    let placeable = bridgeable_placeables(&circuit);
    let mut state = SAState::from_greedy(
        placeable.clone(),
        &circuit,
        &board,
        &crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        },
        &crate::layout::problem::AnnealProblem::default(),
    );
    populate_bridgeable_info(&mut state, &circuit, &board, &[NetId(0), NetId(1)]);

    // 调用前: bridged 全 false
    for i in 0..placeable.len() {
        assert!(
            !state.bridged[i],
            "init 调用前 state.bridged[{i}] 应 = false"
        );
    }

    let ctx = SAContext::new(&circuit, &placeable);
    let mut buf = CostBuf::new(circuit.nets().len(), board.num_rails(), board.main_rows());
    init_bridgeable_to_bridged(
        &mut state,
        &circuit,
        &board,
        &[],
        &Weights::default(),
        &ctx,
        &mut buf,
    );

    // 调用后: 所有 is_bridgeable=true 的元件都应 bridged
    for i in 0..placeable.len() {
        if state.is_bridgeable[i] {
            assert!(state.bridged[i], "is_bridgeable[{i}] 元件应被翻成 bridged");
        }
    }
}

/// 验证 `init_bridgeable_to_bridged` 挑的是 cache 里 cost 最低的 pair。
#[test]
fn init_bridgeable_to_bridged_picks_lowest_cost_pair() {
    let (circuit, board) = bridgeable_two_pin_circuit();
    let placeable = bridgeable_placeables(&circuit);
    let mut state = SAState::from_greedy(
        placeable.clone(),
        &circuit,
        &board,
        &crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        },
        &crate::layout::problem::AnnealProblem::default(),
    );
    populate_bridgeable_info(&mut state, &circuit, &board, &[NetId(0), NetId(1)]);

    let ctx = SAContext::new(&circuit, &placeable);
    let mut buf = CostBuf::new(circuit.nets().len(), board.num_rails(), board.main_rows());
    let weights = Weights::default();
    init_bridgeable_to_bridged(&mut state, &circuit, &board, &[], &weights, &ctx, &mut buf);

    // 对每个 bridgeable 验证 active_bridge_idx 对应的 cache pair 成本
    // 不超过 cache 里任何其他 pair 的成本。
    for i in 0..placeable.len() {
        if !state.is_bridgeable[i] || state.bridged_pin_pairs[i].is_empty() {
            continue;
        }
        let active_idx = state.active_bridge_idx[i];
        let mut min_cost_idx = active_idx;
        let mut min_cost = f64::INFINITY;
        for j in 0..state.bridged_pin_pairs[i].len() {
            state.active_bridge_idx[i] = j;
            let c = cost_fast(&state, &circuit, &board, &[], &weights, &ctx, &mut buf);
            if c < min_cost {
                min_cost = c;
                min_cost_idx = j;
            }
        }
        assert_eq!(
            active_idx, min_cost_idx,
            "init_bridgeable_to_bridged 选中的 active_bridge_idx = {active_idx} \
                 但 cache 里 cost 最低的下标是 {min_cost_idx}"
        );
        // 验证完后复原 关键状态。
        state.active_bridge_idx[i] = active_idx;
    }
}

/// 用于 init_bridgeable_to_bridged 测试的最小 fixture: 1 个 bridgeable R
/// + 1 个带 SIGNAL net pin 的 1-pin 元件 (SIGNAL net 上多个 pin, 让 net center 可算)。
fn bridgeable_two_pin_circuit() -> (Circuit, Breadboard) {
    use crate::circuit::{Footprint, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    let fp_r = Footprint {
        id: FootprintId(0),
        name: "R".into(),
        pins: vec![
            PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            },
            PhysicalPin {
                name: "2".into(),
                offset: Position { x: 4, y: 0 },
            },
        ],
    };
    let fp_x = Footprint {
        id: FootprintId(1),
        name: "X".into(),
        pins: vec![PhysicalPin {
            name: "1".into(),
            offset: Position { x: 0, y: 0 },
        }],
    };
    let circuit = Circuit {
        components: vec![
            Component {
                id: ComponentId(0),
                ref_: "R1".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0), PinId(1)],
                footprint: Some(FootprintId(0)),
                bridgeable: true,
            },
            Component {
                id: ComponentId(1),
                ref_: "X1".into(),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(2)],
                footprint: Some(FootprintId(1)),
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
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(1)),
            },
            Pin {
                id: PinId(2),
                component: ComponentId(1),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(1)),
            },
        ],
        nets: vec![
            Net {
                id: NetId(0),
                name: "+12V".into(),
                pins: vec![PinId(0)],
            },
            Net {
                id: NetId(1),
                name: "SIG".into(),
                pins: vec![PinId(1), PinId(2)],
            },
        ],
        footprints: vec![fp_r, fp_x],
    };
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)),
    });
    (circuit, board)
}

fn bridgeable_placeables(circuit: &Circuit) -> Vec<ComponentId> {
    circuit
        .components
        .iter()
        .filter_map(|c| c.footprint.map(|_| c.id))
        .collect()
}
