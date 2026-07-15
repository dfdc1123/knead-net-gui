//! 模拟退火的不可变输入：调用前已经存在的 placement、wire 与板载固定几何。

use std::collections::{HashMap, HashSet};

use crate::circuit::{NetId, Position};

use super::breadboard::Breadboard;
use super::placement::BBox;
use super::{Layout, LayoutError};

#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedEndpoint {
    pub position: Position,
    pub effective_rail: u32,
    pub net: Option<NetId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FixedWireGeometry {
    pub net: NetId,
    pub from_rail: u32,
    pub to_rail: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PlacedGeometry {
    /// 所有不能再插 pin / body 的格点：固定元件 bbox、wire 端点和 RailTie 端点。
    pub occupied_cells: HashSet<(i32, i32)>,
    /// 固定 OnBoard 与 Bridged 的完整 body bbox。
    pub bboxes: Vec<BBox>,
    /// 固定元件 pin 与已有 wire 端点；参与 MST、rail owner 和 pin collision。
    pub endpoints: Vec<FixedEndpoint>,
    /// wire / RailTie 端点的点状障碍；参与 movable bbox collision，但不参与 compactness。
    pub point_obstacles: Vec<BBox>,
    /// 每个已占 effective rail 的期望/实际 owner，供初始化 hard-legality 使用。
    pub rail_owners: HashMap<u32, Option<NetId>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AnnealProblem {
    pub fixed_geometry: PlacedGeometry,
    pub fixed_wires: Vec<FixedWireGeometry>,
    /// `(net, physical effective rail) -> 该 net 经已有 wire 闭包后的代表 rail`。
    wire_rail_representatives: HashMap<(NetId, u32), u32>,
}

impl AnnealProblem {
    pub fn from_layout(layout: &Layout<'_>, board: &Breadboard) -> Result<Self, Vec<LayoutError>> {
        // R2 的 public legality 是这里可安全提取固定几何的前置条件。
        layout.validate(board)?;

        let mut fixed_geometry = PlacedGeometry::default();

        for (anchor, net) in board.bound_power_rail_anchors() {
            fixed_geometry
                .rail_owners
                .insert(board.effective_rail_id_of(anchor), Some(net));
        }

        for tie in board.rail_ties() {
            for hole in tie.contacts() {
                let position = board.hole(hole).position;
                fixed_geometry
                    .occupied_cells
                    .insert((position.x, position.y));
                fixed_geometry.point_obstacles.push(point_bbox(position));
            }
        }

        for (index, placement) in layout.placements().iter().enumerate() {
            let Some(placement) = placement else { continue };
            let component = &layout.circuit().components()[index];
            let footprint = &layout.circuit().footprints()[component
                .footprint()
                .expect("validate 已保证 footprint")
                .raw()];
            let placed = placement
                .apply(component, footprint, board, layout.circuit().pins())
                .expect("validate 已保证 placement 可应用");

            for pin_hole in placed.pin_holes {
                let position = board.hole(pin_hole.hole).position;
                let effective_rail = board.effective_rail_id_of(pin_hole.hole);
                let net = layout.circuit().pins()[pin_hole.pin.raw()].net();
                fixed_geometry.endpoints.push(FixedEndpoint {
                    position,
                    effective_rail,
                    net,
                });
                fixed_geometry
                    .rail_owners
                    .entry(effective_rail)
                    .or_insert(net);
            }
            if let Some(bbox) = placed.bbox {
                fixed_geometry.bboxes.push(bbox);
                fixed_geometry
                    .occupied_cells
                    .extend(bbox.iter_cells().map(|p| (p.x, p.y)));
            }
        }

        let mut fixed_wires = Vec::with_capacity(layout.wires().len());
        for wire in layout.wires() {
            let from = board.hole(wire.from).position;
            let to = board.hole(wire.to).position;
            let from_rail = board.effective_rail_id_of(wire.from);
            let to_rail = board.effective_rail_id_of(wire.to);
            for (position, effective_rail) in [(from, from_rail), (to, to_rail)] {
                fixed_geometry
                    .occupied_cells
                    .insert((position.x, position.y));
                fixed_geometry.point_obstacles.push(point_bbox(position));
                fixed_geometry.endpoints.push(FixedEndpoint {
                    position,
                    effective_rail,
                    net: Some(wire.net),
                });
                fixed_geometry
                    .rail_owners
                    .entry(effective_rail)
                    .or_insert(Some(wire.net));
            }
            fixed_wires.push(FixedWireGeometry {
                net: wire.net,
                from_rail,
                to_rail,
            });
        }

        let wire_rail_representatives = build_wire_rail_representatives(&fixed_wires);
        Ok(Self {
            fixed_geometry,
            fixed_wires,
            wire_rail_representatives,
        })
    }

    pub fn mst_rail(&self, net: Option<NetId>, physical_rail: u32) -> u32 {
        net.and_then(|net| {
            self.wire_rail_representatives
                .get(&(net, physical_rail))
                .copied()
        })
        .unwrap_or(physical_rail)
    }
}

fn point_bbox(position: Position) -> BBox {
    BBox {
        min_x: position.x,
        max_x: position.x,
        min_y: position.y,
        max_y: position.y,
    }
}

fn build_wire_rail_representatives(wires: &[FixedWireGeometry]) -> HashMap<(NetId, u32), u32> {
    let mut by_net: HashMap<NetId, Vec<(u32, u32)>> = HashMap::new();
    for wire in wires {
        by_net
            .entry(wire.net)
            .or_default()
            .push((wire.from_rail, wire.to_rail));
    }

    let mut result = HashMap::new();
    for (net, edges) in by_net {
        let mut rails: Vec<u32> = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
        rails.sort_unstable();
        rails.dedup();
        let index: HashMap<u32, usize> = rails
            .iter()
            .enumerate()
            .map(|(index, &rail)| (rail, index))
            .collect();
        let mut parent: Vec<usize> = (0..rails.len()).collect();
        for (a, b) in edges {
            let root_a = find_root(&mut parent, index[&a]);
            let root_b = find_root(&mut parent, index[&b]);
            if root_a != root_b {
                parent[root_b] = root_a;
            }
        }
        let mut minimum_by_root: HashMap<usize, u32> = HashMap::new();
        for (idx, &rail) in rails.iter().enumerate() {
            let root = find_root(&mut parent, idx);
            minimum_by_root
                .entry(root)
                .and_modify(|minimum| *minimum = (*minimum).min(rail))
                .or_insert(rail);
        }
        for (idx, &rail) in rails.iter().enumerate() {
            let root = find_root(&mut parent, idx);
            result.insert((net, rail), minimum_by_root[&root]);
        }
    }
    result
}

fn find_root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, PhysicalPin, Pin, PinId,
    };
    use crate::layout::cost::{SAState, Weights, cost_with_problem};
    use crate::layout::{Placement, Rotation, Wire, WireId};

    fn two_testpoint_circuit() -> &'static Circuit {
        Box::leak(Box::new(Circuit {
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
                    net: Some(NetId(0)),
                })
                .collect(),
            nets: vec![Net {
                id: NetId(0),
                name: "N".into(),
                pins: vec![PinId(0), PinId(1)],
            }],
            footprints: vec![Footprint {
                id: FootprintId(0),
                name: "1p".into(),
                pins: vec![PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                }],
            }],
        }))
    }

    fn mst_only() -> Weights {
        Weights {
            mst: 1.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            out_of_bounds: 0.0,
            compactness: 0.0,
            rail_crossing: 0.0,
            row_squash: 0.0,
            mst_congestion: 0.0,
        }
    }

    #[test]
    fn fixed_onboard_pin_participates_in_mst() {
        let circuit = two_testpoint_circuit();
        let board = Breadboard::new(4, 1);
        let mut layout = Layout::new(circuit);
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 0, y: 0 },
                rotation: Rotation::R0,
            },
        );
        let problem = AnnealProblem::from_layout(&layout, &board).unwrap();
        let state = SAState {
            placeable: vec![ComponentId(1)],
            x: vec![3],
            y: vec![0],
            rotation: vec![Rotation::R0],
            ..SAState::no_bridging(1)
        };

        assert_eq!(
            cost_with_problem(
                &state,
                circuit,
                &board,
                &AnnealProblem::default(),
                &mst_only()
            ),
            0.0
        );
        assert_eq!(
            cost_with_problem(&state, circuit, &board, &problem, &mst_only()),
            3.0
        );
    }

    #[test]
    fn existing_wire_contracts_its_two_rails_for_the_same_net() {
        let circuit = two_testpoint_circuit();
        let board = Breadboard::new(4, 2);
        let mut layout = Layout::new(circuit);
        layout.add_wire(Wire {
            id: WireId(0),
            net: NetId(0),
            from: board.at(0, 0).unwrap(),
            to: board.at(3, 0).unwrap(),
        });
        let problem = AnnealProblem::from_layout(&layout, &board).unwrap();
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 3],
            y: vec![1, 1],
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };

        assert_eq!(
            cost_with_problem(
                &state,
                circuit,
                &board,
                &AnnealProblem::default(),
                &mst_only()
            ),
            3.0
        );
        assert_eq!(
            cost_with_problem(&state, circuit, &board, &problem, &mst_only()),
            0.0
        );
    }
}
