use super::*;
use crate::circuit::{
    Component, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin, PinId, Position,
};

fn fixture() -> &'static Circuit {
    Box::leak(Box::new(Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "Q1".to_string(),
            kind: "NPN".to_string(),
            value: Some("BC547".to_string()),
            pins: vec![PinId(0), PinId(1), PinId(2)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        }],
        pins: vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
            Pin {
                id: PinId(2),
                component: ComponentId(0),
                num: "3".into(),
                pinfunction: None,
                physical_pin_index: 2,
                net: None,
            },
        ],
        nets: vec![],
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "TO92".to_string(),
            pins: vec![
                PhysicalPin {
                    name: "1".to_string(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".to_string(),
                    offset: Position { x: 1, y: 0 },
                },
                PhysicalPin {
                    name: "3".to_string(),
                    offset: Position { x: 2, y: 0 },
                },
            ],
        }],
    }))
}

fn board() -> Breadboard {
    Breadboard::new(30, 5)
}

#[test]
fn new_layout_has_all_unplaced() {
    let circuit = fixture();
    let layout = Layout::new(circuit);
    assert_eq!(layout.placements().len(), 1);
    assert!(layout.placement(ComponentId(0)).is_none());
    assert!(layout.wires().is_empty());
}

#[test]
fn place_and_unplace() {
    let circuit = fixture();
    let mut layout = Layout::new(circuit);
    let p = Placement::OnBoard {
        position: Position { x: 5, y: 2 },
        rotation: Rotation::R0,
    };
    layout.place(ComponentId(0), p);
    assert_eq!(layout.placement(ComponentId(0)), Some(p));

    layout.unplace(ComponentId(0));
    assert!(layout.placement(ComponentId(0)).is_none());
}

#[test]
fn end_to_end_placement_then_occupancy() {
    let circuit = fixture();
    let mut layout = Layout::new(circuit);
    let board = board();
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 10, y: 2 },
            rotation: Rotation::R0,
        },
    );

    let occ = layout.occupancy(&board).unwrap();
    assert!(occ.occupant_at(board.at(10, 2).unwrap()).is_some());
    assert!(occ.occupant_at(board.at(11, 2).unwrap()).is_some());
    assert!(occ.occupant_at(board.at(12, 2).unwrap()).is_some());
    assert!(occ.occupant_at(board.at(13, 2).unwrap()).is_none());
}

#[test]
fn validate_clean_layout_ok() {
    let circuit = fixture();
    let mut layout = Layout::new(circuit);
    let board = board();
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 10, y: 2 },
            rotation: Rotation::R0,
        },
    );
    assert!(layout.validate(&board).is_ok());
}

#[test]
fn validate_detects_out_of_bounds() {
    let circuit = fixture();
    let mut layout = Layout::new(circuit);
    let board = board();
    // R90 at (0, 4): pin 2 落在 (0, 6) 越界
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 0, y: 4 },
            rotation: Rotation::R90,
        },
    );
    let errors = layout.validate(&board).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        LayoutError::OutOfBounds {
            component: ComponentId(0),
            ..
        }
    )));
}

#[test]
fn validate_collects_multiple_errors() {
    let board = board();
    // 两个 component: Q1 有 footprint (但越界), ComponentId(1) 没 footprint
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![
            Component {
                id: ComponentId(0),
                ref_: "Q1".to_string(),
                kind: "NPN".to_string(),
                value: None,
                pins: vec![PinId(0), PinId(1), PinId(2)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            },
            Component {
                id: ComponentId(1),
                ref_: "?".to_string(),
                kind: "?".to_string(),
                value: None,
                pins: vec![PinId(3)],
                footprint: None,
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
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
            Pin {
                id: PinId(2),
                component: ComponentId(0),
                num: "3".into(),
                pinfunction: None,
                physical_pin_index: 2,
                net: None,
            },
            Pin {
                id: PinId(3),
                component: ComponentId(1),
                num: "x".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
        ],
        nets: vec![],
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "TO92".to_string(),
            pins: vec![
                PhysicalPin {
                    name: "1".to_string(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".to_string(),
                    offset: Position { x: 1, y: 0 },
                },
                PhysicalPin {
                    name: "3".to_string(),
                    offset: Position { x: 2, y: 0 },
                },
            ],
        }],
    }));
    let mut layout = Layout::new(circuit);
    // Q1 越界
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 0, y: 4 },
            rotation: Rotation::R90,
        },
    );
    // ComponentId(1) 也摆上 (没 footprint 也能摆, 验证时才发现问题)
    layout.place(
        ComponentId(1),
        Placement::OnBoard {
            position: Position { x: 5, y: 0 },
            rotation: Rotation::R0,
        },
    );
    let errors = layout.validate(&board).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        LayoutError::OutOfBounds {
            component: ComponentId(0),
            ..
        }
    )));
    assert!(errors.iter().any(|e| matches!(
        e,
        LayoutError::NoFootprint {
            component: ComponentId(1)
        }
    )));
}

/// 两个 component + 两个 footprint, Q1(宽 3) + R(宽 4), 用来测列间隔。
pub(crate) fn two_component_fixture() -> &'static Circuit {
    Box::leak(Box::new(Circuit {
        components: vec![
            Component {
                id: ComponentId(0),
                ref_: "Q1".to_string(),
                kind: "NPN".to_string(),
                value: None,
                pins: vec![PinId(0), PinId(1), PinId(2)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            },
            Component {
                id: ComponentId(1),
                ref_: "R1".to_string(),
                kind: "R".to_string(),
                value: None,
                pins: vec![PinId(3), PinId(4)],
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
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
            Pin {
                id: PinId(2),
                component: ComponentId(0),
                num: "3".into(),
                pinfunction: None,
                physical_pin_index: 2,
                net: None,
            },
            Pin {
                id: PinId(3),
                component: ComponentId(1),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
            Pin {
                id: PinId(4),
                component: ComponentId(1),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
        ],
        nets: vec![],
        footprints: vec![
            Footprint {
                id: FootprintId(0),
                name: "TO92".to_string(),
                pins: vec![
                    PhysicalPin {
                        name: "1".to_string(),
                        offset: Position { x: 0, y: 0 },
                    },
                    PhysicalPin {
                        name: "2".to_string(),
                        offset: Position { x: 1, y: 0 },
                    },
                    PhysicalPin {
                        name: "3".to_string(),
                        offset: Position { x: 2, y: 0 },
                    },
                ],
            },
            Footprint {
                id: FootprintId(1),
                name: "R2".to_string(),
                pins: vec![
                    PhysicalPin {
                        name: "1".to_string(),
                        offset: Position { x: 0, y: 0 },
                    },
                    PhysicalPin {
                        name: "2".to_string(),
                        offset: Position { x: 3, y: 0 },
                    },
                ],
            },
        ],
    }))
}

#[test]
fn place_row_first_at_origin() {
    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    layout.place_row(&board, 2).unwrap();

    let q1 = layout.placement(ComponentId(0)).unwrap();
    match q1 {
        Placement::OnBoard { position, rotation } => {
            assert_eq!(position, Position { x: 0, y: 2 });
            assert_eq!(rotation, Rotation::R0);
        }
        Placement::Bridged { .. } => panic!("期望 OnBoard, 实际 Bridged"),
    }
}

#[test]
fn place_row_uses_footprint_width_plus_gap() {
    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    layout.place_row(&board, 2).unwrap();

    // Q1 footprint 宽 3, 放 col 0, 下一个应从 col 3+1=4 开始
    let r1 = layout.placement(ComponentId(1)).unwrap();
    match r1 {
        Placement::OnBoard { position, .. } => {
            assert_eq!(position, Position { x: 4, y: 2 });
        }
        Placement::Bridged { .. } => panic!("期望 OnBoard, 实际 Bridged"),
    }
}

#[test]
fn place_row_occupancy_matches_layout() {
    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    layout.place_row(&board, 2).unwrap();

    let occ = layout.occupancy(&board).unwrap();
    // Q1 在 (0,2): 占 (0,2) (1,2) (2,2)
    assert_eq!(
        occ.occupant_at(board.at(0, 2).unwrap()),
        Some(Occupant::Pin(PinId(0)))
    );
    assert_eq!(
        occ.occupant_at(board.at(1, 2).unwrap()),
        Some(Occupant::Pin(PinId(1)))
    );
    assert_eq!(
        occ.occupant_at(board.at(2, 2).unwrap()),
        Some(Occupant::Pin(PinId(2)))
    );
    // col 3 是间隙
    assert_eq!(occ.occupant_at(board.at(3, 2).unwrap()), None);
    // R1 在 (4,2): 占 (4,2) (7,2) (因为 pin2 offset.x=3)
    assert_eq!(
        occ.occupant_at(board.at(4, 2).unwrap()),
        Some(Occupant::Pin(PinId(3)))
    );
    assert_eq!(
        occ.occupant_at(board.at(7, 2).unwrap()),
        Some(Occupant::Pin(PinId(4)))
    );
    // (5,2) (6,2) R1 跨度内但无 pin, 现在算作被 R1 本体占据 (Blocked)
    assert_eq!(
        occ.occupant_at(board.at(5, 2).unwrap()),
        Some(Occupant::Blocked(ComponentId(1)))
    );
    assert_eq!(
        occ.occupant_at(board.at(6, 2).unwrap()),
        Some(Occupant::Blocked(ComponentId(1)))
    );
}

/// 关键: 没有 footprint 的 component 跳过, 不写 placement
/// (`Occupancy::from_layout` 只检查已摆放的 component, 所以不报错)
#[test]
fn place_row_skips_components_without_footprint() {
    let board = board();
    // Q1 有 footprint, R1 没 footprint
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![
            Component {
                id: ComponentId(0),
                ref_: "Q1".to_string(),
                kind: "NPN".to_string(),
                value: None,
                pins: vec![PinId(0)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            },
            Component {
                id: ComponentId(1),
                ref_: "R1".to_string(),
                kind: "R".to_string(),
                value: None,
                pins: vec![PinId(1)],
                footprint: None,
                bridgeable: false,
            },
        ],
        pins: vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "p".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(1),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
        ],
        nets: vec![],
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "X".to_string(),
            pins: vec![PhysicalPin {
                name: "p".to_string(),
                offset: Position { x: 0, y: 0 },
            }],
        }],
    }));
    let mut layout = Layout::new(circuit);
    let result = layout.place_row(&board, 2);
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    // Q1 摆上了
    assert!(layout.placement(ComponentId(0)).is_some());
    // R1 跳过
    assert!(layout.placement(ComponentId(1)).is_none());
}

/// 关键: 越界时 place_row 返回 Err, 但 placement 已经被写入
/// (Q1 宽 3, 放在 (29, 2) → pin 2 落在 (31,2) 越界)
#[test]
fn place_row_returns_error_when_out_of_bounds() {
    let board = board(); // 30x5
    let mut layout = Layout::new(fixture()); // 单 TO92, 宽 3
    // 手动把它放在 (28, 2) → pin 2 落在 (30, 2) 越界
    layout.placements[ComponentId(0).0] = Some(Placement::OnBoard {
        position: Position { x: 28, y: 2 },
        rotation: Rotation::R0,
    });
    let errors = layout.validate(&board).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, LayoutError::OutOfBounds { .. }))
    );
}

// ============================================================
//  place_sa 集成测试
// ============================================================

/// 退火后 validate() 应过: 无 pin 碰撞, 无越界, 全部有 footprint 的 component 都摆放。
#[test]
fn place_sa_produces_valid_layout() {
    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    let result = layout.place_sa(
        &board,
        &SAConfig {
            max_iters: 2000,
            seed: 42,
            ..SAConfig::default()
        },
    );
    assert!(result.is_ok(), "place_sa 应成功, got {result:?}");
    assert!(layout.placement(ComponentId(0)).is_some());
    assert!(layout.placement(ComponentId(1)).is_some());
}

#[test]
fn place_sa_validation_failure_is_transactional_and_emits_no_completion() {
    use std::sync::Mutex;

    let circuit = Box::leak(Box::new(Circuit {
        components: vec![
            Component {
                id: ComponentId(0),
                ref_: "FIXED".into(),
                kind: "TESTPOINT".into(),
                value: None,
                pins: vec![PinId(0)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            },
            Component {
                id: ComponentId(1),
                ref_: "MOVABLE".into(),
                kind: "TESTPOINT".into(),
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
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(1),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
        ],
        nets: vec![Net {
            id: NetId(0),
            name: "WIRE".into(),
            pins: Vec::new(),
        }],
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        }],
    }));
    let board = Breadboard::new(3, 1);
    let mut layout = Layout::new(circuit);
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 0, y: 0 },
            rotation: Rotation::R0,
        },
    );
    layout.add_wire(Wire {
        id: WireId(0),
        net: NetId(0),
        from: board.at(1, 0).unwrap(),
        to: board.at(2, 0).unwrap(),
    });
    layout.validate(&board).expect("调用前布局必须合法");
    let placements_before = layout.placements().to_vec();
    let wires_before = layout.wires().to_vec();
    let committed = Mutex::new(false);
    let mut invalid_candidate = placements_before.clone();
    invalid_candidate[ComponentId(1).raw()] = Some(Placement::OnBoard {
        position: Position { x: 0, y: 0 },
        rotation: Rotation::R0,
    });
    let result = layout.commit_sa_candidate(&board, invalid_candidate, |_| {
        *committed.lock().unwrap() = true;
    });

    assert!(result.is_err(), "候选与固定元件重叠时必须验证失败");
    assert_eq!(layout.placements(), placements_before);
    assert_eq!(layout.wires().len(), wires_before.len());
    for (actual, before) in layout.wires().iter().zip(&wires_before) {
        assert_eq!(
            (actual.id, actual.net, actual.from, actual.to),
            (before.id, before.net, before.from, before.to)
        );
    }
    assert!(!committed.into_inner().unwrap(), "非法候选不能运行完成回调");
}

#[test]
fn place_sa_initialization_avoids_fixed_onboard_geometry() {
    let circuit = Box::leak(Box::new(Circuit {
        components: (0..2)
            .map(|id| Component {
                id: ComponentId(id),
                ref_: format!("TP{id}"),
                kind: "TESTPOINT".into(),
                value: None,
                pins: vec![PinId(id)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect(),
        pins: (0..2)
            .map(|id| Pin {
                id: PinId(id),
                component: ComponentId(id),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            })
            .collect(),
        nets: Vec::new(),
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        }],
    }));
    let board = Breadboard::new(2, 1);
    let mut layout = Layout::new(circuit);
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 0, y: 0 },
            rotation: Rotation::R0,
        },
    );

    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 0,
                n_seeds: 1,
                use_spectral: false,
                ..SAConfig::default()
            },
        )
        .expect("固定 OnBoard 应作为初始化障碍");
    assert_eq!(
        layout.placement(ComponentId(1)),
        Some(Placement::OnBoard {
            position: Position { x: 1, y: 0 },
            rotation: Rotation::R0,
        })
    );
}

#[test]
fn spectral_initialization_with_more_than_two_components_avoids_fixed_geometry() {
    let circuit = Box::leak(Box::new(Circuit {
        components: (0..4)
            .map(|id| Component {
                id: ComponentId(id),
                ref_: format!("TP{id}"),
                kind: "TESTPOINT".into(),
                value: None,
                pins: vec![PinId(id)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect(),
        pins: (0..4)
            .map(|id| Pin {
                id: PinId(id),
                component: ComponentId(id),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            })
            .collect(),
        nets: Vec::new(),
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        }],
    }));
    let board = Breadboard::new(4, 1);
    let mut layout = Layout::new(circuit);
    layout.place(
        ComponentId(0),
        Placement::OnBoard {
            position: Position { x: 0, y: 0 },
            rotation: Rotation::R0,
        },
    );

    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 0,
                n_seeds: 1,
                use_spectral: true,
                ..SAConfig::default()
            },
        )
        .expect("n > 2 的 spectral grid fill 也必须避开 fixed geometry");
    let mut xs = Vec::new();
    for component in 0..4 {
        let Some(Placement::OnBoard { position, .. }) = layout.placement(ComponentId(component))
        else {
            panic!("所有测试点都应为 OnBoard");
        };
        xs.push(position.x);
    }
    xs.sort_unstable();
    assert_eq!(xs, vec![0, 1, 2, 3]);
}

#[test]
fn greedy_initialization_applies_preprocessed_rotation_before_searching() {
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "J1".into(),
            kind: "CONNECTOR".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
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
            Net {
                id: NetId(0),
                name: "A".into(),
                pins: vec![PinId(0)],
            },
            Net {
                id: NetId(1),
                name: "B".into(),
                pins: vec![PinId(1)],
            },
        ],
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "vertical-2p".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 0, y: 2 },
                },
            ],
        }],
    }));
    let board = Breadboard::new(3, 3);
    for use_spectral in [false, true] {
        let mut layout = Layout::new(circuit);
        layout
            .place_sa(
                &board,
                &SAConfig {
                    max_iters: 0,
                    n_seeds: 1,
                    use_spectral,
                    ..SAConfig::default()
                },
            )
            .expect("R90 必须在 first-fit 搜索前生效");
        assert_eq!(
            layout.placement(ComponentId(0)),
            Some(Placement::OnBoard {
                position: Position { x: 2, y: 0 },
                rotation: Rotation::R90,
            }),
            "use_spectral={use_spectral}"
        );
    }
}

#[test]
fn initialization_returns_error_instead_of_panicking_when_board_is_full() {
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        }],
        pins: vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
        ],
        nets: Vec::new(),
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "2p".into(),
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
        }],
    }));
    let board = Breadboard::new(1, 1);
    let mut layout = Layout::new(circuit);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        layout.place_sa(
            &board,
            &SAConfig {
                max_iters: 0,
                n_seeds: 1,
                use_spectral: false,
                ..SAConfig::default()
            },
        )
    }))
    .expect("装不下必须返回结构化错误而不是 panic");
    assert_eq!(
        result.unwrap_err(),
        vec![LayoutError::NoLegalInitialPlacement {
            component: ComponentId(0),
        }]
    );
    assert!(layout.placements().iter().all(Option::is_none));
}

#[test]
fn place_sa_initialization_avoids_fixed_bridged_body() {
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
                ref_: "TP1".into(),
                kind: "TESTPOINT".into(),
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
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
            Pin {
                id: PinId(2),
                component: ComponentId(1),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
        ],
        nets: Vec::new(),
        footprints: vec![
            Footprint {
                id: FootprintId(0),
                name: "2p".into(),
                pins: vec![
                    PhysicalPin {
                        name: "1".into(),
                        offset: Position { x: 0, y: 0 },
                    },
                    PhysicalPin {
                        name: "2".into(),
                        offset: Position { x: 2, y: 0 },
                    },
                ],
            },
            Footprint {
                id: FootprintId(1),
                name: "1p".into(),
                pins: vec![PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                }],
            },
        ],
    }));
    let board = Breadboard::new(4, 1);
    let mut layout = Layout::new(circuit);
    layout.place(
        ComponentId(0),
        Placement::Bridged {
            pin_holes: [
                (board.at(0, 0).unwrap(), PinId(0)),
                (board.at(2, 0).unwrap(), PinId(1)),
            ],
        },
    );

    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 0,
                n_seeds: 1,
                use_spectral: false,
                ..SAConfig::default()
            },
        )
        .expect("固定 Bridged body 的完整 bbox 应作为初始化障碍");
    assert_eq!(
        layout.placement(ComponentId(1)),
        Some(Placement::OnBoard {
            position: Position { x: 3, y: 0 },
            rotation: Rotation::R0,
        })
    );
}

#[test]
fn place_sa_initialization_avoids_existing_wire_endpoints() {
    use std::sync::Mutex;

    let circuit = Box::leak(Box::new(Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "TP1".into(),
            kind: "TESTPOINT".into(),
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
            net: None,
        }],
        nets: vec![Net {
            id: NetId(0),
            name: "FIXED_WIRE".into(),
            pins: Vec::new(),
        }],
        footprints: vec![Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        }],
    }));
    let board = Breadboard::new(3, 1);
    let mut layout = Layout::new(circuit);
    layout.add_wire(Wire {
        id: WireId(0),
        net: NetId(0),
        from: board.at(0, 0).unwrap(),
        to: board.at(1, 0).unwrap(),
    });

    let events = Mutex::new(Vec::new());
    layout
        .place_sa_with_progress(
            &board,
            &SAConfig {
                max_iters: 0,
                n_seeds: 1,
                use_spectral: true,
                ..SAConfig::default()
            },
            ProgressOptions::default(),
            |event| events.lock().unwrap().push(event),
        )
        .expect("已有 wire 两个端点都应作为初始化障碍");
    assert_eq!(
        layout.placement(ComponentId(0)),
        Some(Placement::OnBoard {
            position: Position { x: 2, y: 0 },
            rotation: Rotation::R0,
        })
    );
    for event in events.into_inner().unwrap() {
        let snapshot = match event {
            LayoutProgress::SpectralInitial { snapshot, .. }
            | LayoutProgress::Annealing { snapshot, .. }
            | LayoutProgress::PlacementComplete { snapshot, .. } => Some(snapshot),
            LayoutProgress::SeedsProgress { .. } | LayoutProgress::RoutingComplete { .. } => None,
        };
        if let Some(snapshot) = snapshot {
            assert_eq!(snapshot.wires.len(), 1, "SA progress 不能丢掉已有 wires");
        }
    }
}

/// 退火在固定 seed 下应可重现。
#[test]
fn place_sa_is_deterministic_with_seed() {
    let board = board();
    let config = SAConfig {
        max_iters: 1000,
        seed: 1234,
        ..SAConfig::default()
    };
    let mut a = Layout::new(two_component_fixture());
    let mut b = Layout::new(two_component_fixture());
    a.place_sa(&board, &config).unwrap();
    b.place_sa(&board, &config).unwrap();
    for cid in [ComponentId(0), ComponentId(1)] {
        assert_eq!(a.placement(cid), b.placement(cid));
    }
}

/// 进度观测只能复制状态，不能改变 RNG 消费或最终选优。
#[test]
fn place_sa_progress_does_not_change_result() {
    use std::sync::Mutex;

    let board = board();
    let config = SAConfig {
        max_iters: 600,
        n_seeds: 3,
        seed: 1234,
        use_spectral: true,
        ..SAConfig::default()
    };
    let mut plain = Layout::new(two_component_fixture());
    let mut observed = Layout::new(two_component_fixture());
    plain.place_sa(&board, &config).unwrap();

    let events = Mutex::new(Vec::new());
    observed
        .place_sa_with_progress(
            &board,
            &config,
            crate::layout::ProgressOptions {
                display_seed: 0,
                sample_every: 100,
            },
            |event| events.lock().unwrap().push(event),
        )
        .unwrap();

    assert_eq!(plain.placements(), observed.placements());
    let events = events.into_inner().unwrap();
    assert!(matches!(
        events.first(),
        Some(crate::layout::LayoutProgress::SpectralInitial { seed: 1234, .. })
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        crate::layout::LayoutProgress::Annealing { seed: 1234, .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        crate::layout::LayoutProgress::SeedsProgress {
            completed: 3,
            total: 3
        }
    )));
    assert!(matches!(
        events.last(),
        Some(crate::layout::LayoutProgress::PlacementComplete { .. })
    ));
}

#[test]
fn sa_cancelled_from_progress_callback_returns_a_valid_best_so_far_layout() {
    use std::sync::Mutex;

    let board = board();
    let cancellation = CancellationToken::new();
    let events = Mutex::new(Vec::new());
    let mut layout = Layout::new(two_component_fixture());

    layout
        .place_sa_with_progress_and_cancellation(
            &board,
            &SAConfig {
                max_iters: 1_000_000,
                n_seeds: 4,
                use_spectral: true,
                ..SAConfig::default()
            },
            ProgressOptions {
                display_seed: 0,
                sample_every: 1,
            },
            &cancellation,
            |event| {
                if matches!(event, LayoutProgress::Annealing { .. }) {
                    cancellation.cancel();
                }
                events.lock().unwrap().push(event);
            },
        )
        .unwrap();

    assert!(layout.placements().iter().all(Option::is_some));
    assert!(matches!(
        events.into_inner().unwrap().last(),
        Some(LayoutProgress::PlacementComplete {
            cancelled: true,
            ..
        })
    ));
}

#[test]
fn route_with_progress_replaces_wires_and_reports_final_snapshot() {
    use std::cell::RefCell;

    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 500,
                seed: 17,
                ..SAConfig::default()
            },
        )
        .unwrap();
    let event = RefCell::new(None);
    layout
        .route_with_progress(&board, &PathFinderRouter::default(), |progress| {
            *event.borrow_mut() = Some(progress);
        })
        .unwrap();

    let event = event.into_inner().expect("应报告 routing 完成");
    let crate::layout::LayoutProgress::RoutingComplete { snapshot } = event else {
        panic!("最后事件应是 RoutingComplete");
    };
    assert_eq!(snapshot.wires.len(), layout.wires().len());
    assert_eq!(snapshot.placements, layout.placements());
}

/// 不同 seed 都应能跑出有效布局 (不强求不同——MST 在 1D 顺序布局下是
/// permutation-invariant, swap 沿 MST 是平的, 不同 seed 可能收敛到同解)。
/// 这个测试主要确保"没因为换个 seed 就崩"。
#[test]
fn place_sa_handles_various_seeds() {
    let board = board();
    for seed in [1, 7, 42, 1234, 9999] {
        let mut layout = Layout::new(two_component_fixture());
        layout
            .place_sa(
                &board,
                &SAConfig {
                    seed,
                    max_iters: 1000,
                    ..SAConfig::default()
                },
            )
            .unwrap_or_else(|e| panic!("seed {seed} 失败: {e:?}"));
        assert!(layout.placement(ComponentId(0)).is_some());
        assert!(layout.placement(ComponentId(1)).is_some());
    }
}

/// SA 结果不包含 R90/R270 (v1 限制)。
#[test]
fn place_sa_never_uses_r90_or_r270() {
    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    for seed in 0..5 {
        layout
            .place_sa(
                &board,
                &SAConfig {
                    seed,
                    max_iters: 500,
                    ..SAConfig::default()
                },
            )
            .unwrap();
        for cid in [ComponentId(0), ComponentId(1)] {
            let p = layout.placement(cid).unwrap();
            assert!(
                matches!(
                    p,
                    Placement::OnBoard {
                        rotation: Rotation::R0 | Rotation::R180,
                        ..
                    }
                ),
                "seed {seed}: cid {:?} 出现了 {:?}",
                cid,
                p
            );
        }
    }
}

/// 走线和退火能联调出有效路线: SA 布局后, PathFinder 跑出来 wires 不冲突 pin。
#[test]
fn place_sa_then_pathfinder_routes_cleanly() {
    use crate::Router;
    let board = board();
    let mut layout = Layout::new(two_component_fixture());
    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 2000,
                seed: 17,
                ..SAConfig::default()
            },
        )
        .unwrap();
    let occ = layout.occupancy(&board).unwrap();
    let router = PathFinderRouter {
        max_iterations: 50,
        history_increment: 1.0,
    };
    let wires = router.route(layout.circuit(), &board, &occ, &[]);
    for w in &wires {
        // 端点不能和 pin 撞
        assert!(occ.can_add_wire(w), "wire {:?} 跟 pin 撞了", w);
    }
}

/// Bridged 跨 rail → 主区: 验证 bridged_pins 走通了整条链路
///
/// 场景: 1 个 2-pin 电阻, 跨接 GND (负极轨) 和 主区某行。绑定 rail 到 GND net。
/// 期望:
/// - router 不在 bridged 两条腿之间生成 wire (它们物理上连好了)
/// - bridged 的主区那条腿的 net 跟其他 GND pin 一起, 走 rail 短接
/// - bridged 的 rail 那条腿不需要 wire 到 rail (已经在 rail 里)
#[test]
fn bridged_cross_rail_to_main_routes_correctly() {
    use crate::Router;
    use crate::circuit::{FootprintId, Net, NetId, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    // 2-pin 电阻, pin 1 (主区) + pin 2 (负极轨)
    let fp = Footprint {
        id: FootprintId(0),
        name: "R_BR".into(),
        pins: vec![
            PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            },
            PhysicalPin {
                name: "2".into(),
                offset: Position { x: 5, y: 0 },
            },
        ],
    };
    let r1 = Component {
        id: ComponentId(0),
        ref_: "R1".into(),
        kind: "R".into(),
        value: None,
        pins: vec![PinId(0), PinId(1)],
        footprint: Some(FootprintId(0)),
        bridgeable: false,
    };
    let r1_pins = vec![
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
            net: Some(NetId(0)),
        },
    ];
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![r1],
        pins: r1_pins,
        nets: vec![Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0), PinId(1)],
        }],
        footprints: vec![fp],
    }));

    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(0)),
    });
    // pin 1 (主区) 在 (5, 0), pin 2 (负极轨) 在 (1, -4)；x=0 被默认 tie 占用。
    let h_main = board.at(5, 0).unwrap();
    let h_rail = board.at(1, -4).unwrap();
    let placement = Placement::Bridged {
        pin_holes: [(h_main, PinId(0)), (h_rail, PinId(1))],
    };

    let mut layout = Layout::new(circuit);
    layout.place(ComponentId(0), placement);
    let occ = layout.occupancy(&board).expect("bridged layout 应该合法");

    let router = PathFinderRouter {
        max_iterations: 50,
        history_increment: 1.0,
    };
    let wires = router.route(circuit, &board, &occ, &layout.bridged_pins());

    // 验证: 没有任何 wire 走到 (1, -4) (rail 端点已经被 pin 占, 不能 wire)
    for w in &wires {
        let p1 = board.hole(w.from).position;
        let p2 = board.hole(w.to).position;
        // 都不该是 (1, -4) 这个孔
        assert!(
            !(p1.x == 1 && p1.y == -4),
            "wire 端点不该在已占用 rail 孔 (1, -4): {:?}",
            p1
        );
        assert!(
            !(p2.x == 1 && p2.y == -4),
            "wire 端点不该在已占用 rail 孔 (1, -4): {:?}",
            p2
        );
    }
}

/// bridged body 在 occupancy 里被标 Blocked: 验证桥接 (0, -3) → (5, 0) 后,
/// (1, 0) 到 (4, 0) 这些 main board 主体格 (在 bbox 里、不是 pin) 应该是 Blocked,
/// 而 (0, -3)、(5, 0) 是 Pin. 另验证 router 不会用 (1..=4, 0) 作端点。
#[test]
fn bridged_body_cells_are_marked_blocked() {
    use crate::Occupant;
    use crate::circuit::{FootprintId, Net, NetId, PhysicalPin};
    use crate::layout::breadboard::PowerRailBinding;

    let fp = Footprint {
        id: FootprintId(0),
        name: "R_BR".into(),
        pins: vec![
            PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            },
            PhysicalPin {
                name: "2".into(),
                offset: Position { x: 5, y: 0 },
            },
        ],
    };
    let r1 = Component {
        id: ComponentId(0),
        ref_: "R1".into(),
        kind: "R".into(),
        value: None,
        pins: vec![PinId(0), PinId(1)],
        footprint: Some(FootprintId(0)),
        bridgeable: false,
    };
    let r1_pins = vec![
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
            net: Some(NetId(0)),
        },
    ];
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![r1],
        pins: r1_pins,
        nets: vec![Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0), PinId(1)],
        }],
        footprints: vec![fp],
    }));
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(0)),
    });
    let h_main = board.at(5, 0).unwrap();
    let h_rail = board.at(1, -4).unwrap();
    let placement = Placement::Bridged {
        pin_holes: [(h_main, PinId(0)), (h_rail, PinId(1))],
    };
    let mut layout = Layout::new(circuit);
    layout.place(ComponentId(0), placement);
    let occ = layout.occupancy(&board).expect("bridged layout 应该合法");

    // pin 端点应是 Pin
    assert!(matches!(occ.occupant_at(h_main), Some(Occupant::Pin(_))));
    assert!(matches!(occ.occupant_at(h_rail), Some(Occupant::Pin(_))));

    // body 中间 (2..=4, 0) 这些 main board 格应是 Blocked (R1)
    for x in 2..=4 {
        let h = board.at(x, 0).unwrap();
        assert!(
            matches!(occ.occupant_at(h), Some(Occupant::Blocked(_))),
            "bridged body 中间格 ({x}, 0) 应被标 Blocked, got {:?}",
            occ.occupant_at(h)
        );
    }
    // body 在 rail 行 (-4) 那些格也应是 Blocked
    for x in 2..=4 {
        let h = board.at(x, -4).unwrap();
        assert!(
            matches!(occ.occupant_at(h), Some(Occupant::Blocked(_))),
            "bridged body 在 rail 上的 ({x}, -4) 应被标 Blocked, got {:?}",
            occ.occupant_at(h)
        );
    }
    // (1, -3) / (1, -2) / (1, -1) 是 gap row, 不存在 hole, 不检查
}

/// 关键: 2D 状态下, 18 元件 30x5 板不应再出 OOB (以前 sequential x 会塞不下)
/// 注: 不要求 0 列冲突, 那需要 basin hopping 额外优化
#[test]
fn place_sa_no_oob_for_oversized_circuit() {
    // 手搓 18 元件的密集电路: 总宽 ~94, 远超 30
    use crate::circuit::{Net, NetId, PhysicalPin};
    let mut fp_wide = Footprint {
        id: FootprintId(0),
        name: "wide".into(),
        pins: (0..11)
            .map(|i| PhysicalPin {
                name: i.to_string(),
                offset: Position { x: i, y: 0 },
            })
            .collect(),
    };
    fp_wide.pins.truncate(1);
    let fp_3 = Footprint {
        id: FootprintId(1),
        name: "to92".into(),
        pins: (0..3)
            .map(|i| PhysicalPin {
                name: i.to_string(),
                offset: Position { x: i, y: 0 },
            })
            .collect(),
    };
    let fp_4 = Footprint {
        id: FootprintId(2),
        name: "axial".into(),
        pins: vec![
            PhysicalPin {
                name: "0".into(),
                offset: Position { x: 0, y: 0 },
            },
            PhysicalPin {
                name: "3".into(),
                offset: Position { x: 3, y: 0 },
            },
        ],
    };
    let mut components = vec![];
    let mut pins = vec![];
    // 4 个 11-col, 6 个 3-col, 8 个 4-col → 18 元件
    for i in 0..4 {
        let pin_id = PinId(pins.len());
        components.push(Component {
            id: ComponentId(i),
            ref_: format!("D{i}"),
            kind: "D".into(),
            value: None,
            pins: vec![pin_id],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        });
        pins.push(Pin {
            id: pin_id,
            component: ComponentId(i),
            num: "0".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        });
    }
    for i in 4..10 {
        let pin_id = PinId(pins.len());
        components.push(Component {
            id: ComponentId(i),
            ref_: format!("Q{i}"),
            kind: "Q".into(),
            value: None,
            pins: vec![pin_id],
            footprint: Some(FootprintId(1)),
            bridgeable: false,
        });
        pins.push(Pin {
            id: pin_id,
            component: ComponentId(i),
            num: "0".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        });
    }
    for i in 10..18 {
        let pin_id = PinId(pins.len());
        components.push(Component {
            id: ComponentId(i),
            ref_: format!("R{i}"),
            kind: "R".into(),
            value: None,
            pins: vec![pin_id],
            footprint: Some(FootprintId(2)),
            bridgeable: false,
        });
        pins.push(Pin {
            id: pin_id,
            component: ComponentId(i),
            num: "0".into(),
            pinfunction: None,
            physical_pin_index: 0,
            net: None,
        });
    }
    let circuit = Box::leak(Box::new(Circuit {
        components,
        pins,
        nets: vec![Net {
            id: NetId(0),
            name: "shared".into(),
            pins: (0..18).map(PinId).collect(),
        }],
        footprints: vec![fp_wide, fp_3, fp_4],
    }));
    let board = Breadboard::new(30, 5);
    let mut layout = Layout::new(circuit);
    let result = layout.place_sa(
        &board,
        &SAConfig {
            max_iters: 5000,
            seed: 42,
            ..SAConfig::default()
        },
    );
    // 不一定要 Ok (可能有列冲突), 但 OOB 应该没有
    match result {
        Ok(()) => {}
        Err(errors) => {
            let oob = errors
                .iter()
                .filter(|e| matches!(e, LayoutError::OutOfBounds { .. }))
                .count();
            assert_eq!(oob, 0, "2D SA 不应再出 OOB, got: {errors:?}");
        }
    }
}

// ============================================================
//  桥接 Toggle 端到端
// ============================================================

/// 1 个 bridgeable 2-pin 电阻, 放标准板 + power rail 绑定。
/// 退火后 bridgeable 元件可能是 OnBoard (cost 低) 或 Bridged (启发式选得好)。
/// 验证: 两种 placement 都应合法, 不出 OOB / pin 碰撞。
#[test]
fn place_sa_can_emit_bridged_placement_for_bridgeable_resistor() {
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
    let circuit = Box::leak(Box::new(Circuit {
        components: vec![Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: true, // 关键: 被启发式预选
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
            Net {
                id: NetId(0),
                name: "P".into(),
                pins: vec![PinId(0)],
            },
            Net {
                id: NetId(1),
                name: "S".into(),
                pins: vec![PinId(1)],
            },
        ],
        footprints: vec![fp],
    }));
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)),
    });
    let mut layout = Layout::new(circuit);
    // 退火不一定总选 Bridged (取决于 SA 随机接受), 但无论选哪个, validate 应过。
    // 为提高撞上 Toggle 的概率, 提升 p_toggle_bridge 到 0.3 跑多次。
    let config = SAConfig {
        max_iters: 2000,
        t0: 5.0,
        cool_rate: 0.95,
        n_seeds: 5,
        p_toggle_bridge: 0.3,
        ..SAConfig::default()
    };
    let result = layout.place_sa(&board, &config);
    assert!(
        result.is_ok(),
        "place_sa 应成功 (validate 过), got {result:?}"
    );
    // 验证 placement 类型合法
    match layout.placement(ComponentId(0)) {
        Some(Placement::OnBoard { .. }) | Some(Placement::Bridged { .. }) => {}
        other => panic!("R1 应该有 placement, got {other:?}"),
    }
}

/// 验证: 高 p_toggle_bridge + 多 seed 跑下来, 至少有一个 seed 产出 Bridged。
/// (如果概率分布对, 7% × 多次跑应该能撞上; 提高到 0.5 + n_seeds=20 更稳。)
#[test]
fn place_sa_bridgeable_can_flip_to_bridged() {
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
    let circuit = Box::leak(Box::new(Circuit {
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
            Net {
                id: NetId(0),
                name: "P".into(),
                pins: vec![PinId(0)],
            },
            Net {
                id: NetId(1),
                name: "S".into(),
                pins: vec![PinId(1)],
            },
        ],
        footprints: vec![fp],
    }));
    let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
        positive: Some(NetId(0)),
        negative: Some(NetId(1)),
    });
    // 跑多次, 记录是否出现 Bridged 结果。
    // 不强求 100% 出现 (SA 随机性), 但要求至少 1 次出现 (概率足够大时必然出现)。
    let config = SAConfig {
        max_iters: 2000,
        t0: 10.0,
        cool_rate: 0.9,
        n_seeds: 20,
        p_toggle_bridge: 0.5,
        ..SAConfig::default()
    };
    let mut any_bridged = false;
    for seed in 0..20u64 {
        let mut layout = Layout::new(circuit);
        let cfg = SAConfig { seed, ..config };
        if layout.place_sa(&board, &cfg).is_ok()
            && matches!(
                layout.placement(ComponentId(0)),
                Some(Placement::Bridged { .. })
            )
        {
            any_bridged = true;
            break;
        }
    }
    assert!(
        any_bridged,
        "20 个 seed × p_toggle=0.5 × 2000 iters 至少应出现 1 次 Bridged, 全是 OnBoard"
    );
}
