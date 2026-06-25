//! 元件摆放: position + rotation → 每个 pin 落在哪个孔上。

use crate::circuit::{Component, Footprint};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Placement {
    /// 摆放点, 板内坐标
    pub position: crate::circuit::Position,
    pub rotation: Rotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinHole {
    pub pin: crate::circuit::PinId,
    pub hole: HoleId,
}

#[derive(Debug)]
pub struct PlacedFootprint {
    pub pin_holes: Vec<PinHole>,
}

impl PlacedFootprint {
    pub fn occupied_holes(&self) -> impl Iterator<Item = HoleId> + '_ {
        self.pin_holes.iter().map(|ph| ph.hole)
    }
}

impl Placement {
    /// 把 placement 应用到 component + footprint, 算出每个 pin 落在哪个孔上。
    ///
    /// **按 `pin.num()` 查 footprint 的同名 pad** — 不假设 component.pins 和
    /// footprint.pins 下标对应。KiCad 约定: symbol pin N ↔ footprint pad N,
    /// 但 netlist 里 pin 出现的顺序可能跟 footprint pad 顺序不一样
    /// (例: TO-92 在 netlist 里 pins = [2, 1, 3], footprint pads = [1, 2, 3])。
    /// zip 下来会全部错位。
    ///
    /// `apply` 本身只检查**单个 placement** 的合法性 (边界);
    /// pin 跟其他元件 pin 的碰撞由 [`Layout::validate`] / [`Occupancy::from_layout`] 检查。
    pub fn apply(
        &self,
        component: &Component,
        footprint: &Footprint,
        board: &Breadboard,
        pins: &[crate::circuit::Pin],
    ) -> Result<PlacedFootprint, LayoutError> {
        use crate::circuit::Position;

        let mut pin_holes = Vec::with_capacity(component.pins.len());

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
            let rotated = rotate(physical_pin.offset(), self.rotation);
            let absolute = Position {
                x: self.position.x + rotated.x,
                y: self.position.y + rotated.y,
            };
            let hole = board
                .at(absolute.x, absolute.y)
                .ok_or(LayoutError::OutOfBounds {
                    component: component.id,
                    pin: *pin_id,
                    hole: absolute,
                })?;
            pin_holes.push(PinHole { pin: *pin_id, hole });
        }

        Ok(PlacedFootprint { pin_holes })
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
        let p = Placement {
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
        let p = Placement {
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
        let p = Placement {
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
        let p = Placement {
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
        let p = Placement {
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
        let p = Placement {
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
        let p = Placement {
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
}
