//! 孔占用: 每孔至多一个 occupant (面包板孔径物理约束)。
//!
//! 由 [`Occupancy::from_layout`] 从 Layout 派生; layout 不合法时返回 `Err`,
//! 不返回部分 occupancy。把"layout 合法"这个不变量从契约提到 type 层。

use std::collections::HashSet;

use crate::circuit::{ComponentId, PinId};

use super::Layout;
use super::LayoutError;
use super::breadboard::{Breadboard, HoleId, RailTieId};
use super::routing::Wire;

/// 一个孔的占有者. 面包板一孔径约束: 至多 1 个 occupant。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Occupant {
    /// 某个 component 的某个 pin 占此孔。
    Pin(PinId),
    /// 线插在此孔。`Wire` 只有 `from` 和 `to` 两个接触点 (没有中间点)。
    Wire(super::routing::WireId),
    /// 显式电源轨短接线的固定端点。
    RailTie(RailTieId),
    /// 被元件本体占据的孔 (在元件包围盒内, 但不是 pin)。
    /// 线不能插在 Blocked 孔上 (没有物理空间), 别的元件也不能把本体伸进来。
    Blocked(ComponentId),
}

#[derive(Debug)]
pub struct Occupancy {
    /// 下标 == HoleId.0
    at: Vec<Option<Occupant>>,
}

impl Occupancy {
    /// 没有元件和普通 wire 的 occupancy；固定 RailTie 端点已占用。
    pub fn empty(board: &Breadboard) -> Self {
        let mut occupancy = Self {
            at: vec![None; board.len()],
        };
        for tie in board.rail_ties() {
            for hole in tie.contacts() {
                occupancy.at[hole.raw()] = Some(Occupant::RailTie(tie.id));
            }
        }
        occupancy
    }

    /// 构造严格 occupancy. 任何非法状态都返回 `Err`, 不返回部分 occupancy。
    ///
    /// 错误种类 ([`crate::layout::LayoutError`]): `NoFootprint`, `NoFootprintPad`,
    /// `OutOfBounds`, `PinCollision`, `BBoxOverlap`, `WireConflict`, `ColumnConflict`。
    ///
    /// 下列情况的报告路径:
    /// - pin 跟其它 pin 重叠 → `PinCollision`
    /// - pin 跟其它元件 Blocked 孔重叠 → `BBoxOverlap`
    /// - pin 跟现有 wire 端点撞 → `PinCollision` (跟下面 bbox-vs-wire 不对称)
    /// - bbox 跟其它 pin 重叠 → `BBoxOverlap`
    /// - bbox 跟其它元件 Blocked 重叠 → `BBoxOverlap`
    /// - bbox 跟现有 wire 端点撞 → `WireConflict` (注: 此时 wire 字段是占位
    ///   `WireId(0)`, 真实 wire id 当前无法反查)
    /// - 同列同 rail 不同 net 的 pin → `ColumnConflict` (仅第一个冲突对)
    pub fn from_layout(layout: &Layout, board: &Breadboard) -> Result<Self, Vec<LayoutError>> {
        let mut occ = Self::empty(board);
        let mut errors = Vec::new();
        let mut occupied: HashSet<HoleId> = HashSet::new();
        // 按 rail_id 收集 endpoint, 用于检查"同短路集合不同 net" 冲突。
        // 之前用 (col, rail_top) 做 key, 引入电源轨后不够: 电源轨里两个不同 col
        // 的孔在同一 rail_id (横向短接), 也会被面包板短路。统一用 rail_id。
        let mut by_rail: std::collections::BTreeMap<u32, Vec<super::ColumnEndpoint>> =
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

            // 这个元件自己的 pin 集合, 区分"这个孔是它的 pin" vs "这个孔只是被它的本体遮挡"。
            let pin_hole_set: HashSet<HoleId> = placed.pin_holes.iter().map(|ph| ph.hole).collect();

            // 走遍包围盒的所有孔 (含 pin), 给每个填上 Pin / Blocked / 报错。
            // pin 优先于 Blocked; 先看 pin, 再处理 bbox 内的"非 pin"格。
            for ph in &placed.pin_holes {
                let hole = ph.hole;
                if let Some(prev) = occ.at[hole.0] {
                    errors.push(match prev {
                        Occupant::Pin(_) => LayoutError::PinCollision {
                            component: component.id,
                            pin: ph.pin,
                            hole,
                        },
                        Occupant::Blocked(other) => LayoutError::BBoxOverlap {
                            a: component.id,
                            b: other,
                            hole,
                        },
                        Occupant::Wire(_) => LayoutError::PinCollision {
                            component: component.id,
                            pin: ph.pin,
                            hole,
                        },
                        Occupant::RailTie(_) => LayoutError::PinCollision {
                            component: component.id,
                            pin: ph.pin,
                            hole,
                        },
                    });
                    continue;
                }
                occupied.insert(hole);
                occ.at[hole.0] = Some(Occupant::Pin(ph.pin));
                let pin = &layout.circuit().pins[ph.pin.0];
                let pos = board.hole(hole).position;
                // rail_id 统一处理纵向 rail + 电源轨横向 rail
                let rail_id = board.effective_rail_id_of(hole);
                by_rail
                    .entry(rail_id)
                    .or_default()
                    .push(super::ColumnEndpoint::Pin {
                        pin: ph.pin,
                        net: pin.net,
                    });
                let _ = pos;
            }

            if let Some(bbox) = placed.bbox {
                for pos in bbox.iter_cells() {
                    let Some(hole) = board.at(pos.x, pos.y) else {
                        continue;
                    };
                    if pin_hole_set.contains(&hole) {
                        // 已经处理过 pin
                        continue;
                    }
                    if let Some(prev) = occ.at[hole.0] {
                        errors.push(match prev {
                            Occupant::Pin(_) => {
                                // pin_owner 靠 occ.at[hole.0] 反查不出, 但 Pin 单元的
                                // owner 就是它自己的 PinId (没有 pin id → component id
                                // 的映射)。直接用占的孔反向查一下。
                                let owner = occ.at[hole.0]
                                    .and_then(|o| match o {
                                        Occupant::Pin(pid) => {
                                            Some(layout.circuit().pins[pid.0].component)
                                        }
                                        _ => None,
                                    })
                                    .unwrap_or(component.id);
                                LayoutError::BBoxOverlap {
                                    a: component.id,
                                    b: owner,
                                    hole,
                                }
                            }
                            Occupant::Blocked(other) => LayoutError::BBoxOverlap {
                                a: component.id,
                                b: other,
                                hole,
                            },
                            Occupant::Wire(_) => LayoutError::WireConflict {
                                wire: super::routing::WireId(0), // wire id 查不到, 占位
                                hole,
                            },
                            Occupant::RailTie(tie) => LayoutError::RailTieConflict { tie, hole },
                        });
                        continue;
                    }
                    occupied.insert(hole);
                    occ.at[hole.0] = Some(Occupant::Blocked(component.id));
                }
            }
        }

        for wire in layout.wires() {
            for hole in wire.contacts() {
                if let Some(prev) = occ.at[hole.0] {
                    errors.push(match prev {
                        Occupant::Pin(_) => {
                            // wire 端点落在某个元件的 pin 上 — pin 本来就被
                            // 占用, 但 wire 是后来加的, 这种是 wire 的问题。
                            LayoutError::WireConflict {
                                wire: wire.id,
                                hole,
                            }
                        }
                        Occupant::Wire(_) => LayoutError::WireConflict {
                            wire: wire.id,
                            hole,
                        },
                        Occupant::Blocked(_) => LayoutError::WireConflict {
                            wire: wire.id,
                            hole,
                        },
                        Occupant::RailTie(_) => LayoutError::WireConflict {
                            wire: wire.id,
                            hole,
                        },
                    });
                    continue;
                }
                occupied.insert(hole);
                occ.at[hole.0] = Some(Occupant::Wire(wire.id));
                let pos = board.hole(hole).position;
                let rail_id = board.effective_rail_id_of(hole);
                by_rail
                    .entry(rail_id)
                    .or_default()
                    .push(super::ColumnEndpoint::Wire {
                        wire: wire.id,
                        net: wire.net,
                    });
                let _ = pos;
            }
        }

        // Rail 冲突检查: 任意 rail_id 上, 任意两个 endpoint 的 net 不一致 → 报 ColumnConflict。
        // "Column" 现在名不副实 (电源轨冲突是 row 冲突), 但 API 名称保留, 只是里面的
        // column 字段改为 rail_id。选第一项作为"基准", 其后只报第一对 (一 rail 报一次避免刷屏)。
        for (rail_id, endpoints) in by_rail {
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
                        column: rail_id as i32,
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

    /// 查询某孔的当前占有者; `None` 表示孔空。
    pub fn occupant_at(&self, hole: HoleId) -> Option<Occupant> {
        self.at[hole.0]
    }

    /// 从 layout 构造 occupancy, **忽略所有错误**。
    ///
    /// placement.apply() 失败 (如一个 pin 越界) 时, **整个 placement 被跳过**:
    /// 不光 OOB 那个 pin, 该元件其它合法 pin + bbox 也一起丢失。
    /// placement 内部合法时: pin 优先于 Blocked (同一孔在 bbox 内又是 pin → 保留
    /// `Occupant::Pin` 而非 `Occupant::Blocked`); 不同元件间: 后到的覆盖先到的
    /// (典型例: 后到的 pin 会覆盖先到的 Blocked 标记)。
    ///
    /// 主程序在 `validate` 报错时用它来 "尽力跑接线 + 画 SVG"。
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
            // 先把 Blocked 填上, 再让 Pin 覆盖。
            if let Some(bbox) = placed.bbox {
                for pos in bbox.iter_cells() {
                    let Some(hole) = board.at(pos.x, pos.y) else {
                        continue;
                    };
                    if !matches!(occ.at[hole.0], Some(Occupant::RailTie(_))) {
                        occ.at[hole.0] = Some(Occupant::Blocked(component.id));
                    }
                }
            }
            for ph in placed.pin_holes {
                if !matches!(occ.at[ph.hole.0], Some(Occupant::RailTie(_))) {
                    occ.at[ph.hole.0] = Some(Occupant::Pin(ph.pin));
                }
            }
        }
        for w in layout.wires() {
            for h in w.contacts() {
                if !matches!(occ.at[h.0], Some(Occupant::RailTie(_))) {
                    occ.at[h.0] = Some(Occupant::Wire(w.id));
                }
            }
        }
        occ
    }

    /// 当前是否可在此孔插 pin (仅当孔空)。
    pub fn can_place_pin(&self, hole: HoleId) -> bool {
        self.at[hole.0].is_none()
    }

    /// wire 两端孔当前是否都为空。
    pub fn can_add_wire(&self, wire: &Wire) -> bool {
        self.at[wire.from.0].is_none() && self.at[wire.to.0].is_none()
    }
}

#[cfg(test)]
mod rail_tie_tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::breadboard::standard_power_rails;
    use crate::layout::placement::Placement;

    #[test]
    fn preset_rail_tie_endpoints_are_fixed_occupied_geometry() {
        let board = Breadboard::standard();
        let occupancy = Occupancy::empty(&board);

        assert_eq!(board.rail_ties().len(), 2);
        for tie in board.rail_ties() {
            for hole in tie.contacts() {
                assert_eq!(occupancy.occupant_at(hole), Some(Occupant::RailTie(tie.id)));
            }
        }
    }

    #[test]
    fn validation_uses_the_same_effective_connectivity_as_rail_ties() {
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "X1".into(),
                kind: "X".into(),
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
        let raw = Breadboard::with_power_rails(30, 12, [5, 6], standard_power_rails(30));
        let preset = Breadboard::standard();
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::Bridged {
                pin_holes: [
                    (raw.at(1, -4).unwrap(), PinId(0)),
                    (raw.at(1, 14).unwrap(), PinId(1)),
                ],
            },
        );

        assert!(
            layout.occupancy(&raw).is_ok(),
            "无 tie 时 top/bottom 不同 net 不应互相短路"
        );
        let errors = layout
            .occupancy(&preset)
            .expect_err("preset tie 应让 top/bottom 的不同 net 产生冲突");
        assert!(
            errors
                .iter()
                .any(|error| matches!(error, LayoutError::ColumnConflict { .. }))
        );
    }
}

// ============================================================
//  BBox / Blocked 专项测试
// ============================================================

#[cfg(test)]
mod bbox_tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, NetId, PhysicalPin, Pin, PinId,
        Position,
    };
    use crate::layout::placement::{Placement, Rotation};
    use crate::layout::routing::{Wire, WireId};

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    /// 2 pin footprint, pins at (0,0) and (3,0) → 跨度 4 cols, (1,0) (2,0) 是本体。
    fn axial_footprint() -> Footprint {
        Footprint {
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
        }
    }

    fn axial_circuit() -> &'static Circuit {
        Box::leak(Box::new(Circuit {
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
            nets: vec![],
            footprints: vec![axial_footprint()],
        }))
    }

    /// 单独 axial 摆放后: pin = Pin, 中间两个孔 = Blocked
    #[test]
    fn axial_marks_pins_and_body() {
        let b = board();
        let mut layout = Layout::new(axial_circuit());
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        // 端点
        assert_eq!(
            occ.occupant_at(b.at(5, 2).unwrap()),
            Some(Occupant::Pin(PinId(0)))
        );
        assert_eq!(
            occ.occupant_at(b.at(8, 2).unwrap()),
            Some(Occupant::Pin(PinId(1)))
        );
        // 本体跨过的中间两个孔
        assert_eq!(
            occ.occupant_at(b.at(6, 2).unwrap()),
            Some(Occupant::Blocked(ComponentId(0)))
        );
        assert_eq!(
            occ.occupant_at(b.at(7, 2).unwrap()),
            Some(Occupant::Blocked(ComponentId(0)))
        );
        // 外面仍然是空
        assert_eq!(occ.occupant_at(b.at(4, 2).unwrap()), None);
        assert_eq!(occ.occupant_at(b.at(9, 2).unwrap()), None);
    }

    /// can_place_pin / can_add_wire 返回 false, 表示 线不能穿过本体
    #[test]
    fn blocked_holes_reject_pins_and_wires() {
        let b = board();
        let mut layout = Layout::new(axial_circuit());
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        // pin 孔不能上放
        assert!(!occ.can_place_pin(b.at(5, 2).unwrap()));
        assert!(!occ.can_place_pin(b.at(8, 2).unwrap()));
        // 本体孔也不能
        assert!(!occ.can_place_pin(b.at(6, 2).unwrap()));
        assert!(!occ.can_place_pin(b.at(7, 2).unwrap()));
        // 线穿过也不行
        let wire = Wire {
            id: WireId(0),
            net: NetId(0),
            from: b.at(6, 2).unwrap(),
            to: b.at(10, 0).unwrap(),
        };
        assert!(!occ.can_add_wire(&wire));
        // 旁边仍然是空, 能上放
        assert!(occ.can_place_pin(b.at(4, 2).unwrap()));
        let wire_ok = Wire {
            id: WireId(0),
            net: NetId(0),
            from: b.at(4, 2).unwrap(),
            to: b.at(10, 0).unwrap(),
        };
        assert!(occ.can_add_wire(&wire_ok));
    }

    /// 另一个元件的 pin 落在已有元件的 bbox 里 → BBoxOverlap
    #[test]
    fn bbox_overlap_pin_under_body_reported() {
        let b = board();
        // 2 个 axial footprint
        let fp = axial_footprint();
        let make_comp = |id: usize| Component {
            id: ComponentId(id),
            ref_: format!("R{id}"),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(id * 2), PinId(id * 2 + 1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let make_pin = |id: usize, comp_id: usize| Pin {
            id: PinId(id),
            component: ComponentId(comp_id),
            num: if id.is_multiple_of(2) { "1" } else { "2" }.into(),
            pinfunction: None,
            physical_pin_index: if id.is_multiple_of(2) { 0 } else { 1 },
            net: None,
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![make_comp(0), make_comp(1)],
            pins: vec![
                make_pin(0, 0),
                make_pin(1, 0),
                make_pin(2, 1),
                make_pin(3, 1),
            ],
            nets: vec![],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        // R1 摆在 (5, 2): bbox (5..=8, 2..=2), pin 在 (5,2) (8,2)
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // R2 摆在 (7, 2): bbox (7..=10, 2..=2), pin (7,2) 落在 R1 本体上
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 7, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let errs = layout.occupancy(&b).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, LayoutError::BBoxOverlap { .. })),
            "应当报 BBoxOverlap, got {errs:?}"
        );
    }

    /// 两个元件本体互撞 (都没有 pin 落在重叠区) 也要报 BBoxOverlap。
    #[test]
    fn bbox_overlap_body_under_body_reported() {
        let b = board();
        // 两个 footprint: pin 跨度 3, 中间 1 个本体格
        let fp = Footprint {
            id: FootprintId(0),
            name: "axial_wide".into(),
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
        };
        let make_comp = |id: usize| Component {
            id: ComponentId(id),
            ref_: format!("R{id}"),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(id * 2), PinId(id * 2 + 1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let make_pin = |id: usize, comp: usize, num: &str| Pin {
            id: PinId(id),
            component: ComponentId(comp),
            num: num.into(),
            pinfunction: None,
            physical_pin_index: if num == "1" { 0 } else { 1 },
            net: None,
        };
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![make_comp(0), make_comp(1)],
            pins: vec![
                make_pin(0, 0, "1"),
                make_pin(1, 0, "2"),
                make_pin(2, 1, "1"),
                make_pin(3, 1, "2"),
            ],
            nets: vec![],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        // R1 bbox (5..=7, 2); pin 在 (5,2) (7,2)
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // R2 bbox (6..=8, 2); pin 在 (6,2) (8,2)。重叠在 (6,2) (7,2)。
        // (6,2) 是 R1 本体; (7,2) 是 R1 pin。
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 6, y: 2 },
                rotation: Rotation::R0,
            },
        );
        let errs = layout.occupancy(&b).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, LayoutError::BBoxOverlap { .. })),
            "应报 BBoxOverlap, got {errs:?}"
        );
    }

    /// R180: pin 反转, bbox 也跟着翻。
    #[test]
    fn bbox_handles_r180() {
        let b = board();
        let mut layout = Layout::new(axial_circuit());
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 8, y: 2 },
                rotation: Rotation::R180,
            },
        );
        let occ = layout.occupancy(&b).unwrap();
        // R180: pin offset (0,0) → (0,0); pin offset (3,0) → (-3,0).
        // placement (8,2): pin 在 (8,2) 和 (5,2). bbox = (5..=8, 2..=2).
        assert_eq!(
            occ.occupant_at(b.at(8, 2).unwrap()),
            Some(Occupant::Pin(PinId(0)))
        );
        assert_eq!(
            occ.occupant_at(b.at(5, 2).unwrap()),
            Some(Occupant::Pin(PinId(1)))
        );
        assert_eq!(
            occ.occupant_at(b.at(6, 2).unwrap()),
            Some(Occupant::Blocked(ComponentId(0)))
        );
        assert_eq!(
            occ.occupant_at(b.at(7, 2).unwrap()),
            Some(Occupant::Blocked(ComponentId(0)))
        );
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
                bridgeable: false,
            }],
            pins: vec![
                crate::circuit::Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    physical_pin_index: 1,
                    net: None,
                },
                crate::circuit::Pin {
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

    /// 列冲突反面: 2 个 pin 落在同列不同 row, 但都是 "无 net" → 不报 ColumnConflict
    /// (None 跟 None 视为同一, 两个未连接的 pin 插同列不是电气连接——它们本来就不该连)
    #[test]
    fn column_conflict_no_net_no_conflict() {
        let b = Breadboard::new(30, 5);
        let mut layout = Layout::new(two_component_fixture());
        // two_component_fixture 的所有 pin 都 net: None
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
            Placement::OnBoard {
                position: Position { x: 0, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
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
                name: "n0".into(),
                pins: vec![PinId(0)],
            }],
            footprints: vec![fp],
        }));
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
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
            Placement::OnBoard {
                position: Position { x: 0, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
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
            Placement::OnBoard {
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
                    bridgeable: false,
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R2".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(2), PinId(3)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
            ],
            pins: vec![
                crate::circuit::Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    physical_pin_index: 1,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(2),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "2".into(),
                    pinfunction: None,
                    physical_pin_index: 1,
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
            Placement::OnBoard {
                position: Position { x: 5, y: 2 },
                rotation: Rotation::R0,
            },
        );
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
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
            Placement::OnBoard {
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
                crate::circuit::Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    physical_pin_index: 1,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    physical_pin_index: 2,
                    net: None,
                },
                crate::circuit::Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    physical_pin_index: 0,
                    net: None,
                },
                crate::circuit::Pin {
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
            Placement::OnBoard {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );
        // R1 在 (11, 2) R0 → 占 (11,2)(12,2) (跟 Q1 撞)
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 11, y: 2 },
                rotation: Rotation::R0,
            },
        );

        let result = layout.occupancy(&b);
        // 哪怕 Q1 单独是合法的, 只要整个 layout 有错, 就返回 Err
        assert!(result.is_err());
    }

    // ============================================================
    //  Rail 感知: 上下半的 pin 即便同列, 也不该视为冲突
    // ============================================================

    /// 标准板 (30x12, rows 5..7 blocked) 上, 同列不同 rail 的不同 net pin
    /// 不该被报 ColumnConflict — 它们在物理上不连通。
    #[test]
    fn column_conflict_ignores_different_rails_on_full_board() {
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
        // A 在上 rail: (0, 2), B 在下 rail: (0, 10) — 同 col, 不同 rail
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
                position: Position { x: 0, y: 10 },
                rotation: Rotation::R0,
            },
        );
        let result = layout.occupancy(&b);
        assert!(
            result.is_ok(),
            "上下 rail 同列不同 net 不该报 ColumnConflict, got: {result:?}"
        );
    }

    /// 标准板上, 同列同 rail 不同 net → 仍报 ColumnConflict (回归测试)。
    #[test]
    fn column_conflict_still_reports_same_rail_on_full_board() {
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
        // A 在上 rail: (0, 2), B 在上 rail: (0, 4) — 同 col, 同 rail, 不同 net
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
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R0,
            },
        );
        let errors = layout.occupancy(&b).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, crate::layout::LayoutError::ColumnConflict { .. })),
            "同 rail 内同列不同 net 必须报 ColumnConflict, got: {errors:?}"
        );
    }
}
