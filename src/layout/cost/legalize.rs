//! Shared legalization for continuous/ordered initializer hints.

use crate::circuit::{Circuit, ComponentId};
use crate::layout::LayoutError;
use crate::layout::breadboard::Breadboard;
use crate::layout::placement::Rotation;
use crate::layout::preprocess::PreprocessResult;
use crate::layout::problem::AnnealProblem;

use super::state::{InitialGeometry, InitialOccupancy, SAState};
use super::{Weights, cost_with_problem};

const BEAM_WIDTH: usize = 12;
const CANDIDATES_PER_BRANCH: usize = 6;
const ROWS_PER_X: usize = 2;

pub(super) struct PlacementHints {
    pub order: Vec<usize>,
    pub target_x: Vec<i32>,
    pub row_preferences: Vec<Vec<i32>>,
}

#[derive(Clone)]
struct PartialPlacement {
    occupancy: InitialOccupancy,
    x: Vec<i32>,
    y: Vec<i32>,
    hint_penalty: f64,
}

pub(super) fn legalize(
    placeable: Vec<ComponentId>,
    circuit: &Circuit,
    board: &Breadboard,
    preprocess: &PreprocessResult,
    problem: &AnnealProblem,
    weights: &Weights,
    hints: PlacementHints,
) -> Result<SAState, LayoutError> {
    let n = placeable.len();
    debug_assert_eq!(hints.order.len(), n);
    debug_assert_eq!(hints.target_x.len(), n);
    debug_assert_eq!(hints.row_preferences.len(), n);
    if n == 0 {
        return Ok(assemble_state(
            placeable,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
    }

    let rotation: Vec<Rotation> = placeable
        .iter()
        .map(|component| preferred_rotation(*component, preprocess))
        .collect();
    let r90_only = placeable
        .iter()
        .map(|component| preprocess.r90_only.contains(component))
        .collect();
    let y_locked: Vec<Option<i32>> = placeable
        .iter()
        .map(|component| preprocess.y_locked.get(component).copied())
        .collect();
    let mut beam = vec![PartialPlacement {
        occupancy: InitialOccupancy::new(problem),
        x: vec![0; n],
        y: vec![0; n],
        hint_penalty: 0.0,
    }];

    for &idx in &hints.order {
        let component = &circuit.components[placeable[idx].raw()];
        let geometry = InitialGeometry::new(component, circuit, rotation[idx]);
        let allow_channel_crossing = y_locked[idx].is_some();
        let mut next = Vec::new();
        for partial in &beam {
            let mut accepted = 0usize;
            'positions: for dx in 0..=board.cols() as i32 {
                for x_sign in [1, -1] {
                    if dx == 0 && x_sign == -1 {
                        continue;
                    }
                    let x = hints.target_x[idx] + x_sign * dx;
                    if !(0..board.cols() as i32).contains(&x) {
                        continue;
                    }
                    let mut accepted_at_x = 0usize;
                    for (row_rank, &y) in hints.row_preferences[idx].iter().enumerate() {
                        let mut candidate = partial.clone();
                        if !candidate.occupancy.try_reserve(
                            board,
                            &geometry,
                            x,
                            y,
                            allow_channel_crossing,
                        ) {
                            continue;
                        }
                        candidate.x[idx] = x;
                        candidate.y[idx] = y;
                        candidate.hint_penalty +=
                            dx as f64 + row_rank as f64 * 0.25 + x as f64 * 0.01;
                        next.push(candidate);
                        accepted += 1;
                        accepted_at_x += 1;
                        if accepted >= CANDIDATES_PER_BRANCH {
                            break 'positions;
                        }
                        if accepted_at_x >= ROWS_PER_X {
                            break;
                        }
                    }
                }
            }
        }

        if next.is_empty() {
            return first_fit(
                placeable, circuit, board, problem, rotation, r90_only, y_locked,
            );
        }
        next.sort_by(|a, b| a.hint_penalty.total_cmp(&b.hint_penalty));
        next.truncate(BEAM_WIDTH);
        beam = next;
    }

    beam.into_iter()
        .map(|candidate| {
            let state = assemble_state(
                placeable.clone(),
                candidate.x,
                candidate.y,
                rotation.clone(),
                r90_only.clone(),
                y_locked.clone(),
            );
            let cost = cost_with_problem(&state, circuit, board, problem, weights);
            (cost, state)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, state)| state)
        .ok_or(LayoutError::NoLegalInitialPlacement {
            component: placeable[0],
        })
}

fn first_fit(
    placeable: Vec<ComponentId>,
    circuit: &Circuit,
    board: &Breadboard,
    problem: &AnnealProblem,
    rotation: Vec<Rotation>,
    r90_only: Vec<bool>,
    y_locked: Vec<Option<i32>>,
) -> Result<SAState, LayoutError> {
    let mut occupancy = InitialOccupancy::new(problem);
    let mut x = vec![0; placeable.len()];
    let mut y = vec![0; placeable.len()];
    for idx in 0..placeable.len() {
        let geometry = InitialGeometry::new(
            &circuit.components[placeable[idx].raw()],
            circuit,
            rotation[idx],
        );
        let rows: Vec<i32> =
            y_locked[idx].map_or_else(|| (0..board.main_rows() as i32).collect(), |row| vec![row]);
        let found = rows.into_iter().find_map(|row| {
            (0..board.cols() as i32).find_map(|col| {
                occupancy
                    .try_reserve(board, &geometry, col, row, y_locked[idx].is_some())
                    .then_some((col, row))
            })
        });
        let (col, row) = found.ok_or(LayoutError::NoLegalInitialPlacement {
            component: placeable[idx],
        })?;
        x[idx] = col;
        y[idx] = row;
    }
    Ok(assemble_state(
        placeable, x, y, rotation, r90_only, y_locked,
    ))
}

fn assemble_state(
    placeable: Vec<ComponentId>,
    x: Vec<i32>,
    y: Vec<i32>,
    rotation: Vec<Rotation>,
    r90_only: Vec<bool>,
    y_locked: Vec<Option<i32>>,
) -> SAState {
    let n = placeable.len();
    SAState {
        placeable,
        is_bridgeable: vec![false; n],
        bridged: vec![false; n],
        bridged_pin_pairs: vec![Vec::new(); n],
        active_bridge_idx: vec![0; n],
        x,
        y,
        rotation,
        r90_only,
        y_locked,
    }
}

fn preferred_rotation(component: ComponentId, preprocess: &PreprocessResult) -> Rotation {
    if preprocess.r90_only.contains(&component) {
        Rotation::R90
    } else {
        Rotation::R0
    }
}
