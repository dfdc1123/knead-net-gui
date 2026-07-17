//! Routed-net connectivity and pre-routing port-capacity validation.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::circuit::{ComponentId, NetId};

use super::{Breadboard, Layout, LayoutError, Occupancy};

#[derive(Debug, Default)]
struct RailDsu {
    parent: HashMap<u32, u32>,
}

impl RailDsu {
    fn add(&mut self, rail: u32) {
        self.parent.entry(rail).or_insert(rail);
    }

    fn find(&mut self, rail: u32) -> u32 {
        self.add(rail);
        let parent = self.parent[&rail];
        if parent == rail {
            rail
        } else {
            let root = self.find(parent);
            self.parent.insert(rail, root);
            root
        }
    }

    fn union(&mut self, a: u32, b: u32) {
        let a = self.find(a);
        let b = self.find(b);
        if a != b {
            self.parent.insert(a.max(b), a.min(b));
        }
    }
}

#[derive(Debug)]
struct NetTopology {
    rails: Vec<BTreeSet<u32>>,
}

fn net_topologies(
    layout: &Layout<'_>,
    board: &Breadboard,
) -> Result<Vec<NetTopology>, Vec<LayoutError>> {
    let circuit = layout.circuit();
    let mut rails_by_net = vec![BTreeSet::<u32>::new(); circuit.nets().len()];
    let mut dsu_by_net: Vec<RailDsu> = (0..circuit.nets().len())
        .map(|_| RailDsu::default())
        .collect();
    let mut duplicate_pads = HashMap::<(ComponentId, NetId, String), Vec<u32>>::new();

    for component in circuit.components() {
        let Some(placement) = layout.placement(component.id()) else {
            continue;
        };
        let Some(footprint_id) = component.footprint() else {
            continue;
        };
        let footprint = &circuit.footprints()[footprint_id.raw()];
        let placed = match placement.apply(component, footprint, board, circuit.pins()) {
            Ok(placed) => placed,
            Err(error) => return Err(vec![error]),
        };
        for pin_hole in placed.pin_holes {
            let pin = &circuit.pins()[pin_hole.pin.raw()];
            let Some(net) = pin.net() else { continue };
            let rail = board.effective_rail_id_of(pin_hole.hole);
            rails_by_net[net.raw()].insert(rail);
            dsu_by_net[net.raw()].add(rail);
            duplicate_pads
                .entry((component.id(), net, pin.num().to_string()))
                .or_default()
                .push(rail);
        }
    }

    for ((_, net, _), rails) in duplicate_pads {
        if let Some((&first, rest)) = rails.split_first() {
            for &rail in rest {
                dsu_by_net[net.raw()].union(first, rail);
            }
        }
    }
    for (anchor, net) in board.bound_power_rail_anchors() {
        let rail = board.effective_rail_id_of(anchor);
        rails_by_net[net.raw()].insert(rail);
        dsu_by_net[net.raw()].add(rail);
    }
    for wire in layout.wires() {
        let from = board.effective_rail_id_of(wire.from);
        let to = board.effective_rail_id_of(wire.to);
        rails_by_net[wire.net.raw()].extend([from, to]);
        dsu_by_net[wire.net.raw()].union(from, to);
    }

    Ok(rails_by_net
        .into_iter()
        .zip(dsu_by_net)
        .map(|(rails, mut dsu)| {
            let mut groups = BTreeMap::<u32, BTreeSet<u32>>::new();
            for rail in rails {
                groups.entry(dsu.find(rail)).or_default().insert(rail);
            }
            NetTopology {
                rails: groups.into_values().collect(),
            }
        })
        .collect())
}

pub(super) fn validate_routed_connectivity(
    layout: &Layout<'_>,
    board: &Breadboard,
) -> Result<(), Vec<LayoutError>> {
    let topologies = net_topologies(layout, board)?;
    let errors = topologies
        .iter()
        .enumerate()
        .filter(|(_, topology)| topology.rails.len() > 1)
        .map(|(net, topology)| LayoutError::DisconnectedNet {
            net: NetId(net),
            connected_groups: topology.rails.len(),
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn available_ports(occupancy: &Occupancy, board: &Breadboard, rails: &BTreeSet<u32>) -> usize {
    board
        .holes()
        .iter()
        .filter(|hole| rails.contains(&board.effective_rail_id_of(hole.id)))
        .filter(|hole| occupancy.can_place_pin(hole.id))
        .count()
}

pub(super) fn validate_routing_ports(
    layout: &Layout<'_>,
    board: &Breadboard,
) -> Result<(), Vec<LayoutError>> {
    let occupancy = layout.occupancy(board)?;
    let topologies = net_topologies(layout, board)?;
    let mut errors = Vec::new();
    for (net, topology) in topologies.iter().enumerate() {
        if topology.rails.len() <= 1 {
            continue;
        }
        let capacities = topology
            .rails
            .iter()
            .map(|rails| (rails, available_ports(&occupancy, board, rails)))
            .collect::<Vec<_>>();
        for (rails, available) in &capacities {
            if *available == 0 {
                errors.push(LayoutError::InsufficientRoutingPorts {
                    net: NetId(net),
                    effective_rail: rails.first().copied(),
                    available: 0,
                    required: 1,
                });
            }
        }
        let available = capacities
            .iter()
            .map(|(_, capacity)| capacity)
            .sum::<usize>();
        let required = 2 * topology.rails.len().saturating_sub(1);
        if available < required && !capacities.iter().any(|(_, capacity)| *capacity == 0) {
            errors.push(LayoutError::InsufficientRoutingPorts {
                net: NetId(net),
                effective_rail: None,
                available,
                required,
            });
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsu_uses_a_stable_minimum_representative() {
        let mut dsu = RailDsu::default();
        dsu.union(9, 4);
        dsu.union(7, 9);
        assert_eq!(dsu.find(4), 4);
        assert_eq!(dsu.find(7), 4);
        assert_eq!(dsu.find(9), 4);
    }
}
