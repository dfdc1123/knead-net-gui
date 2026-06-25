//! 孔占用: 每孔至多一个 occupant (面包板孔径物理约束)。
//!
//! 由 [`Occupancy::from_layout`] 从 Layout 派生; layout 不合法时返回 `Err`,
//! 不返回部分 occupancy。把"layout 合法"这个不变量从契约提到 type 层。

use std::collections::HashSet;

use crate::circuit::PinId;

use super::Layout;
use super::LayoutError;
use super::breadboard::{Breadboard, HoleId};
use super::routing::Wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Occupant {
    Pin(PinId),
    /// 线插在此孔。`Wire` 只有 `from` 和 `to` 两个接触点 (没有中间点)。
    Wire(super::routing::WireId),
}

#[derive(Debug)]
pub struct Occupancy {
    /// 下标 == HoleId.0
    at: Vec<Option<Occupant>>,
}

impl Occupancy {
    /// 全部空的 occupancy, 大小跟 board 孔数一致。
    pub fn empty(board: &Breadboard) -> Self {
        Self {
            at: vec![None; board.len()],
        }
    }

    /// 从 layout 派生 occupancy, 同时检查 layout 的合法性。
    ///
    /// **严格**: 任何非法状态 (no footprint / 越界 / pin 碰撞 / wire 冲突 / 列短路) 都返回 `Err`,
    /// 不返回部分 occupancy。错误列表里包含所有发现的问题。
    pub fn from_layout(layout: &Layout, board: &Breadboard) -> Result<Self, Vec<LayoutError>> {
        let mut occ = Self::empty(board);
        let mut errors = Vec::new();
        let mut occupied: HashSet<HoleId> = HashSet::new();
        // 按列收集 endpoint, 用于检查"同列不同 net" 短路。
        let mut by_column: std::collections::BTreeMap<i32, Vec<super::ColumnEndpoint>> =
            std::collections::BTreeMap::new();

        for (idx, placement_opt) in layout.placements().iter().enumerate() {
            let Some(placement) = placement_opt else {
                continue;
            };
            let component = &layout.circuit().components[idx];
            let Some(fid) = component.footprint else {
                errors.push(LayoutError::NoFootprint {
                    component: component.id,
                });
                continue;
            };
            let footprint = &layout.circuit().footprints[fid.0];
            let placed = match placement.apply(component, footprint, board, layout.circuit().pins())
            {
                Ok(p) => p,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            for pin_hole in placed.pin_holes {
                if !occupied.insert(pin_hole.hole) {
                    errors.push(LayoutError::PinCollision {
                        component: component.id,
                        pin: pin_hole.pin,
                        hole: pin_hole.hole,
                    });
                } else {
                    occ.at[pin_hole.hole.0] = Some(Occupant::Pin(pin_hole.pin));
                    // 收集到按列索引, 供后面短路检查
                    let pin = &layout.circuit().pins[pin_hole.pin.0];
                    let col = board.hole(pin_hole.hole).position.x;
                    by_column
                        .entry(col)
                        .or_default()
                        .push(super::ColumnEndpoint::Pin {
                            pin: pin_hole.pin,
                            net: pin.net,
                        });
                }
            }
        }

        for wire in layout.wires() {
            for hole in wire.contacts() {
                if !occupied.insert(hole) {
                    errors.push(LayoutError::WireConflict {
                        wire: wire.id,
                        hole,
                    });
                } else {
                    occ.at[hole.0] = Some(Occupant::Wire(wire.id));
                    let col = board.hole(hole).position.x;
                    by_column
                        .entry(col)
                        .or_default()
                        .push(super::ColumnEndpoint::Wire {
                            wire: wire.id,
                            net: wire.net,
                        });
                }
            }
        }

        // 列短路检查: 任意一列上, 任意两个 endpoint 的 net 不一致 → 报 ColumnConflict。
        // 选第一项作为"基准", 其后只报第一对 (一列报一次避免刷屏)。
        for (col, endpoints) in by_column {
            if endpoints.len() < 2 {
                continue;
            }
            let base_net = match endpoints[0] {
                super::ColumnEndpoint::Pin { net, .. } => net,
                super::ColumnEndpoint::Wire { net, .. } => Some(net),
            };
            for i in 1..endpoints.len() {
                let other_net = match endpoints[i] {
                    super::ColumnEndpoint::Pin { net, .. } => net,
                    super::ColumnEndpoint::Wire { net, .. } => Some(net),
                };
                if other_net != base_net {
                    errors.push(LayoutError::ColumnConflict {
                        column: col,
                        a: endpoints[0],
                        b: endpoints[i],
                    });
                    break;
                }
            }
        }

        if errors.is_empty() {
            Ok(occ)
        } else {
            Err(errors)
        }
    }

    pub fn occupant_at(&self, hole: HoleId) -> Option<Occupant> {
        self.at[hole.0]
    }

    /// 从 layout 构造 occupancy, **忽略所有错误**。 有冲突/OOB 的孔要么不填, 要么
    /// 后到的 pin 覆盖先到的。 主程序在 `validate` 报错时用它来 "尽力跑接线 + 画 SVG"。
    pub fn from_layout_lossy(layout: &Layout, board: &Breadboard) -> Self {
        let mut occ = Self::empty(board);
        for (idx, slot) in layout.placements().iter().enumerate() {
            let Some(placement) = *slot else { continue };
            let component = &layout.circuit().components()[idx];
            let Some(fid) = component.footprint() else {
                continue;
            };
            let footprint = &layout.circuit().footprints()[fid.raw()];
            let Ok(placed) = placement.apply(component, footprint, board, layout.circuit().pins())
            else {
                continue;
            };
            for ph in placed.pin_holes {
                occ.at[ph.hole.0] = Some(Occupant::Pin(ph.pin));
            }
        }
        for w in layout.wires() {
            for h in w.contacts() {
                occ.at[h.0] = Some(Occupant::Wire(w.id));
            }
        }
        occ
    }

    pub fn can_place_pin(&self, hole: HoleId) -> bool {
        self.at[hole.0].is_none()
    }

    pub fn can_add_wire(&self, wire: &Wire) -> bool {
        self.at[wire.from.0].is_none() && self.at[wire.to.0].is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::placement::{Placement, Rotation};
    use crate::layout::routing::{Wire, WireId};
    use crate::layout::tests::two_component_fixture;

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn empty_occupancy_is_all_none() {
        let b = board();
        let occ = Occupancy::empty(&b);
        for hole in b.holes() {
            assert!(occ.occupant_at(hole.id).is_none());
        }
    }

    #[test]
    fn can_place_pin_when_empty() {
        let b = board();
        let occ = Occupancy::empty(&b);
        assert!(occ.can_place_pin(b.at(5, 2).unwrap()));
    }

    /// 单个 placed component, footprint TO92 在 (10, 2) R0 → 占 (10,2)(11,2)(12,2)
    fn placed_q1_fixture() -> &'static crate::circuit::Circuit {
        Box::leak(Box::new(crate::circuit::Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "Q1".to_string(),
                kind: "NPN".to_string(),
                value: None,
                pins: vec![PinId(0), PinId(1), PinId(2)],
                footprint: Some(FootprintId(0)),
            }],
            pins: vec![
                crate::circuit::Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
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

    /// 列冲突反面: 2 个 pin 落在同列不同 row, 但都是 "无 net" → 不报 ColumnConflict
    /// (None 跟 None 视为同一, 两个未连接的 pin 插同列不是电气连接——它们本来就不该连)
    #[test]
    fn column_conflict_no_net_no_conflict() {
        let b = Breadboard::new(30, 5);
        let mut layout = Layout::new(two_component_fixture());
        // two_component_fixture 的所有 pin 都 net: None
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 0, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement {
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R0,
            },
        );
        // Q1.pin1 在 (0,2), R1.pin1 在 (0,4) — 同列不同行
        // 两个都 net=None → 视为同一 "无 net" 状态, 不算冲突
        let result = layout.occupancy(&b);
        assert!(
            result.is_ok(),
            "两个无 net pin 同列不该报冲突, got: {result:?}"
        );
    }

    /// 列冲突: 不同 net 的 pin 同列 → 必报 ColumnConflict
    #[test]
    fn column_conflict_different_nets() {
        let b = Breadboard::new(30, 5);
        // 手搓一个电路: 2 个 1-pin 元件, 不同 net
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
                },
                Component {
                    id: ComponentId(1),
                    ref_: "B".into(),
                    kind: "X".into(),
                    value: None,
                    pins: vec![PinId(1)],
                    footprint: Some(FootprintId(0)),
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
            ],
            nets: vec![
                Net {
                    id: NetId(0),
                    name: "n0".into(),
                    pins: vec![PinId(0)],
                },
                Net {
                    id: NetId(1),
                    name: "n1".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        // A 在 (0, 2), B 在 (0, 4) — 同 col 0, 不同 row, 不同 net
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 0, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement {
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R0,
            },
        );
        let errors = layout.occupancy(&b).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, crate::layout::LayoutError::ColumnConflict { .. })),
            "不同 net 同列应该报 ColumnConflict, got: {errors:?}"
        );
    }

    /// pin 跟 wire 在同列且同 net → 不报 ColumnConflict
    /// (wire 走同 net, 相当于这个 net 在该列多点接出, 面包板本身就该这么连)
    #[test]
    fn column_conflict_pin_vs_wire_same_net_ok() {
        let b = Breadboard::new(30, 5);
        // 手搓 1-pin 元件, 设 net = Some(0)
        let fp = Footprint {
            id: FootprintId(0),
            name: "single".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "A".into(),
                kind: "X".into(),
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
                name: "n0".into(),
                pins: vec![PinId(0)],
            }],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // wire 也走 net 0, 一头在 (5, 0) — 跟 A.pin (5, 2) 同列同 net
        let wire = Wire {
            id: WireId(0),
            net: NetId(0),
            from: b.at(5, 0).unwrap(),
            to: b.at(10, 0).unwrap(),
        };
        layout.add_wire(wire);
        let result = layout.occupancy(&b);
        assert!(
            result.is_ok(),
            "同 net pin + wire 同列不该报冲突: {result:?}"
        );
    }

    /// lossy: 有 column 冲突的 layout, lossy 版应该仍然出 occupancy
    /// (pin 都在不同孔, 都能填进去)
    #[test]
    fn from_layout_lossy_succeeds_with_column_conflicts() {
        let b = Breadboard::new(30, 5);
        // 两个 1-pin 元件, 不同 net, 放同列不同行
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
                },
                Component {
                    id: ComponentId(1),
                    ref_: "B".into(),
                    kind: "X".into(),
                    value: None,
                    pins: vec![PinId(1)],
                    footprint: Some(FootprintId(0)),
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
            ],
            nets: vec![
                Net {
                    id: NetId(0),
                    name: "n0".into(),
                    pins: vec![PinId(0)],
                },
                Net {
                    id: NetId(1),
                    name: "n1".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 0, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement {
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R0,
            },
        );
        // 严格版报 ColumnConflict
        assert!(layout.occupancy(&b).is_err());
        // lossy 版应该成功, 两个 pin 都在
        let occ = Occupancy::from_layout_lossy(&layout, &b);
        let p0 = b.at(0, 2).unwrap();
        let p1 = b.at(0, 4).unwrap();
        assert!(matches!(occ.occupant_at(p0), Some(Occupant::Pin(PinId(0)))));
        assert!(matches!(occ.occupant_at(p1), Some(Occupant::Pin(PinId(1)))));
    }

    #[test]
    fn placed_pin_blocks_hole() {
        let b = board();
        let mut layout = Layout::new(placed_q1_fixture());
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );

        let occ = layout.occupancy(&b).unwrap();
        assert_eq!(
            occ.occupant_at(b.at(10, 2).unwrap()),
            Some(Occupant::Pin(PinId(0)))
        );
        assert_eq!(
            occ.occupant_at(b.at(11, 2).unwrap()),
            Some(Occupant::Pin(PinId(1)))
        );
        assert_eq!(
            occ.occupant_at(b.at(12, 2).unwrap()),
            Some(Occupant::Pin(PinId(2)))
        );

        assert!(!occ.can_place_pin(b.at(10, 2).unwrap()));
        assert!(!occ.can_place_pin(b.at(11, 2).unwrap()));

        assert!(occ.can_place_pin(b.at(13, 2).unwrap()));
        assert!(occ.can_place_pin(b.at(10, 3).unwrap()));
    }

    #[test]
    fn wire_occupies_only_endpoints() {
        let b = board();
        let circuit = Box::leak(Box::new(crate::circuit::Circuit {
            components: vec![],
            pins: vec![],
            nets: vec![],
            footprints: vec![],
        }));
        let mut layout = Layout::new(circuit);
        // 跨列 wire: from (5, 2) → to (10, 2), 桥接两列
        // 只有这两个端点被 wire 占用, 中间 (6, 2)..(9, 2) 都不占用
        let wire = Wire {
            id: super::super::routing::WireId(0),
            net: crate::circuit::NetId(0),
            from: b.at(5, 2).unwrap(),
            to: b.at(10, 2).unwrap(),
        };
        layout.add_wire(wire.clone());

        let occ = layout.occupancy(&b).unwrap();
        assert_eq!(
            occ.occupant_at(b.at(5, 2).unwrap()),
            Some(Occupant::Wire(super::super::routing::WireId(0)))
        );
        assert_eq!(
            occ.occupant_at(b.at(10, 2).unwrap()),
            Some(Occupant::Wire(super::super::routing::WireId(0)))
        );

        // 中间 (6, 2)..(9, 2) 不被 wire 占用
        assert!(occ.can_place_pin(b.at(6, 2).unwrap()));
        assert!(occ.can_place_pin(b.at(7, 2).unwrap()));
        assert!(occ.can_place_pin(b.at(8, 2).unwrap()));
        assert!(occ.can_place_pin(b.at(9, 2).unwrap()));

        assert!(!occ.can_add_wire(&wire));
    }

    /// 关键 bug 修复测试: 两个 component 的 pin 撞同一个孔, 应该返回 Err
    /// (旧实现会静默 overwrite, 不报错)
    #[test]
    fn from_layout_detects_pin_collision() {
        let b = board();
        // 两个 2-pin 的"电阻", footprint offsets (0,0) (1,0)
        let circuit = Box::leak(Box::new(crate::circuit::Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "R1".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(0), PinId(1)],
                    footprint: Some(FootprintId(0)),
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R2".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(2), PinId(3)],
                    footprint: Some(FootprintId(0)),
                },
            ],
            pins: vec![
                crate::circuit::Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(2),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
            ],
            nets: vec![],
            footprints: vec![Footprint {
                id: FootprintId(0),
                name: "RES2".to_string(),
                pins: vec![
                    PhysicalPin {
                        name: "1".to_string(),
                        offset: Position { x: 0, y: 0 },
                    },
                    PhysicalPin {
                        name: "2".to_string(),
                        offset: Position { x: 1, y: 0 },
                    },
                ],
            }],
        }));
        let mut layout = Layout::new(circuit);
        // R1 在 (5, 2) → (5,2) (6,2)
        // R2 在 (6, 2) → (6,2) (7,2)
        // 碰撞: (6,2) 是 R1.pin1 和 R2.pin0
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement {
                position: Position { x: 6, y: 2 },
                rotation: Rotation::R0,
            },
        );

        let result = layout.occupancy(&b);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(
                |e| matches!(e, LayoutError::PinCollision { component: ComponentId(1), pin: PinId(2), hole } if *hole == b.at(6, 2).unwrap())
            ),
            "expected PinCollision on R2.pin0 at (6, 2), got: {errors:?}"
        );
    }

    /// 关键 bug 修复测试: wire 端点跟 pin 撞同孔, 应该返回 Err
    #[test]
    fn from_layout_detects_wire_pin_conflict() {
        let b = board();
        let mut layout = Layout::new(placed_q1_fixture());
        // 放 Q1, 占 (10,2) (11,2) (12,2)
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // wire 的 to 端点选 (11, 2) — 跟 Q1.pin1 撞
        let wire = Wire {
            id: super::super::routing::WireId(0),
            net: crate::circuit::NetId(0),
            from: b.at(15, 4).unwrap(),
            to: b.at(11, 2).unwrap(),
        };
        layout.add_wire(wire);

        let result = layout.occupancy(&b);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(
            |e| matches!(e, LayoutError::WireConflict { hole, .. } if *hole == b.at(11, 2).unwrap())
        ));
    }

    /// 验证 layout 不合法时**不返回**部分 occupancy
    #[test]
    fn from_layout_no_partial_occupancy_on_error() {
        let b = board();
        let circuit = Box::leak(Box::new(crate::circuit::Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "Q1".to_string(),
                    kind: "NPN".to_string(),
                    value: None,
                    pins: vec![PinId(0), PinId(1), PinId(2)],
                    footprint: Some(FootprintId(0)),
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R1".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(3), PinId(4)],
                    footprint: Some(FootprintId(1)),
                },
            ],
            pins: vec![
                crate::circuit::Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(4),
                    component: ComponentId(1),
                    num: "2".into(),
                    pinfunction: None,
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
                    name: "RES2".to_string(),
                    pins: vec![
                        PhysicalPin {
                            name: "1".to_string(),
                            offset: Position { x: 0, y: 0 },
                        },
                        PhysicalPin {
                            name: "2".to_string(),
                            offset: Position { x: 1, y: 0 },
                        },
                    ],
                },
            ],
        }));
        let mut layout = Layout::new(circuit);
        // Q1 在 (10, 2) R0 → 占 (10,2)(11,2)(12,2) (合法)
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // R1 在 (11, 2) R0 → 占 (11,2)(12,2) (跟 Q1 撞)
        layout.place(
            ComponentId(1),
            Placement {
                position: Position { x: 11, y: 2 },
                rotation: Rotation::R0,
            },
        );

        let result = layout.occupancy(&b);
        // 哪怕 Q1 单独是合法的, 只要整个 layout 有错, 就返回 Err
        assert!(result.is_err());
    }
}
