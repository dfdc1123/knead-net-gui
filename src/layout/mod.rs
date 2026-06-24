//! 面包板布局: 把 Circuit 投影到 Breadboard 上。
//!
//! 模块组织:
//! - [`breadboard`]: 物理结构 (30×5 矩形, 每列纵向连通, 无电源轨)
//! - [`placement`]: 摆放 (位置 + 旋转) → 投影到具体 HoleId
//! - [`occupancy`]: 当前孔占用 (派生, 不缓存)
//! - [`routing`]: 接线 (Wire, Router trait)
//! - [`Layout`]: 顶层容器, 持有 Circuit 引用 + placements + wires

pub mod breadboard;
pub mod occupancy;
pub mod placement;
pub mod routing;

pub use breadboard::{Breadboard, Hole, HoleId};
pub use occupancy::{Occupancy, Occupant};
pub use placement::{PinHole, PlacedFootprint, Placement, Rotation};
pub use routing::{Router, Wire, WireId};

use crate::circuit::{Circuit, ComponentId, PinId, Position};

/// 布局错误。`apply` / `validate` / `from_layout` 都会返回这个。
///
/// `apply` 只产生 `OutOfBounds` (单个 placement 内的检查);
/// `validate` / `from_layout` 还会产生 `NoFootprint` / `PinCollision` / `WireConflict`
/// (跨 placement 的检查)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// Component 没有 footprint
    NoFootprint { component: ComponentId },
    /// 某个 pin 算出来落在板外
    OutOfBounds {
        component: ComponentId,
        pin: PinId,
        hole: Position,
    },
    /// pin 跟已摆的元件的 pin 撞同一个孔
    PinCollision {
        component: ComponentId,
        pin: PinId,
        hole: HoleId,
    },
    /// wire path 跟已占用的孔冲突 (跟 pin 或别的 wire)
    WireConflict { wire: WireId, hole: HoleId },
}

/// 顶层布局: 持有 Circuit 引用 + 每个 component 的 placement + 所有 wire。
///
/// 跟 Circuit 本身**解耦**: Component 不携带 placement, Layout 单独管理。
/// 这让 Circuit 可以独立 serialize / 在不同 layout 间切换。
#[derive(Debug)]
pub struct Layout<'c> {
    pub(crate) circuit: &'c Circuit,
    pub(crate) placements: Vec<Option<Placement>>,
    pub(crate) wires: Vec<Wire>,
}

impl<'c> Layout<'c> {
    pub fn new(circuit: &'c Circuit) -> Self {
        Self {
            circuit,
            placements: vec![None; circuit.components.len()],
            wires: Vec::new(),
        }
    }

    /// 摆放 (不验证, 调用方负责确保 placement 合法; 想验证调 `validate`)
    pub fn place(&mut self, component: ComponentId, placement: Placement) {
        self.placements[component.0] = Some(placement);
    }

    pub fn unplace(&mut self, component: ComponentId) {
        self.placements[component.0] = None;
    }

    pub fn placement(&self, component: ComponentId) -> Option<Placement> {
        self.placements[component.0]
    }

    pub fn placements(&self) -> &[Option<Placement>] {
        &self.placements
    }

    pub fn add_wire(&mut self, wire: Wire) {
        self.wires.push(wire);
    }

    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }

    pub fn circuit(&self) -> &Circuit {
        self.circuit
    }

    /// 一次性验证整个 layout, 返回所有错误 (no footprint / 越界 / pin 碰撞 / wire 冲突)。
    ///
    /// `validate` 跟 `occupancy` 走同一条检查路径, 区别是 `validate` 丢掉了
    /// 构建出来的 occupancy 表, 只关心错误。语义上"我只想问合不合法"。
    pub fn validate(&self, board: &Breadboard) -> Result<(), Vec<LayoutError>> {
        self.occupancy(board).map(|_| ())
    }

    /// 从 placements + wires 派生当前占用, 同时验证合法性。
    ///
    /// **严格**: 任何非法状态返回 `Err`, 不返回部分 occupancy。
    /// 调用方必须拿到 `Ok` 之后才能使用 `Occupancy`。
    pub fn occupancy(&self, board: &Breadboard) -> Result<Occupancy, Vec<LayoutError>> {
        Occupancy::from_layout(self, board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Component, Footprint, FootprintId, PhysicalPin, PinId, Position};

    fn fixture() -> &'static Circuit {
        Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "Q1".to_string(),
                kind: "NPN".to_string(),
                value: Some("BC547".to_string()),
                pins: vec![PinId(0), PinId(1), PinId(2)],
                footprint: Some(FootprintId(0)),
            }],
            pins: vec![],
            nets: vec![],
            footprints: vec![Footprint {
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
        let p = Placement {
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
            Placement {
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
            Placement {
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
            Placement {
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
                },
                Component {
                    id: ComponentId(1),
                    ref_: "?".to_string(),
                    kind: "?".to_string(),
                    value: None,
                    pins: vec![PinId(3)],
                    footprint: None,
                },
            ],
            pins: vec![],
            nets: vec![],
            footprints: vec![Footprint {
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
            }],
        }));
        let mut layout = Layout::new(circuit);
        // Q1 越界
        layout.place(
            ComponentId(0),
            Placement {
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R90,
            },
        );
        // ComponentId(1) 也摆上 (没 footprint 也能摆, 验证时才发现问题)
        layout.place(
            ComponentId(1),
            Placement {
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
}
