//! 元件摆放: position + rotation → 每个 pin 落在哪个孔上 + 元件包围盒。

use crate::circuit::{Component, Footprint, Position};

use super::LayoutError;
use super::breadboard::{Breadboard, HoleId};

/// 旋转 90° 的整数倍。
///
/// 旋转公式 (逆时针, 数学约定; 在 y 向下的屏幕上看是顺时针):
/// - `R0`:   `(x, y) → ( x,  y)`
/// - `R90`:  `(x, y) → (-y,  x)`
/// - `R180`: `(x, y) → (-x, -y)`
/// - `R270`: `(x, y) → ( y, -x)`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotation {
    R0,
    R90,
    R180,
    R270,
}

/// 元件的轴对齐包围盒 (单位: 面包板孔)。
///
/// 既用于渲染 (画 reference 框), 也用于 occupancy: bbox 内部的孔除了 pin
/// 之外都属于"被元件本体占据" (`Occupant::Blocked`), wire 不能进, 别的元件
/// 的 bbox 也不能跨进来。
///
/// `min_* <= max_*`, 跟面包板网格对齐 (整数坐标)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox {
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}

impl BBox {
    /// 用一组 (x, y) 点构造包围盒; 空集返回 None。
    pub fn from_points(points: impl IntoIterator<Item = Position>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let mut min_x = first.x;
        let mut max_x = first.x;
        let mut min_y = first.y;
        let mut max_y = first.y;
        for p in it {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }
        Some(Self {
            min_x,
            max_x,
            min_y,
            max_y,
        })
    }

    /// 两个 bbox 在至少一个 cell 上重叠 (含边界相等)。
    /// 边界刚好相切 (A.max_x == B.min_x) 不算重叠。
    pub fn overlaps(&self, other: &BBox) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }

    /// 枚举 bbox 内部所有 (x, y) 整数格点。
    pub fn iter_cells(&self) -> impl Iterator<Item = Position> + '_ {
        let min_x = self.min_x;
        let max_x = self.max_x;
        let min_y = self.min_y;
        let max_y = self.max_y;
        (min_y..=max_y).flat_map(move |y| (min_x..=max_x).map(move |x| Position { x, y }))
    }
}

/// 元件的摆放方式。
///
/// 有两种:
/// - [`Placement::OnBoard`] — 标准: 给定位置 + 旋转, footprint 上的 pin 偏移
///   推出每个 pin 的世界坐标。多数元件走这条路。
/// - [`Placement::Bridged`] — 桥接: 两条腿各指定一个 [`HoleId`], body 浮在
///   板外。常见于从 power rail 跨到主区的电阻 / LED / 二极管。**不进 SA**,
///   由用户自己摆。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placement {
    OnBoard {
        /// 摆放点, 板内坐标
        position: crate::circuit::Position,
        rotation: Rotation,
    },
    /// 桥接元件: body 在板外, 两条腿各一个孔, 内部走"导线"(就是元件本身)。
    /// 每条 leg 一对 `(HoleId, PinId)`, 顺序无要求。
    Bridged {
        pin_holes: [(HoleId, crate::circuit::PinId); 2],
    },
}

impl Placement {
    /// 是否是桥接元件。
    pub fn is_bridged(&self) -> bool {
        matches!(self, Placement::Bridged { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinHole {
    pub pin: crate::circuit::PinId,
    pub hole: HoleId,
}

#[derive(Debug)]
pub struct PlacedFootprint {
    pub pin_holes: Vec<PinHole>,
    /// 元件本体在板上的轴对齐包围盒 (旋转 + 平移后)。没有 pin 时为 None
    /// (例如纯 silk 元件) — 这种元件不算占据任何网格。
    pub bbox: Option<BBox>,
}

impl PlacedFootprint {
    pub fn occupied_holes(&self) -> impl Iterator<Item = HoleId> + '_ {
        self.pin_holes.iter().map(|ph| ph.hole)
    }
}

impl Placement {
    /// 把 placement 应用到 component + footprint, 算出每个 pin 落在哪个孔上。
    ///
    /// **OnBoard 路径**: 按 `pin.num()` 查 footprint 的同名 pad — 不假设
    /// component.pins 和 footprint.pins 下标对应。KiCad 约定: symbol pin N
    /// ↔ footprint pad N, 但 netlist 里 pin 出现的顺序可能跟 footprint pad 顺序
    /// 不一样 (例: TO-92 在 netlist 里 pins = [2, 1, 3], footprint pads =
    /// [1, 2, 3])。zip 下来会全部错位。
    ///
    /// **Bridged 路径**: 直接拿用户指定的 `(HoleId, PinId)` 对, 验证 pin 属
    /// 于这个 component, hole 在板上, 两条腿不撞同一个孔。**没有 bbox** (body
    /// 浮在板外, 不占任何网格)。
    ///
    /// `apply` 本身只检查**单个 placement** 的合法性 (边界 / 孔有效 / 腿不撞);
    /// pin 跟其他元件 pin / wire 的碰撞由 [`Layout::validate`] /
    /// [`Occupancy::from_layout`] 检查。
    pub fn apply(
        &self,
        component: &Component,
        footprint: &Footprint,
        board: &Breadboard,
        pins: &[crate::circuit::Pin],
    ) -> Result<PlacedFootprint, LayoutError> {
        use crate::circuit::Position;

        match self {
            Placement::OnBoard { position, rotation } => {
                let mut pin_holes = Vec::with_capacity(component.pins.len());
                let mut world_positions: Vec<Position> = Vec::with_capacity(component.pins.len());

                for pin_id in &component.pins {
                    let pin = &pins[pin_id.0];
                    // 按 num 找 footprint 里同名 pad
                    let physical_pin = footprint
                        .pins()
                        .iter()
                        .find(|pp| pp.name() == pin.num())
                        .ok_or_else(|| LayoutError::NoFootprintPad {
                            component: component.id,
                            pin: *pin_id,
                            pad_name: pin.num().to_string(),
                        })?;
                    let rotated = rotate(physical_pin.offset(), *rotation);
                    let absolute = Position {
                        x: position.x + rotated.x,
                        y: position.y + rotated.y,
                    };
                    let hole =
                        board
                            .at(absolute.x, absolute.y)
                            .ok_or(LayoutError::OutOfBounds {
                                component: component.id,
                                pin: *pin_id,
                                hole: absolute,
                            })?;
                    pin_holes.push(PinHole { pin: *pin_id, hole });
                    world_positions.push(absolute);
                }

                // 用所有 pin 的世界坐标构 bbox。footprint 暂时没有 "body extent" 字段,
                // 这里就跟渲染里一样, 用 pin 的范围代表元件占据的网格范围。
                // 后续如果从 .kicad_mod 解析了 body silk, 可以替换这个 bbox。
                let bbox = BBox::from_points(world_positions);
                Ok(PlacedFootprint { pin_holes, bbox })
            }
            Placement::Bridged { pin_holes } => {
                let mut placed: Vec<PinHole> = Vec::with_capacity(pin_holes.len());
                for &(hole_id, pin_id) in pin_holes {
                    // pin 必须属于这个 component
                    if !component.pins.contains(&pin_id) {
                        let pad_name = pins
                            .get(pin_id.0)
                            .map(|p| p.num().to_string())
                            .unwrap_or_default();
                        return Err(LayoutError::NoFootprintPad {
                            component: component.id,
                            pin: pin_id,
                            pad_name,
                        });
                    }
                    // hole 必须真实存在 (不查 rail / region 语义, 由后续 occupancy 判定)
                    if hole_id.0 >= board.holes().len() {
                        let hole_pos = board
                            .holes()
                            .get(hole_id.0)
                            .map(|h| h.position)
                            .unwrap_or(Position { x: -1, y: -1 });
                        return Err(LayoutError::OutOfBounds {
                            component: component.id,
                            pin: pin_id,
                            hole: hole_pos,
                        });
                    }
                    placed.push(PinHole {
                        pin: pin_id,
                        hole: hole_id,
                    });
                }
                // 两条腿不能落在同一个孔
                if placed[0].hole == placed[1].hole {
                    return Err(LayoutError::PinCollision {
                        component: component.id,
                        pin: placed[1].pin,
                        hole: placed[1].hole,
                    });
                }
                // Bridged 没有 body, 不算占据任何网格
                Ok(PlacedFootprint {
                    pin_holes: placed,
                    bbox: None,
                })
            }
        }
    }
}

pub(crate) fn rotate(pos: crate::circuit::Position, rot: Rotation) -> crate::circuit::Position {
    match rot {
        Rotation::R0 => pos,
        Rotation::R90 => crate::circuit::Position {
            x: -pos.y,
            y: pos.x,
        },
        Rotation::R180 => crate::circuit::Position {
            x: -pos.x,
            y: -pos.y,
        },
        Rotation::R270 => crate::circuit::Position {
            x: pos.y,
            y: -pos.x,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{ComponentId, FootprintId, PhysicalPin, Pin, PinId, Position};

    /// TO92 footprint, pad num 跟 component pin num 一一对应:
    /// pad "1" @ (0,0)  pad "2" @ (1,0)  pad "3" @ (2,0)
    fn to92_footprint() -> Footprint {
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
        }
    }

    /// 3 个 pin, num 顺序 1, 2, 3
    fn q1_pins() -> Vec<Pin> {
        (0..3)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(0),
                num: (i + 1).to_string(),
                pinfunction: None,
                net: None,
            })
            .collect()
    }

    fn q1_component() -> Component {
        Component {
            id: ComponentId(0),
            ref_: "Q1".to_string(),
            kind: "NPN".to_string(),
            value: Some("BC547".to_string()),
            pins: vec![PinId(0), PinId(1), PinId(2)],
            footprint: Some(FootprintId(0)),
                bridgeable: false,
        }
    }

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn rotate_r0_unchanged() {
        assert_eq!(
            rotate(Position { x: 1, y: 2 }, Rotation::R0),
            Position { x: 1, y: 2 }
        );
    }

    #[test]
    fn rotate_r90() {
        assert_eq!(
            rotate(Position { x: 1, y: 0 }, Rotation::R90),
            Position { x: 0, y: 1 }
        );
        assert_eq!(
            rotate(Position { x: 0, y: 1 }, Rotation::R90),
            Position { x: -1, y: 0 }
        );
    }

    #[test]
    fn rotate_r180() {
        assert_eq!(
            rotate(Position { x: 1, y: 0 }, Rotation::R180),
            Position { x: -1, y: 0 }
        );
        assert_eq!(
            rotate(Position { x: 0, y: 1 }, Rotation::R180),
            Position { x: 0, y: -1 }
        );
    }

    #[test]
    fn rotate_r270() {
        assert_eq!(
            rotate(Position { x: 1, y: 0 }, Rotation::R270),
            Position { x: 0, y: -1 }
        );
        assert_eq!(
            rotate(Position { x: 0, y: 1 }, Rotation::R270),
            Position { x: 1, y: 0 }
        );
    }

    #[test]
    fn apply_r0_at_origin() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let pins = q1_pins();
        let p = Placement::OnBoard {
            position: Position { x: 5, y: 2 },
            rotation: Rotation::R0,
        };

        let r = p.apply(&comp, &fp, &b, &pins).unwrap();
        assert_eq!(r.pin_holes.len(), 3);
        assert_eq!(r.pin_holes[0].hole, b.at(5, 2).unwrap());
        assert_eq!(r.pin_holes[1].hole, b.at(6, 2).unwrap());
        assert_eq!(r.pin_holes[2].hole, b.at(7, 2).unwrap());
    }

    #[test]
    fn apply_r90_pins_go_down() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let pins = q1_pins();
        let p = Placement::OnBoard {
            position: Position { x: 10, y: 0 },
            rotation: Rotation::R90,
        };

        let r = p.apply(&comp, &fp, &b, &pins).unwrap();
        assert_eq!(r.pin_holes[0].hole, b.at(10, 0).unwrap());
        assert_eq!(r.pin_holes[1].hole, b.at(10, 1).unwrap());
        assert_eq!(r.pin_holes[2].hole, b.at(10, 2).unwrap());
    }

    #[test]
    fn apply_r90_at_top_row_out_of_bounds() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let pins = q1_pins();
        let p = Placement::OnBoard {
            position: Position { x: 0, y: 4 },
            rotation: Rotation::R90,
        };

        let r = p.apply(&comp, &fp, &b, &pins);
        assert!(matches!(r, Err(LayoutError::OutOfBounds { .. })));
    }

    #[test]
    fn apply_out_of_bounds_includes_component_and_pin() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let pins = q1_pins();
        let p = Placement::OnBoard {
            position: Position { x: 0, y: 4 },
            rotation: Rotation::R90,
        };
        // R90: (x, y) → (-y, x)
        // pin num "1" (col 0 offset): (0,0) → (0,0) 绝对 (0,4) ✓
        // pin num "2" (col 1 offset): (1,0) → (0,1) 绝对 (0,5) ✗ 越界
        // pin num "3" (col 2 offset): (2,0) → (0,2) 绝对 (0,6) ✗ 越界
        // 顺序遍历 component.pins (PinId 0, 1, 2) → num "1", "2", "3", 第一个越界是 pin "2" (PinId 1)
        let r = p.apply(&comp, &fp, &b, &pins);
        match r {
            Err(LayoutError::OutOfBounds {
                component,
                pin,
                hole,
            }) => {
                assert_eq!(component, ComponentId(0));
                assert_eq!(pin, PinId(1));
                assert_eq!(hole, Position { x: 0, y: 5 });
            }
            _ => panic!("expected OutOfBounds"),
        }
    }

    #[test]
    fn apply_r180_flips_horizontally() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let pins = q1_pins();
        let p = Placement::OnBoard {
            position: Position { x: 10, y: 2 },
            rotation: Rotation::R180,
        };

        let r = p.apply(&comp, &fp, &b, &pins).unwrap();
        assert_eq!(r.pin_holes[0].hole, b.at(10, 2).unwrap());
        assert_eq!(r.pin_holes[1].hole, b.at(9, 2).unwrap());
        assert_eq!(r.pin_holes[2].hole, b.at(8, 2).unwrap());
    }

    #[test]
    fn occupied_holes_iter() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let pins = q1_pins();
        let p = Placement::OnBoard {
            position: Position { x: 1, y: 1 },
            rotation: Rotation::R0,
        };

        let r = p.apply(&comp, &fp, &b, &pins).unwrap();
        let occ: Vec<HoleId> = r.occupied_holes().collect();
        assert_eq!(occ.len(), 3);
    }

    /// **回归测试**: 之前是按 index zip, Q1 在 netlist 里 pins = [2, 1, 3],
    /// footprint pads = [1, 2, 3] 时会全部错位。现在按 num 查, 位置正确。
    #[test]
    fn apply_matches_by_pin_num_not_index() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        // pins 顺序: num "2", "1", "3" (跟 random.net 里 Q1 一样)
        let pins = vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                net: None,
            },
            Pin {
                id: PinId(2),
                component: ComponentId(0),
                num: "3".into(),
                pinfunction: None,
                net: None,
            },
        ];
        let p = Placement::OnBoard {
            position: Position { x: 10, y: 2 },
            rotation: Rotation::R0,
        };

        let r = p.apply(&comp, &fp, &b, &pins).unwrap();
        // pin num "1" 应该在 footprint pad 1 (col 0 offset) → (10, 2)
        // pin num "2" 应该在 footprint pad 2 (col 1 offset) → (11, 2)
        // pin num "3" 应该在 footprint pad 3 (col 2 offset) → (12, 2)
        // pin_holes 按 component.pins 顺序排 (PinId 0, 1, 2)
        assert_eq!(
            r.pin_holes[0].hole,
            b.at(11, 2).unwrap(),
            "PinId 0 (num 2) → pad 2 → col 11"
        );
        assert_eq!(
            r.pin_holes[1].hole,
            b.at(10, 2).unwrap(),
            "PinId 1 (num 1) → pad 1 → col 10"
        );
        assert_eq!(
            r.pin_holes[2].hole,
            b.at(12, 2).unwrap(),
            "PinId 2 (num 3) → pad 3 → col 12"
        );
    }

    // ============================================================
    //  Bridged placement
    // ============================================================

    fn two_pin_resistor() -> (Component, Footprint, Vec<Pin>) {
        let fp = Footprint {
            id: FootprintId(1),
            name: "RES2".into(),
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
        let comp = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(1)),
                bridgeable: false,
        };
        let pins = vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                net: None,
            },
        ];
        (comp, fp, pins)
    }

    #[test]
    fn bridged_apply_registers_both_pins() {
        // 桥接 2-pin 元件: pin 1 在 main board (3, 0), pin 2 在负极轨 (0, -4)
        let b = Breadboard::standard();
        let (comp, fp, pins) = two_pin_resistor();
        let h_main = b.at(3, 0).unwrap();
        let h_rail = b.at(0, -4).unwrap();
        let placement = Placement::Bridged {
            pin_holes: [(h_main, PinId(0)), (h_rail, PinId(1))],
        };
        let placed = placement
            .apply(&comp, &fp, &b, &pins)
            .expect("bridged apply should succeed");
        assert_eq!(placed.pin_holes.len(), 2);
        assert!(placed.bbox.is_none(), "bridged 没有 body, bbox = None");
        let ph0 = placed.pin_holes.iter().find(|p| p.pin == PinId(0)).unwrap();
        let ph1 = placed.pin_holes.iter().find(|p| p.pin == PinId(1)).unwrap();
        assert_eq!(ph0.hole, h_main);
        assert_eq!(ph1.hole, h_rail);
    }

    #[test]
    fn bridged_rejects_pin_not_in_component() {
        // pin 99 不在 component.pins 里, 应该报错
        let b = Breadboard::standard();
        let (comp, fp, pins) = two_pin_resistor();
        let h1 = b.at(3, 0).unwrap();
        let h2 = b.at(0, -4).unwrap();
        let placement = Placement::Bridged {
            pin_holes: [(h1, PinId(0)), (h2, PinId(99))],
        };
        let result = placement.apply(&comp, &fp, &b, &pins);
        assert!(matches!(result, Err(LayoutError::NoFootprintPad { .. })));
    }

    #[test]
    fn bridged_rejects_same_hole_twice() {
        // 两条腿落在同一个孔: 应该报 PinCollision
        let b = Breadboard::standard();
        let (comp, fp, pins) = two_pin_resistor();
        let h1 = b.at(3, 0).unwrap();
        let h2 = b.at(3, 0).unwrap();
        let placement = Placement::Bridged {
            pin_holes: [(h1, PinId(0)), (h2, PinId(1))],
        };
        let result = placement.apply(&comp, &fp, &b, &pins);
        assert!(matches!(result, Err(LayoutError::PinCollision { .. })));
    }

    #[test]
    fn bridged_rejects_out_of_bounds_hole() {
        // HoleId 越界 (随便编一个超大的 id)
        let b = Breadboard::standard();
        let (comp, fp, pins) = two_pin_resistor();
        let h1 = b.at(3, 0).unwrap();
        let bogus_hole = HoleId(99999);
        let placement = Placement::Bridged {
            pin_holes: [(h1, PinId(0)), (bogus_hole, PinId(1))],
        };
        let result = placement.apply(&comp, &fp, &b, &pins);
        assert!(matches!(result, Err(LayoutError::OutOfBounds { .. })));
    }

    #[test]
    fn bridged_is_bridged_helper() {
        let on_board = Placement::OnBoard {
            position: Position { x: 0, y: 0 },
            rotation: Rotation::R0,
        };
        assert!(!on_board.is_bridged());

        let bridged = Placement::Bridged {
            pin_holes: [(HoleId(0), PinId(0)), (HoleId(1), PinId(1))],
        };
        assert!(bridged.is_bridged());
    }
}
