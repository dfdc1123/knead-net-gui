//! 频谱布局 (Fiedler 向量) 辅助函数。
//!
//! 私有: 本文件所有函数仅在 cost/ 内部使用, 不 re-export。

use std::collections::{HashMap, HashSet};

use fastrand;

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;

/// 幂迭代求 Fiedler 向量 (拉普拉斯 L 的第二小特征向量)。
///
/// L 的最小特征值为 0, 对应常向量 [1,1,...,1]。
/// 对 M = cI - L 做幂迭代, 投射掉常向量分量, 收敛到 Fiedler。
///
/// **初始向量确定性**: 用 `seed` 初始化的本地 fastrand Rng 生成均勯分布
/// 向量, 投射调正后归一化。`seed` 不同 → 初始向量不同 → 最终 v₂/初排不同;
/// `seed` 相同 → 完全一致的幂迭代手检 (跨进程可复现)。
///
/// **为什么不让 `seed = None` 走上次全局 fastrand**: 进程启动时全局 RNG
/// 种子随机 (`fastrand::seed(...)` 没被调), 同 seed 不同进程得到不同 v₂;
/// 之前以为 "5 wires [10, 13]" 之间的浮动能复现, 其实不能。
pub(super) fn compute_fiedler(l: &[Vec<f64>], n: usize, seed: u64) -> Vec<f64> {
    // c > λ_max(L)。λ_max ≤ 2·max_degree
    let max_deg = (0..n).map(|i| l[i][i]).fold(0.0f64, f64::max);
    let c = if max_deg > 0.0 { 2.0 * max_deg } else { 1.0 };

    // 本地 seed RNG — 跟 SA 主循环的 `with_seed(config.seed)` 一致, 保证
    // 同 seed 跨进程可复现。
    let mut rng = fastrand::Rng::with_seed(seed);
    let mut v: Vec<f64> = (0..n).map(|_| rng.f64() - 0.5).collect();
    project_out_constant(&mut v, n);
    if !normalize_vec(&mut v) {
        // 退化情形 (如 n=1), 幂迭代不能收敛; 给个常向量, 后续 finalize 会处理。
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

/// 幂迭代求第三特征向量 v₃ (正交于常向量和 v₂)。同 `compute_fiedler` 的 seed 语义。
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

/// (cI - L) * v
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

/// 投射掉常向量分量: v ← v - mean(v)·1
pub(super) fn project_out_constant(v: &mut [f64], n: usize) {
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    for vi in v.iter_mut() {
        *vi -= mean;
    }
}

/// 投射掉常向量和 v2 分量
pub(super) fn project_out_two(v: &mut [f64], v2: &[f64], n: usize) {
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    let dot_v2: f64 = v.iter().zip(v2).map(|(a, b)| a * b).sum();
    for (i, vi) in v.iter_mut().enumerate() {
        *vi = *vi - mean - dot_v2 * v2[i];
    }
}

/// 归一化, 返回是否成功 (norm > 0)
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

/// 频谱 → 格点映射: v₂ 值 → x 目标位置 (保聚类), v₃ rank → y 分布,
/// 然后贪心左紧排消碰撞。
///
/// v₂ 相近的元件 (同 net / 强耦合) 自然映射到相近的 x, 不像 rank 均匀分布
/// 那样把 5 个元件也摊满 60 列。`effective_width = max(2, min(n * 3, cols - 2))`
/// 进一步防止过散, 贪心碰撞解决保证无 pin/bbox/列冲突。
pub(super) fn grid_fill_2d(
    v2: &[f64],
    v3: &[f64],
    board: &Breadboard,
    n: usize,
    placeable: &[ComponentId],
    circuit: &Circuit,
) -> (Vec<i32>, Vec<i32>) {
    let valid_rows: Vec<i32> = (0..board.rows() as i32)
        .filter(|&r| !board.is_blocked(r as usize))
        .collect();
    let n_rows = valid_rows.len().max(1);

    // ── v₂ 归一化到 [0, 1] (保留聚类信息) ──
    let v2_min = v2.iter().cloned().fold(f64::INFINITY, f64::min);
    let v2_max = v2.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let v2_range = (v2_max - v2_min).max(1e-9);

    // ── v₃ rank → y ──
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

    // ── v₂ 排序决定从左到右的贪心放置顺序 ──
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        v2[a]
            .partial_cmp(&v2[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let cols = board.cols() as i32;

    // 有效宽度: 每个元件 ~3 列 (自身 + 间距), 上限为板宽
    let effective_width = (n as i32 * 3).max(2).min(cols - 2);

    // 目标 x: 由 v₂ 值决定, 缩放至有效宽度
    let mut target_x = vec![0i32; n];
    for i in 0..n {
        let frac = (v2[i] - v2_min) / v2_range;
        target_x[i] = (1.0 + frac * effective_width as f64) as i32;
        target_x[i] = target_x[i].clamp(0, cols - 1);
    }

    // 目标 y
    let mut target_y = vec![0i32; n];
    for i in 0..n {
        target_y[i] = valid_rows[rank_y[i] % n_rows];
    }

    // ── 贪心碰撞解决: v₂ 顺序, 从目标位置向右扫 (保持聚类顺序) ──
    let mut x = vec![0i32; n];
    let mut y = vec![0i32; n];
    let mut occupied: HashSet<(i32, i32)> = HashSet::new();
    let mut col_owner: HashMap<(i32, i32), Option<NetId>> = HashMap::new();

    for &idx in &order {
        let comp_id = placeable[idx];
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.expect("placeable 必有 footprint");
        let footprint = &circuit.footprints[fid.0];

        // pin 信息：(本地 offset, net) — 用于列冲突检查
        let pin_info: Vec<(i32, i32, Option<NetId>)> = component
            .pins
            .iter()
            .map(|&pin_id| {
                let pin = &circuit.pins[pin_id.0];
                let physical = footprint
                    .pins()
                    .iter()
                    .find(|p| p.name() == pin.num())
                    .expect("footprint 缺 pin");
                (physical.offset.x, physical.offset.y, pin.net)
            })
            .collect();

        // bbox 用 footprint 全部物理 pin 算
        let (min_x, max_x, min_y, max_y) = footprint.pins().iter().fold(
            (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
            |(lx, rx, ly, ry), p| {
                (
                    lx.min(p.offset.x),
                    rx.max(p.offset.x),
                    ly.min(p.offset.y),
                    ry.max(p.offset.y),
                )
            },
        );
        let bbox_cells: Vec<(i32, i32)> = (min_y..=max_y)
            .flat_map(|yy| (min_x..=max_x).map(move |xx| (xx, yy)))
            .collect();

        // 从目标位置出发, 左右交替扩展, 同 row 优先, 再换行
        let mut best: Option<(i32, i32)> = None;
        'search: for dx in 0..=cols {
            for &x_sign in &[1i32, -1i32] {
                if dx == 0 && x_sign == -1 {
                    continue; // 跳过 dx=0 的重复
                }
                let try_x = target_x[idx] + x_sign * dx;
                if try_x < 0 || try_x >= cols {
                    continue;
                }
                // 优先目标行, 然后上下轮替
                for dy in 0..n_rows as i32 {
                    for &dy_sign in &[0i32, 1i32, -1i32] {
                        if dy == 0 && dy_sign != 0 {
                            continue;
                        }
                        let try_y_idx =
                            (rank_y[idx] as i32 + dy_sign * dy).rem_euclid(n_rows as i32) as usize;
                        let try_y = valid_rows[try_y_idx];

                        // OOB / blocked
                        let oob_or_blocked = bbox_cells.iter().any(|&(ox, oy)| {
                            let ax = try_x + ox;
                            let ay = try_y + oy;
                            ax < 0
                                || ax >= cols
                                || ay < 0
                                || ay >= board.rows() as i32
                                || board.is_blocked(ay as usize)
                        });
                        if oob_or_blocked {
                            continue;
                        }
                        // bbox 碰撞
                        let collides = bbox_cells
                            .iter()
                            .any(|&(ox, oy)| occupied.contains(&(try_x + ox, try_y + oy)));
                        if collides {
                            continue;
                        }
                        // 列冲突
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
                            let rail_top = board.rail_rows(abs_y).first().copied().unwrap_or(abs_y);
                            match col_owner.get(&(abs_x, rail_top)) {
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
        }

        let (fx, fy) =
            best.unwrap_or_else(|| panic!("板太小, 装不下元件 {} (spectral grid fill)", comp_id.0));
        x[idx] = fx;
        y[idx] = fy;
        for &(ox, oy) in &bbox_cells {
            occupied.insert((fx + ox, fy + oy));
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
                let rail_top = board.rail_rows(abs_y).first().copied().unwrap_or(abs_y);
                col_owner.entry((abs_x, rail_top)).or_insert(pin_net);
            }
        }
    }

    (x, y)
}
