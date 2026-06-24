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
    /// 约定: `component.pins` 和 `footprint.pins` 按下标一一对应
    /// (KiCad netlist 把 symbol pin N 接到 footprint pad N)。
    /// 不一致时按下标 zip, 多出的会被截断。
    ///
    /// `apply` 本身只检查**单个 placement** 的合法性 (边界);
    /// pin 跟其他元件 pin 的碰撞由 [`Layout::validate`] / [`Occupancy::from_layout`] 检查。
    pub fn apply(
        &self,
        component: &Component,
        footprint: &Footprint,
        board: &Breadboard,
    ) -> Result<PlacedFootprint, LayoutError> {
        use crate::circuit::Position;

        let mut pin_holes = Vec::with_capacity(component.pins.len().min(footprint.pins.len()));

        for (pin_id, physical_pin) in component.pins.iter().zip(footprint.pins.iter()) {
            let rotated = rotate(physical_pin.offset, self.rotation);
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

fn rotate(pos: crate::circuit::Position, rot: Rotation) -> crate::circuit::Position {
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
    use crate::circuit::{ComponentId, FootprintId, PhysicalPin, PinId, Position};

    fn to92_footprint() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "TO92".to_string(),
            pins: vec![
                PhysicalPin {
                    name: "C".to_string(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "B".to_string(),
                    offset: Position { x: 1, y: 0 },
                },
                PhysicalPin {
                    name: "E".to_string(),
                    offset: Position { x: 2, y: 0 },
                },
            ],
        }
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
        let p = Placement {
            position: Position { x: 5, y: 2 },
            rotation: Rotation::R0,
        };

        let r = p.apply(&comp, &fp, &b).unwrap();
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
        let p = Placement {
            position: Position { x: 10, y: 0 },
            rotation: Rotation::R90,
        };

        let r = p.apply(&comp, &fp, &b).unwrap();
        assert_eq!(r.pin_holes[0].hole, b.at(10, 0).unwrap());
        assert_eq!(r.pin_holes[1].hole, b.at(10, 1).unwrap());
        assert_eq!(r.pin_holes[2].hole, b.at(10, 2).unwrap());
    }

    #[test]
    fn apply_r90_at_top_row_out_of_bounds() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let p = Placement {
            position: Position { x: 0, y: 4 },
            rotation: Rotation::R90,
        };

        let r = p.apply(&comp, &fp, &b);
        assert!(matches!(r, Err(LayoutError::OutOfBounds { .. })));
    }

    #[test]
    fn apply_out_of_bounds_includes_component_and_pin() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let p = Placement {
            position: Position { x: 0, y: 4 },
            rotation: Rotation::R90,
        };
        // R90: (x, y) → (-y, x)
        // pin 0: (0,0) → (0,0) 绝对 (0,4) ✓
        // pin 1: (1,0) → (0,1) 绝对 (0,5) ✗ 越界 (row 5)
        // pin 2: (2,0) → (0,2) 绝对 (0,6) ✗ 越界
        // zip 顺序遍历, pin 1 先失败
        let r = p.apply(&comp, &fp, &b);
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
        let p = Placement {
            position: Position { x: 10, y: 2 },
            rotation: Rotation::R180,
        };

        let r = p.apply(&comp, &fp, &b).unwrap();
        assert_eq!(r.pin_holes[0].hole, b.at(10, 2).unwrap());
        assert_eq!(r.pin_holes[1].hole, b.at(9, 2).unwrap());
        assert_eq!(r.pin_holes[2].hole, b.at(8, 2).unwrap());
    }

    #[test]
    fn occupied_holes_iter() {
        let b = board();
        let fp = to92_footprint();
        let comp = q1_component();
        let p = Placement {
            position: Position { x: 1, y: 1 },
            rotation: Rotation::R0,
        };

        let r = p.apply(&comp, &fp, &b).unwrap();
        let occ: Vec<HoleId> = r.occupied_holes().collect();
        assert_eq!(occ.len(), 3);
    }
}
