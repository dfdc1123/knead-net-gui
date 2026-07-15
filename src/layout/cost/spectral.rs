//! 频谱布局 (Fiedler 向量) 辅助函数。

use fastrand;

use crate::circuit::ComponentId;
use crate::layout::breadboard::Breadboard;
use crate::layout::preprocess::PreprocessResult;

use super::legalize::PlacementHints;

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

/// Convert spectral coordinates into ordering and position preferences for the shared legalizer.
pub(super) fn spectral_hints(
    v2: &[f64],
    v3: &[f64],
    board: &Breadboard,
    placeable: &[ComponentId],
    preprocess: &PreprocessResult,
) -> Result<PlacementHints, crate::layout::LayoutError> {
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

    let v3_min = v3.iter().copied().fold(f64::INFINITY, f64::min);
    let v3_max = v3.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let v3_range = (v3_max - v3_min).max(1e-9);

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

    let row_preferences = placeable
        .iter()
        .enumerate()
        .map(|(idx, component)| {
            if let Some(row) = preprocess.y_locked.get(component) {
                return vec![*row];
            }
            let fraction = (v3[idx] - v3_min) / v3_range;
            let target = (fraction * (n_rows - 1) as f64).round() as usize;
            let mut row_indices: Vec<usize> = (0..n_rows).collect();
            row_indices.sort_by_key(|row| (row.abs_diff(target), *row));
            row_indices.into_iter().map(|row| valid_rows[row]).collect()
        })
        .collect();

    Ok(PlacementHints {
        order,
        target_x,
        row_preferences,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_preferences_preserve_spectral_distance() {
        let placeable = vec![ComponentId(0), ComponentId(1), ComponentId(2)];
        let hints = spectral_hints(
            &[0.0, 0.5, 1.0],
            &[0.0, 0.01, 1.0],
            &Breadboard::new(10, 5),
            &placeable,
            &PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
        )
        .unwrap();

        assert_eq!(hints.row_preferences[0][0], 0);
        assert_eq!(hints.row_preferences[1][0], 0);
        assert_eq!(hints.row_preferences[2][0], 4);
    }
}
