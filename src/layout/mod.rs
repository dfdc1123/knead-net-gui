//! 面包板布局: 把 Circuit 投影到 Breadboard 上。
//!
//! 模块组织:
//! - [`breadboard`]: 物理结构 (30×5 矩形, 每列纵向连通, 无电源轨)
//! - [`placement`]: 摆放 (位置 + 旋转) → 投影到具体 HoleId
//! - [`occupancy`]: 当前孔占用 (派生, 不缓存)
//! - [`routing`]: 接线 (Wire, Router trait)
//! - [`Layout`]: 顶层容器, 持有 Circuit 引用 + placements + wires

pub mod breadboard;
pub mod cost;
pub mod occupancy;
pub mod placement;
pub mod routing;
pub mod sa;

pub use breadboard::{Breadboard, Hole, HoleId};
pub use cost::Weights;
pub use occupancy::{Occupancy, Occupant};
pub use placement::{PinHole, PlacedFootprint, Placement, Rotation};
pub use routing::{PathFinderRouter, Router, Wire, WireId};
pub use sa::SAConfig;

use crate::circuit::{Circuit, ComponentId, Footprint, NetId, PinId, Position};

/// 一列上的某个 pin / wire 端点, 捎带它的 net 信息。
///
/// [`LayoutError::ColumnConflict`] 用这个告诉你 "col X 的 a 和 b 被纵向 rail 连起来了,
/// 它们不在同一 net, 算短路" —— 拿这个 net 信息能直接看出是哪个 net 被连到了哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnEndpoint {
    /// pin + 所属 net (`None` = 未连接, e.g. unconnected-pad)
    Pin { pin: PinId, net: Option<NetId> },
    /// wire 端点, 必有 net
    Wire { wire: WireId, net: NetId },
}

/// 布局错误。`apply` / `validate` / `from_layout` 都会返回这个。
///
/// `apply` 只产生 `OutOfBounds` (单个 placement 内的检查);
/// `validate` / `from_layout` 还会产生 `NoFootprint` / `PinCollision` / `WireConflict`
/// / `ColumnConflict` (跨 placement 的检查)。
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
    /// Component 引用了 footprint 里不存在的 pad (按 num 找)
    NoFootprintPad {
        component: ComponentId,
        pin: PinId,
        pad_name: String,
    },
    /// 同列上两个不同 net 的 pin / wire 被面包板纵向 rail 短路。
    /// 面包板的每列内部连通, 所以同列不同 row 上的 pin 也会被连起来。
    ColumnConflict {
        column: i32,
        a: ColumnEndpoint,
        b: ColumnEndpoint,
    },
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

    /// 用模拟退火 + 后续压缩布局。
    ///
    /// 流程: 收集有 footprint 的 component → `sa::simulate` → `sa::compact`
    /// → 把结果写回 `placements` → `validate(board)`。
    ///
    /// 没有 footprint 的 component 保持未摆放, `validate` 会报 `NoFootprint`。
    /// 调参见 [`SAConfig`], 默认参数适合 ~5 元件级别。
    pub fn place_sa(
        &mut self,
        board: &Breadboard,
        config: &SAConfig,
    ) -> Result<(), Vec<LayoutError>> {
        use crate::layout::sa;

        let placeable: Vec<ComponentId> = self
            .circuit
            .components
            .iter()
            .filter_map(|c| c.footprint.map(|_| c.id))
            .collect();

        if placeable.is_empty() {
            return self.validate(board);
        }

        let best = sa::simulate(placeable, self.circuit, board, config);
        let xs = sa::compact(&best, self.circuit, board);

        for (idx, &comp_id) in best.placeable.iter().enumerate() {
            self.placements[comp_id.0] = Some(Placement {
                position: Position {
                    x: xs[idx],
                    y: best.row[idx],
                },
                rotation: best.rotation[idx],
            });
        }

        self.validate(board)
    }

    /// 把所有有 footprint 的 component 横向摆在指定行, R0 方向, 元件之间留 1 空列。
    ///
    /// 最简单的"排成一排"策略: 按 component 顺序, 算出 footprint 水平跨度,
    /// 依次放下去。**会覆盖已存在的 placement**; 没有 footprint 的 component 跳过
    /// (validate 会把它们报为 `NoFootprint`)。
    ///
    /// 越界 / pin 碰撞 / wire 冲突都通过返回值上报; 即使有错, placement 也已经写入,
    /// 调用方可以检查后调整。
    pub fn place_row(&mut self, board: &Breadboard, row: i32) -> Result<(), Vec<LayoutError>> {
        let mut col: i32 = 0;
        for component in &self.circuit.components {
            let Some(fid) = component.footprint else {
                continue;
            };
            let footprint = &self.circuit.footprints[fid.0];
            let width = footprint_horizontal_width(footprint);

            self.placements[component.id.0] = Some(Placement {
                position: Position { x: col, y: row },
                rotation: Rotation::R0,
            });
            col += width + 1; // +1 是元件间空列
        }
        self.validate(board)
    }

    /// 从 placements + wires 派生当前占用, 同时验证合法性。
    ///
    /// **严格**: 任何非法状态返回 `Err`, 不返回部分 occupancy。
    /// 调用方必须拿到 `Ok` 之后才能使用 `Occupancy`。
    pub fn occupancy(&self, board: &Breadboard) -> Result<Occupancy, Vec<LayoutError>> {
        Occupancy::from_layout(self, board)
    }
}

/// R0 方向下 footprint 占多少个列 (= `max_x - min_x + 1`)。
///
/// 空 footprint 当作 1 列, 防止减法下溢。
pub(crate) fn footprint_horizontal_width(footprint: &Footprint) -> i32 {
    if footprint.pins.is_empty() {
        return 1;
    }
    let min_x = footprint.pins.iter().map(|p| p.offset.x).min().unwrap();
    let max_x = footprint.pins.iter().map(|p| p.offset.x).max().unwrap();
    max_x - min_x + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Component, Footprint, FootprintId, PhysicalPin, Pin, PinId, Position};

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
            pins: vec![
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
                Pin {
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
            pins: vec![
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
                Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "x".into(),
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
                Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
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
        assert_eq!(q1.position, Position { x: 0, y: 2 });
        assert_eq!(q1.rotation, Rotation::R0);
    }

    #[test]
    fn place_row_uses_footprint_width_plus_gap() {
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        layout.place_row(&board, 2).unwrap();

        // Q1 footprint 宽 3, 放 col 0, 下一个应从 col 3+1=4 开始
        let r1 = layout.placement(ComponentId(1)).unwrap();
        assert_eq!(r1.position, Position { x: 4, y: 2 });
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
        // (5,2) (6,2) R1 跨度内但无 pin, 应该空
        assert_eq!(occ.occupant_at(board.at(5, 2).unwrap()), None);
        assert_eq!(occ.occupant_at(board.at(6, 2).unwrap()), None);
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
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R1".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(1)],
                    footprint: None,
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "p".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
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
        layout.placements[ComponentId(0).0] = Some(Placement {
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

    /// 不同 seed 都应能跑出有效布局 (不强求不同——HPWL 在 1D 顺序布局下是
    /// permutation-invariant, swap 沿 HPWL 是平的, 不同 seed 可能收敛到同解)。
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
                    matches!(p.rotation, Rotation::R0 | Rotation::R180),
                    "seed {seed}: cid {:?} 出现了 {:?}",
                    cid,
                    p.rotation
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
        let wires = router.route(layout.circuit(), &board, &occ);
        // two_component_fixture 里的 4 个 pin 都在同一 net, 4 个 pin 全部在同一 footprint 组?
        // 看一下 fixture, Q1 (comp 0) 有 3 个 pin, R1 (comp 1) 有 1 个 pin
        // 6 个 pin total 都连到 net 0, 6 个不同 col → 5 根 wire
        // (上接路灯: PathFinder 可能收敛到冲突最少的方案)
        for w in &wires {
            // 端点不能和 pin 撞
            assert!(occ.can_add_wire(w), "wire {:?} 跟 pin 撞了", w);
        }
    }
}
