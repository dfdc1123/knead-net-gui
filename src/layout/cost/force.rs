//! Breadboard-aware initializer hints: discrete main-region assignment plus 1-D force relaxation.

use std::collections::HashMap;

use crate::circuit::{Circuit, ComponentId};
use crate::layout::breadboard::Breadboard;
use crate::layout::placement::{Rotation, rotate};
use crate::layout::preprocess::PreprocessResult;

use super::legalize::PlacementHints;

const FORCE_STEPS: usize = 120;

pub(super) fn force_hints(
    placeable: &[ComponentId],
    circuit: &Circuit,
    board: &Breadboard,
    preprocess: &PreprocessResult,
    seed: u64,
) -> PlacementHints {
    let n = placeable.len();
    let nets = active_nets(placeable, circuit);
    let adjacency = adjacency(n, &nets);
    let row_groups = main_row_groups(board);
    let locked_regions: Vec<Option<usize>> = placeable
        .iter()
        .map(|component| {
            preprocess
                .y_locked
                .get(component)
                .and_then(|row| row_groups.iter().position(|rows| rows.contains(row)))
        })
        .collect();
    let regions = assign_regions(&adjacency, &locked_regions, row_groups.len(), seed);
    let widths: Vec<f64> = placeable
        .iter()
        .map(|component| footprint_width(*component, circuit, preprocess) as f64)
        .collect();
    let x = relax_horizontal(&nets, &regions, &widths, board.cols(), seed);
    let target_x: Vec<i32> = x
        .into_iter()
        .map(|value| value.round().clamp(0.0, (board.cols() - 1) as f64) as i32)
        .collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| {
        target_x[*a]
            .cmp(&target_x[*b])
            .then_with(|| widths[*b].total_cmp(&widths[*a]))
            .then_with(|| a.cmp(b))
    });
    let row_preferences = placeable
        .iter()
        .enumerate()
        .map(|(idx, component)| {
            if let Some(row) = preprocess.y_locked.get(component) {
                return vec![*row];
            }
            let mut rows = Vec::with_capacity(board.main_rows());
            if let Some(primary) = row_groups.get(regions[idx]) {
                append_rotated(&mut rows, primary, mixed_index(seed, idx, primary.len()));
            }
            for (region, group) in row_groups.iter().enumerate() {
                if region != regions[idx] {
                    append_rotated(&mut rows, group, mixed_index(seed, idx, group.len()));
                }
            }
            rows
        })
        .collect();

    PlacementHints {
        order,
        target_x,
        row_preferences,
    }
}

fn active_nets(placeable: &[ComponentId], circuit: &Circuit) -> Vec<(Vec<usize>, f64)> {
    let indices: HashMap<ComponentId, usize> = placeable
        .iter()
        .enumerate()
        .map(|(idx, component)| (*component, idx))
        .collect();
    circuit
        .nets()
        .iter()
        .filter_map(|net| {
            let mut components: Vec<usize> = net
                .pins()
                .iter()
                .filter_map(|pin| indices.get(&circuit.pins()[pin.raw()].component()))
                .copied()
                .collect();
            components.sort_unstable();
            components.dedup();
            (components.len() >= 2).then(|| {
                let weight = 1.0 / (components.len() - 1) as f64;
                (components, weight)
            })
        })
        .collect()
}

fn adjacency(n: usize, nets: &[(Vec<usize>, f64)]) -> Vec<Vec<(usize, f64)>> {
    let mut weights = vec![vec![0.0; n]; n];
    for (components, weight) in nets {
        for (offset, &a) in components.iter().enumerate() {
            for &b in &components[offset + 1..] {
                weights[a][b] += weight;
                weights[b][a] += weight;
            }
        }
    }
    weights
        .into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .filter_map(|(idx, weight)| (weight > 0.0).then_some((idx, weight)))
                .collect()
        })
        .collect()
}

fn assign_regions(
    adjacency: &[Vec<(usize, f64)>],
    locked: &[Option<usize>],
    region_count: usize,
    seed: u64,
) -> Vec<usize> {
    if region_count <= 1 {
        return vec![0; adjacency.len()];
    }
    let mut assigned = locked.to_vec();
    let mut counts = vec![0usize; region_count];
    for region in assigned.iter().flatten() {
        counts[*region] += 1;
    }
    let mut order: Vec<usize> = (0..adjacency.len())
        .filter(|idx| assigned[*idx].is_none())
        .collect();
    order.sort_by(|a, b| {
        let degree_a: f64 = adjacency[*a].iter().map(|(_, weight)| weight).sum();
        let degree_b: f64 = adjacency[*b].iter().map(|(_, weight)| weight).sum();
        degree_b
            .total_cmp(&degree_a)
            .then_with(|| mixed(seed, *a).cmp(&mixed(seed, *b)))
    });
    let capacity = adjacency.len().div_ceil(region_count);
    for idx in order {
        let region = (0..region_count)
            .filter(|region| counts[*region] < capacity)
            .max_by(|a, b| {
                affinity(idx, *a, adjacency, &assigned, counts[*a])
                    .total_cmp(&affinity(idx, *b, adjacency, &assigned, counts[*b]))
                    .then_with(|| b.cmp(a))
            })
            .unwrap_or_else(|| {
                counts
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, count)| **count)
                    .map(|(region, _)| region)
                    .unwrap_or(0)
            });
        assigned[idx] = Some(region);
        counts[region] += 1;
    }
    assigned
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

fn affinity(
    idx: usize,
    region: usize,
    adjacency: &[Vec<(usize, f64)>],
    assigned: &[Option<usize>],
    count: usize,
) -> f64 {
    adjacency[idx]
        .iter()
        .filter(|(other, _)| assigned[*other] == Some(region))
        .map(|(_, weight)| weight)
        .sum::<f64>()
        - count as f64 * 0.08
}

fn relax_horizontal(
    nets: &[(Vec<usize>, f64)],
    regions: &[usize],
    widths: &[f64],
    cols: usize,
    seed: u64,
) -> Vec<f64> {
    let usable = cols.saturating_sub(1) as f64;
    let mut x: Vec<f64> = (0..widths.len())
        .map(|idx| mixed(seed, idx) as f64 / u64::MAX as f64 * usable)
        .collect();
    for step in 0..FORCE_STEPS {
        let mut force = vec![0.0; x.len()];
        for (components, weight) in nets {
            let centroid =
                components.iter().map(|idx| x[*idx]).sum::<f64>() / components.len() as f64;
            for idx in components {
                force[*idx] += (centroid - x[*idx]) * weight * 0.18;
            }
        }
        for a in 0..x.len() {
            for b in a + 1..x.len() {
                if regions[a] != regions[b] {
                    continue;
                }
                let required = (widths[a] + widths[b]) * 0.5 + 1.0;
                let delta = x[b] - x[a];
                let distance = delta.abs();
                if distance < required {
                    let direction = if distance < 1e-9 {
                        if mixed(seed, a) < mixed(seed, b) {
                            -1.0
                        } else {
                            1.0
                        }
                    } else {
                        delta.signum()
                    };
                    let push = (required - distance) * 0.35;
                    force[a] -= direction * push;
                    force[b] += direction * push;
                }
            }
        }
        let rate = 0.08 + (1.0 - step as f64 / FORCE_STEPS as f64) * 0.22;
        for idx in 0..x.len() {
            force[idx] -= x[idx] * 0.002;
            x[idx] = (x[idx] + force[idx] * rate).clamp(0.0, usable);
        }
    }
    let left = x.iter().copied().fold(f64::INFINITY, f64::min);
    if left.is_finite() {
        for value in &mut x {
            *value -= left;
        }
    }
    x
}

fn footprint_width(
    component: ComponentId,
    circuit: &Circuit,
    preprocess: &PreprocessResult,
) -> i32 {
    let component = &circuit.components()[component.raw()];
    let footprint =
        &circuit.footprints()[component.footprint().expect("placeable footprint").raw()];
    let rotation = if preprocess.r90_only.contains(&component.id()) {
        Rotation::R90
    } else {
        Rotation::R0
    };
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    for pin in footprint.pins() {
        let x = rotate(pin.offset(), rotation).x;
        min_x = min_x.min(x);
        max_x = max_x.max(x);
    }
    if min_x > max_x { 1 } else { max_x - min_x + 1 }
}

fn main_row_groups(board: &Breadboard) -> Vec<Vec<i32>> {
    let mut groups = Vec::<Vec<i32>>::new();
    for row in 0..board.main_rows() {
        if board.is_blocked(row) {
            continue;
        }
        if groups.is_empty() || (row > 0 && board.is_blocked(row - 1)) {
            groups.push(Vec::new());
        }
        groups.last_mut().expect("row group").push(row as i32);
    }
    groups
}

fn append_rotated(out: &mut Vec<i32>, rows: &[i32], offset: usize) {
    if !rows.is_empty() {
        out.extend((0..rows.len()).map(|idx| rows[(idx + offset) % rows.len()]));
    }
}

fn mixed_index(seed: u64, idx: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        mixed(seed, idx) as usize % len
    }
}

fn mixed(seed: u64, idx: usize) -> u64 {
    let mut value = seed ^ (idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_assignment_is_balanced_and_keeps_connected_pairs_together() {
        let adjacency = vec![
            vec![(1, 2.0)],
            vec![(0, 2.0)],
            vec![(3, 2.0)],
            vec![(2, 2.0)],
        ];
        let regions = assign_regions(&adjacency, &[None; 4], 2, 7);
        assert_eq!(regions.iter().filter(|region| **region == 0).count(), 2);
        assert_eq!(regions.iter().filter(|region| **region == 1).count(), 2);
        assert_eq!(regions[0], regions[1]);
        assert_eq!(regions[2], regions[3]);
    }

    #[test]
    fn horizontal_force_is_seed_reproducible_and_separates_bodies() {
        let nets = vec![(vec![0, 1], 1.0)];
        let first = relax_horizontal(&nets, &[0, 0], &[4.0, 4.0], 30, 11);
        let second = relax_horizontal(&nets, &[0, 0], &[4.0, 4.0], 30, 11);
        assert_eq!(first, second);
        assert!(
            (first[0] - first[1]).abs() >= 3.5,
            "force relaxation collapsed the bodies: {first:?}"
        );
    }
}
