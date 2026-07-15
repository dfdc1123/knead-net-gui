//! 频谱布局 (Fiedler 向量) 辅助函数。

use std::collections::HashSet;

use fastrand;

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;
use crate::layout::preprocess::PreprocessResult;
use crate::layout::problem::AnnealProblem;

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
) -> (Vec<i32>, Vec<i32>) {
    let n = placeable.len();
    let valid_rows: Vec<i32> = (0..board.rows() as i32)
        .filter(|&r| !board.is_blocked(r as usize))
        .collect();
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
        target_x[i] = (1.0 + frac * effective_width as f64) as i32;
        target_x[i] = target_x[i].clamp(0, cols - 1);
    }

    let mut target_y = vec![0i32; n];
    for i in 0..n {
        target_y[i] = valid_rows[rank_y[i] % n_rows];
    }

    let mut x = vec![0i32; n];
    let mut y = vec![0i32; n];
    let mut occupied: HashSet<(i32, i32)> = problem.fixed_geometry.occupied_cells.clone();
    let mut col_owner = problem.fixed_geometry.rail_owners.clone();

    for &idx in &order {
        let comp_id = placeable[idx];
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.expect("placeable 必有 footprint");
        let footprint = &circuit.footprints[fid.0];

        let r90 = preprocess.r90_only.contains(&comp_id);
        let is_y_locked = preprocess.y_locked.contains_key(&comp_id);

        // pin info with rotation
        let pin_info: Vec<(i32, i32, Option<NetId>)> = component
            .pins
            .iter()
            .map(|&pin_id| {
                let pin = &circuit.pins[pin_id.0];
                let physical = footprint.physical_pin_for(pin).expect("footprint 缺 pin");
                let (ox, oy) = if r90 {
                    (-physical.offset.y, physical.offset.x)
                } else {
                    (physical.offset.x, physical.offset.y)
                };
                (ox, oy, pin.net)
            })
            .collect();

        // bbox with rotation
        let (min_x, max_x, min_y, max_y) = footprint.pins().iter().fold(
            (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
            |(lx, rx, ly, ry), p| {
                let (ox, oy) = if r90 {
                    (-p.offset.y, p.offset.x)
                } else {
                    (p.offset.x, p.offset.y)
                };
                (lx.min(ox), rx.max(ox), ly.min(oy), ry.max(oy))
            },
        );
        let bbox_cells: Vec<(i32, i32)> = (min_y..=max_y)
            .flat_map(|yy| (min_x..=max_x).map(move |xx| (xx, yy)))
            .collect();

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
                    // OOB / blocked (skip blocked rows for y_locked)
                    let oob_or_blocked = bbox_cells.iter().any(|&(ox, oy)| {
                        let ax = try_x + ox;
                        let ay = try_y + oy;
                        ax < 0
                            || ax >= cols
                            || ay < 0
                            || ay >= board.rows() as i32
                            || (!is_y_locked && board.is_blocked(ay as usize))
                    });
                    if oob_or_blocked {
                        continue;
                    }

                    // collision
                    let collides = bbox_cells.iter().any(|&(ox, oy)| {
                        let ay = try_y + oy;
                        if is_y_locked && board.is_blocked(ay as usize) {
                            return false;
                        }
                        occupied.contains(&(try_x + ox, ay))
                    });
                    if collides {
                        continue;
                    }

                    // column conflict
                    let col_conflict = pin_info.iter().any(|&(lx, ly, pin_net)| {
                        let abs_x = try_x + lx;
                        let abs_y = try_y + ly;
                        if abs_x < 0
                            || abs_x >= cols
                            || abs_y < 0
                            || abs_y >= board.rows() as i32
                            || board.is_blocked(abs_y as usize)
                        {
                            return true;
                        }
                        let rail_id = board.effective_rail_id_at(abs_x, abs_y);
                        match col_owner.get(&rail_id) {
                            Some(existing) => *existing != pin_net,
                            None => false,
                        }
                    });
                    if col_conflict {
                        continue;
                    }

                    best = Some((try_x, try_y));
                    break 'search;
                }
            }
        }

        let (fx, fy) =
            best.unwrap_or_else(|| panic!("板太小, 装不下元件 {} (spectral grid fill)", comp_id.0));
        x[idx] = fx;
        y[idx] = fy;

        for &(ox, oy) in &bbox_cells {
            let ay = fy + oy;
            if !is_y_locked || !board.is_blocked(ay as usize) {
                occupied.insert((fx + ox, ay));
            }
        }
        for &(lx, ly, pin_net) in &pin_info {
            let abs_x = fx + lx;
            let abs_y = fy + ly;
            if abs_x >= 0
                && abs_x < cols
                && abs_y >= 0
                && abs_y < board.rows() as i32
                && !board.is_blocked(abs_y as usize)
            {
                let rail_id = board.effective_rail_id_at(abs_x, abs_y);
                col_owner.entry(rail_id).or_insert(pin_net);
            }
        }
    }

    (x, y)
}
