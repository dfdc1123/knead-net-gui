//! 频谱布局 (Fiedler 向量) 辅助函数。

use fastrand;

use crate::circuit::{Circuit, ComponentId};
use crate::layout::breadboard::Breadboard;
use crate::layout::placement::Rotation;
use crate::layout::preprocess::PreprocessResult;
use crate::layout::problem::AnnealProblem;

use super::state::{InitialGeometry, InitialOccupancy};

/// 幂迭代求 Fiedler 向量
pub(super) fn compute_fiedler(l: &[Vec<f64>], n: usize, seed: u64) -> Vec<f64> {
    let max_deg = (0..n).map(|i| l[i][i]).fold(0.0f64, f64::max);
    let c = if max_deg > 0.0 { 2.0 * max_deg } else { 1.0 };
    let mut rng = fastrand::Rng::with_seed(seed);
    let mut v: Vec<f64> = (0..n).map(|_| rng.f64() - 0.5).collect();
    project_out_constant(&mut v, n);
    if !normalize_vec(&mut v) {
        v = vec![1.0; n];
    }
    for _ in 0..300 {
        let mut w = mat_vec_mul_shifted(l, &v, c, n);
        project_out_constant(&mut w, n);
        if !normalize_vec(&mut w) {
            break;
        }
        v = w;
    }
    v
}

pub(super) fn compute_second_evec(l: &[Vec<f64>], v2: &[f64], n: usize, seed: u64) -> Vec<f64> {
    let max_deg = (0..n).map(|i| l[i][i]).fold(0.0f64, f64::max);
    let c = if max_deg > 0.0 { 2.0 * max_deg } else { 1.0 };
    let mut rng = fastrand::Rng::with_seed(seed.wrapping_add(0x517CC1B7));
    let mut v: Vec<f64> = (0..n).map(|_| rng.f64() - 0.5).collect();
    project_out_two(&mut v, v2, n);
    if !normalize_vec(&mut v) {
        v = vec![1.0; n];
        project_out_two(&mut v, v2, n);
        normalize_vec(&mut v);
    }
    for _ in 0..300 {
        let mut w = mat_vec_mul_shifted(l, &v, c, n);
        project_out_two(&mut w, v2, n);
        if !normalize_vec(&mut w) {
            break;
        }
        v = w;
    }
    v
}

pub(super) fn mat_vec_mul_shifted(l: &[Vec<f64>], v: &[f64], c: f64, n: usize) -> Vec<f64> {
    let mut w = vec![0.0; n];
    for i in 0..n {
        w[i] = c * v[i];
        for j in 0..n {
            w[i] -= l[i][j] * v[j];
        }
    }
    w
}

pub(super) fn project_out_constant(v: &mut [f64], n: usize) {
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    for vi in v.iter_mut() {
        *vi -= mean;
    }
}

pub(super) fn project_out_two(v: &mut [f64], v2: &[f64], n: usize) {
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    let dot_v2: f64 = v.iter().zip(v2).map(|(a, b)| a * b).sum();
    for (i, vi) in v.iter_mut().enumerate() {
        *vi = *vi - mean - dot_v2 * v2[i];
    }
}

pub(super) fn normalize_vec(v: &mut [f64]) -> bool {
    let norm_sq: f64 = v.iter().map(|x| x * x).sum();
    if norm_sq < 1e-24 {
        return false;
    }
    let inv = 1.0 / norm_sq.sqrt();
    for vi in v.iter_mut() {
        *vi *= inv;
    }
    true
}

/// 频谱 → 格点映射
pub(super) fn grid_fill_2d(
    v2: &[f64],
    v3: &[f64],
    board: &Breadboard,
    placeable: &[ComponentId],
    circuit: &Circuit,
    preprocess: &PreprocessResult,
    problem: &AnnealProblem,
) -> Result<(Vec<i32>, Vec<i32>), crate::layout::LayoutError> {
    let n = placeable.len();
    let valid_rows: Vec<i32> = (0..board.rows() as i32)
        .filter(|&r| !board.is_blocked(r as usize))
        .collect();
    if valid_rows.is_empty() {
        return Err(crate::layout::LayoutError::NoLegalInitialPlacement {
            component: placeable[0],
        });
    }
    let n_rows = valid_rows.len().max(1);

    let v2_min = v2.iter().cloned().fold(f64::INFINITY, f64::min);
    let v2_max = v2.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let v2_range = (v2_max - v2_min).max(1e-9);

    let mut order_y: Vec<usize> = (0..n).collect();
    order_y.sort_by(|&a, &b| {
        v3[a]
            .partial_cmp(&v3[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut rank_y = vec![0usize; n];
    for (rank, &idx) in order_y.iter().enumerate() {
        rank_y[idx] = rank;
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        v2[a]
            .partial_cmp(&v2[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let cols = board.cols() as i32;
    let effective_width = (n as i32 * 3).max(2).min(cols - 2);

    let mut target_x = vec![0i32; n];
    for i in 0..n {
        let frac = (v2[i] - v2_min) / v2_range;
        target_x[i] = (frac * effective_width as f64) as i32;
        target_x[i] = target_x[i].clamp(0, cols - 1);
    }

    let mut target_y = vec![0i32; n];
    for i in 0..n {
        target_y[i] = valid_rows[rank_y[i] % n_rows];
    }

    let mut x = vec![0i32; n];
    let mut y = vec![0i32; n];
    let mut occupancy = InitialOccupancy::new(problem);

    for &idx in &order {
        let comp_id = placeable[idx];
        let component = &circuit.components[comp_id.0];
        let r90 = preprocess.r90_only.contains(&comp_id);
        let is_y_locked = preprocess.y_locked.contains_key(&comp_id);
        let rotation = if r90 { Rotation::R90 } else { Rotation::R0 };
        let geometry = InitialGeometry::new(component, circuit, rotation);

        // search
        let mut best: Option<(i32, i32)> = None;
        'search: for dx in 0..=cols {
            for &x_sign in &[1i32, -1i32] {
                if dx == 0 && x_sign == -1 {
                    continue;
                }
                let try_x = target_x[idx] + x_sign * dx;
                if try_x < 0 || try_x >= cols {
                    continue;
                }

                let try_ys: Vec<i32> = if is_y_locked {
                    vec![preprocess.y_locked[&comp_id]]
                } else {
                    let mut cands = Vec::with_capacity(n_rows);
                    for d in 0..n_rows as i32 {
                        for &s in &[0i32, 1i32, -1i32] {
                            if d == 0 && s != 0 {
                                continue;
                            }
                            let yi =
                                (rank_y[idx] as i32 + s * d).rem_euclid(n_rows as i32) as usize;
                            let yv = valid_rows[yi];
                            if !cands.contains(&yv) {
                                cands.push(yv);
                            }
                        }
                    }
                    cands
                };

                for &try_y in &try_ys {
                    if occupancy.try_reserve(board, &geometry, try_x, try_y, is_y_locked) {
                        best = Some((try_x, try_y));
                        break 'search;
                    }
                }
            }
        }

        let (fx, fy) =
            best.ok_or(crate::layout::LayoutError::NoLegalInitialPlacement { component: comp_id })?;
        x[idx] = fx;
        y[idx] = fy;
    }

    Ok((x, y))
}
