//! 模拟退火布局使用显式 `(x, y, rotation)` 与 bridge pose 状态。
//!
//! move generator 只在当前 pose、y lock 和 bridge catalog 下可应用的 move class
//! 之间重新归一化。OnBoard 可水平/垂直移动和 R0/R180 翻转；Bridged 可沿 catalog
//! 水平移动或更换候选；Explore policy 还可切换 OnBoard/Bridged。`ShiftGroup` 仅在
//! 整组都能严格左移时生成。
//!
//! 每个 attempt 都按 `attempt_index / max_iters` 推进温度，包括没有候选和 hard-invalid
//! 的 attempt。应用后的状态先过 hard legality，再计算 soft cost；reject 使用逐 move
//! backup 完整恢复。调用方只会在最终候选验证通过后事务性写回 `Layout`。
//!
//! OnBoard 姿态限定为 R0/R180；Bridged 候选可由 footprint 的四种旋转生成。
//! seed 可复现契约是同一算法版本、同一输入和同一 seed 得到同一结果。

use std::sync::atomic::{AtomicU64, Ordering};

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;
#[cfg(test)]
use crate::layout::cost::cost;
#[cfg(debug_assertions)]
use crate::layout::cost::state_hard_legal;
use crate::layout::cost::{
    BridgeInitContext, BridgePolicy, CostBuf, SAContext, SAState, Weights, cost_fast,
    cost_fast_if_legal, initialize_bridging, populate_bridgeable_info,
};
use crate::layout::placement::Rotation;
use crate::layout::progress::AnnealMetrics;

// ============================================================
//  Profile helpers (profile_sa cfg 启用, 默认 noop)
//  atomic fetch_add 在 hot path 上有 ~1% 开销; 未启用 profile_sa 时下面
//  的宏都是 noop, 不污染 baseline。
// ============================================================
static PROF_INIT_NS: AtomicU64 = AtomicU64::new(0);
static PROF_GENERATE_NS: AtomicU64 = AtomicU64::new(0);
static PROF_COST_NS: AtomicU64 = AtomicU64::new(0);
static PROF_APPLY_NS: AtomicU64 = AtomicU64::new(0);
static PROF_BEST_NS: AtomicU64 = AtomicU64::new(0);
static PROF_ITERS: AtomicU64 = AtomicU64::new(0);

#[cfg(not(profile_sa))]
macro_rules! prof_iter_inc {
    () => {};
}

#[cfg(profile_sa)]
macro_rules! prof_cost_add {
    ($n:expr) => {
        $crate::layout::sa::PROF_COST_NS.fetch_add($n, Ordering::Relaxed);
    };
}
#[cfg(profile_sa)]
macro_rules! prof_generate_add {
    ($n:expr) => {
        $crate::layout::sa::PROF_GENERATE_NS.fetch_add($n, Ordering::Relaxed);
    };
}
#[cfg(profile_sa)]
macro_rules! prof_apply_add {
    ($n:expr) => {
        $crate::layout::sa::PROF_APPLY_NS.fetch_add($n, Ordering::Relaxed);
    };
}
#[cfg(profile_sa)]
macro_rules! prof_best_add {
    ($n:expr) => {
        $crate::layout::sa::PROF_BEST_NS.fetch_add($n, Ordering::Relaxed);
    };
}
#[cfg(profile_sa)]
macro_rules! prof_iter_inc {
    () => {
        $crate::layout::sa::PROF_ITERS.fetch_add(1, Ordering::Relaxed);
    };
}

pub fn reset_profile() {
    PROF_INIT_NS.store(0, Ordering::Relaxed);
    PROF_GENERATE_NS.store(0, Ordering::Relaxed);
    PROF_COST_NS.store(0, Ordering::Relaxed);
    PROF_APPLY_NS.store(0, Ordering::Relaxed);
    PROF_BEST_NS.store(0, Ordering::Relaxed);
    PROF_ITERS.store(0, Ordering::Relaxed);
}

pub fn dump_profile(prefix: &str) {
    let it = PROF_ITERS.load(Ordering::Relaxed);
    if it == 0 {
        return;
    }
    let init = PROF_INIT_NS.load(Ordering::Relaxed);
    let generate = PROF_GENERATE_NS.load(Ordering::Relaxed);
    let co = PROF_COST_NS.load(Ordering::Relaxed);
    let apply = PROF_APPLY_NS.load(Ordering::Relaxed);
    let b = PROF_BEST_NS.load(Ordering::Relaxed);
    eprintln!(
        "[profile {prefix}] attempts={} init={:?} generate={:?} apply={:?} checked_cost={:?} best_clone={:?}",
        it,
        std::time::Duration::from_nanos(init),
        std::time::Duration::from_nanos(generate),
        std::time::Duration::from_nanos(apply),
        std::time::Duration::from_nanos(co),
        std::time::Duration::from_nanos(b),
    );
}

// 测试用: HashSet 和 rotate 只在下面的 mod tests 里用, 放到 cfg(test) 块里避免非测试
// 构建时的 unused_imports 警告。
#[cfg(test)]
use crate::layout::placement::rotate;
#[cfg(test)]
use std::collections::HashSet;

/// SA 总配置。`Default` 给出 18 元件级别的合理起点。
#[derive(Debug, Clone, Copy)]
pub struct SAConfig {
    /// 退火总迭代数; 后期 SA 接受率接近 0, 跑也是空转。
    pub max_iters: usize,
    /// Attempt 0 的温度。
    pub t_start: f64,
    /// 最后一次 attempt 的温度；schedule 按 `max_iters` 归一化推导。
    pub t_end: f64,
    pub weights: Weights,
    /// 决定随机扰动序列; 改 seed 可重新跑一遍出不同结果。
    pub seed: u64,
    /// 跑多少次取最低 cost 的解。SA 是随机算法, 单次可能卡在 local optimum。
    /// 多 seed 独立跑, 取 cost 最低的。默认 1。
    pub n_seeds: usize,
    /// `true` 用 [`SAState::from_spectral`] 做初排 (频谱嵌入, 无参数, 一步到位);
    /// `false` 用贪心 first-fit [`SAState::from_greedy`].
    pub use_spectral: bool,
    /// Whether Bridged poses are disabled, explored, or mandatory.
    pub bridge_policy: BridgePolicy,
    /// 在当前状态存在可 Toggle 元件时，`Move::ToggleBridging` 的相对权重。
    /// 调高会更频繁探索 Bridged 与 OnBoard 两种 pose。
    /// 只在 [`BridgePolicy::Explore`] 下生效；Disabled / Forced 都不会生成 Toggle。
    /// 0 = 关闭 Explore 的 Toggle 区间；它不再决定是否建立候选或初始姿态。
    pub p_toggle_bridge: f64,
    /// Relative probability mass for changing the active Bridged candidate without
    /// changing pose mode. Only applicable when at least two candidates exist.
    pub p_change_bridge_candidate: f64,
}

impl Default for SAConfig {
    fn default() -> Self {
        Self {
            max_iters: 10000,
            t_start: 10.0,
            t_end: 0.01,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
            n_seeds: 1,
            use_spectral: false,
            bridge_policy: BridgePolicy::default(),
            p_toggle_bridge: 0.15,
            p_change_bridge_candidate: 0.10,
        }
    }
}

// ============================================================
//  扰动
// ============================================================

#[derive(Debug, Clone)]
enum Move {
    /// 翻转单个元件的旋转 (R0 ↔ R180)
    Flip(usize),
    /// 单个元件 x 增 ±N (N 随温度: 高温 1..=3, 低温 1)
    ShiftX(usize, i32),
    /// 单个元件 y 增 ±N (N 随温度: 高温 1..=3, 低温 1)
    ShiftY(usize, i32),
    /// 翻转 `state.bridged[i]` (bridgeable 元件在 OnBoard ↔ Bridged 之间切换)。
    /// 仅当 `state.is_bridgeable[i] = true` 时生成 (见 `random_move`)。
    ToggleBridging(usize),
    /// Bridged 元件切换到另一个预计算候选。
    ChangeBridgeCandidate(usize, usize),
    /// 同 rail 内一组紧邻元件整体左移 1 列, 填桥接留下的空洞。
    /// 组由密度聚类决定 (gap ≤ 2 算同组)。
    ShiftGroup(Vec<usize>),
}

fn random_move(
    state: &SAState,
    rng: &mut fastrand::Rng,
    t: f64,
    t_start: f64,
    config: &SAConfig,
    board: &Breadboard,
) -> Option<Move> {
    let n = state.n();
    if n == 0 {
        return None;
    }
    let start = rng.usize(0..n);

    // 步长随温度变: 高温期 N ∈ {1, 2, 3} 均匀, 中温 {1, 2}, 低温恒为 1。
    // 三个区间按 T0 的 0.5 / 0.2 划分; 越冷越精细, 越热越敢跳。
    let max_n = if t > t_start * 0.5 {
        3
    } else if t > t_start * 0.2 {
        2
    } else {
        1
    };
    let n_amp = 1 + rng.usize(0..max_n) as i32;
    let dx_sign = if rng.f64() < 0.5 { -1 } else { 1 };
    let dy_sign = if rng.f64() < 0.5 { -1 } else { 1 };
    let dx = dx_sign * n_amp;
    let dy = dy_sign * n_amp;

    let p_toggle = if matches!(config.bridge_policy, BridgePolicy::Explore { .. }) {
        config.p_toggle_bridge.clamp(0.0, 0.45)
    } else {
        0.0
    };
    let p_change = config.p_change_bridge_candidate.clamp(0.0, 0.45);
    let standard_scale = 1.0 - p_toggle - p_change;
    let shift_x_weight = standard_scale * 0.37 / 0.85;
    let flip_weight = standard_scale * 0.20 / 0.85;
    let shift_y_weight = standard_scale * 0.20 / 0.85;
    let group_weight = standard_scale * 0.08 / 0.85;

    for offset in 0..n {
        let p = (start + offset) % n;
        let bridged_shift = state.bridged[p]
            .then(|| bridged_shift_target(state, board, p, dx))
            .flatten();
        let can_shift_x = !state.bridged[p] || bridged_shift.is_some();
        let can_flip_or_shift_y = !state.bridged[p] && state.y_locked[p].is_none();
        let can_toggle = p_toggle > 0.0 && state.is_bridgeable[p];
        let can_change = state.bridged[p] && state.bridged_pin_pairs[p].len() > 1;
        let group = find_left_shiftable_group(state, board, p);

        let mut total = 0.0;
        if can_shift_x {
            total += shift_x_weight;
        }
        if can_flip_or_shift_y {
            total += flip_weight + shift_y_weight;
        }
        if can_toggle {
            total += p_toggle;
        }
        if can_change {
            total += p_change;
        }
        if group.is_some() {
            total += group_weight;
        }
        if total <= 0.0 {
            continue;
        }

        let mut choice = rng.f64() * total;
        if can_shift_x {
            if choice < shift_x_weight {
                return Some(Move::ShiftX(p, dx));
            }
            choice -= shift_x_weight;
        }
        if can_flip_or_shift_y {
            if choice < flip_weight {
                return Some(Move::Flip(p));
            }
            choice -= flip_weight;
            if choice < shift_y_weight {
                return Some(Move::ShiftY(p, dy));
            }
            choice -= shift_y_weight;
        }
        if can_toggle {
            if choice < p_toggle {
                return Some(Move::ToggleBridging(p));
            }
            choice -= p_toggle;
        }
        if can_change {
            if choice < p_change {
                let current = state.active_bridge_idx[p];
                let candidate_offset = rng.usize(0..state.bridged_pin_pairs[p].len() - 1);
                let target = if candidate_offset >= current {
                    candidate_offset + 1
                } else {
                    candidate_offset
                };
                return Some(Move::ChangeBridgeCandidate(p, target));
            }
            choice -= p_change;
        }
        if let Some(group) = group {
            debug_assert!(choice < group_weight);
            return Some(Move::ShiftGroup(group));
        }
        debug_assert!(false, "weighted move selection fell through");
    }
    None
}

/// 同一 rail 内密度聚类, 找到 `i` 所在的组。
///
/// 聚类规则: 同 vertical rail 的所有元件 (bridged + OnBoard) 按逻辑 x 排序,
/// 相邻间距 ≤ 2 列为同组。**左移 1 列严格语义**: 任一组成员不能严格左移
/// (bridged 撞 gap 或 OnBoard 贴左壁) → 整个组被拒, 不留半成品。
///
/// 组成员数 + 左边 gap 规则:
/// - 组 ≥2 人总是有效
/// - 单人组仅在左边 gap > 3 列时有效 (落单元件也该被拉近, 填补桥接留下的大空洞)
fn find_left_shiftable_group(state: &SAState, board: &Breadboard, i: usize) -> Option<Vec<usize>> {
    let anchor_rail_top = rail_top(state, board, i)?;

    // 收集同 rail 的所有元件 (bridged 也算, 按 signal pin 的 rail 归组)
    let mut same_rail: Vec<(usize, i32)> = (0..state.n())
        .filter_map(|j| {
            let rj = rail_top(state, board, j)?;
            (rj == anchor_rail_top).then_some((j, logical_x(state, board, j)))
        })
        .collect();

    if same_rail.len() < 2 {
        return None; // 就一个元件, 单个 ShiftX 就好
    }

    same_rail.sort_by_key(|a| a.1);

    // 密度聚类: gap > 2 切分
    let threshold: i32 = 2;
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = vec![same_rail[0].0];
    for w in same_rail.windows(2) {
        let (_prev_j, prev_x) = w[0];
        let (next_j, next_x) = w[1];
        if next_x - prev_x - 1 <= threshold {
            // gap 小: 同组
            cur.push(next_j);
        } else {
            // gap 大: 切开
            groups.push(std::mem::take(&mut cur));
            cur.push(next_j);
        }
    }
    if !cur.is_empty() {
        groups.push(cur);
    }

    // 找到 i 所属的组
    let group_idx = groups.iter().position(|g| g.contains(&i))?;
    let group = &groups[group_idx];

    // 全部成员都能严格左移 1 列才让组成立 (撞 gap 的 bridged 排除 → 整组被拒)
    if !group.iter().all(|&j| can_shift_left_one(state, board, j)) {
        return None;
    }

    // 查 leftmost 的逻辑 x, 算 left gap (用于单 / 多成员判定)
    let leftmost_x = group
        .iter()
        .map(|&j| logical_x(state, board, j))
        .min()
        .unwrap_or(i32::MAX);

    // 查找左边最近的**非组**同 rail 元件 (按逻辑 x)
    let left_neighbor_x = same_rail
        .iter()
        .filter(|(j, _)| !group.contains(j))
        .filter(|(_, x)| *x < leftmost_x)
        .map(|(_, x)| *x)
        .max();

    let gap = match left_neighbor_x {
        Some(lx) => (leftmost_x - lx - 1).max(0),
        None => leftmost_x, // 没左边邻居, gap = 到左壁的距离
    };

    // 组内 ≥2 人, 或 单人但左边空隙 > 3 列 (落单元件也该被拉近)
    if group.len() < 2 && gap <= 3 {
        return None;
    }

    Some(group.clone())
}

/// 应用一个 Move 到状态上, 返回 `true` 表示状态已变更, `false` 表示该 Move
/// 在当前状态下不应落地 (典型的例子: bridged 元素上的 `Flip`/`ShiftY`,
/// bridged `ShiftX` 在 cache 里找不到匹配 pair, `ShiftGroup` 任何一个成员
/// 不可左移)。返回 `false` 时**保证 0 修改**, 调用方应把该候选丢弃。
///
/// 设计要点:
/// - generator 不会为 Bridged 生成 `Flip` / `ShiftY`；直接调用时仍返回 `None`，
///   且不得修改任何字段。
/// - `ShiftX` 在 bridged 上: 在 cache 里找 (power.x+dx, signal.x+dx); 没找到时
///   退一步 (dx + sign(dx)) 跳过电源轨 gap 再试一次, 仍失败再返 `false`。
/// - `ShiftGroup` 见 `apply_group_shift_x`: 两段式验证 / 落写, 任何成员失败
///   则全组放弃, 不留半成品。
/// - backup 保存 move 修改的全部原始字段，用于 reject 时完整恢复。
#[derive(Debug, Clone)]
enum Backup {
    Flip {
        idx: usize,
        old_rot: Rotation,
    },
    ShiftX {
        idx: usize,
        was_bridged: bool,
        old_x: i32,
        old_active_idx: usize,
    },
    ShiftY {
        idx: usize,
        old_y: i32,
    },
    ToggleBridging {
        idx: usize,
        old_bridged: bool,
        old_active_idx: usize,
    },
    ChangeBridgeCandidate {
        idx: usize,
        old_active_idx: usize,
    },
    ShiftGroup {
        // 每成员: (idx, was_bridged, old_x, old_active_idx)
        entries: Vec<(usize, bool, i32, usize)>,
    },
}

impl Backup {
    #[inline]
    fn revert(self, state: &mut SAState) {
        match self {
            Backup::Flip { idx, old_rot } => state.rotation[idx] = old_rot,
            Backup::ShiftX {
                idx,
                was_bridged,
                old_x,
                old_active_idx,
            } => {
                if was_bridged {
                    state.active_bridge_idx[idx] = old_active_idx;
                } else {
                    state.x[idx] = old_x;
                }
            }
            Backup::ShiftY { idx, old_y } => state.y[idx] = old_y,
            Backup::ToggleBridging {
                idx,
                old_bridged,
                old_active_idx,
            } => {
                state.bridged[idx] = old_bridged;
                state.active_bridge_idx[idx] = old_active_idx;
            }
            Backup::ChangeBridgeCandidate {
                idx,
                old_active_idx,
            } => state.active_bridge_idx[idx] = old_active_idx,
            Backup::ShiftGroup { entries } => {
                for (idx, was_bridged, old_x, old_active_idx) in entries {
                    if was_bridged {
                        state.active_bridge_idx[idx] = old_active_idx;
                    } else {
                        state.x[idx] = old_x;
                    }
                }
            }
        }
    }
}

/// 应用 Move, 在成功时返 `Some(backup)` 以便后续 revert。返 `None` 表示该
/// Move 在当前状态下不应落地 (state 未修改, 不需 revert)。
fn apply_move(state: &mut SAState, m: &Move, board: &Breadboard) -> Option<Backup> {
    match m {
        Move::Flip(i) => {
            if state.bridged[*i] || state.y_locked[*i].is_some() {
                return None;
            }
            let old_rot = state.rotation[*i];
            state.rotation[*i] = if state.r90_only[*i] {
                match old_rot {
                    Rotation::R90 => Rotation::R270,
                    Rotation::R270 => Rotation::R90,
                    other => panic!("r90_only 元件不该出现 {:?}", other),
                }
            } else {
                match old_rot {
                    Rotation::R0 => Rotation::R180,
                    Rotation::R180 => Rotation::R0,
                    other => panic!("非 r90_only 元件不该出现 {:?}", other),
                }
            };
            Some(Backup::Flip { idx: *i, old_rot })
        }
        Move::ShiftY(i, dy) => {
            if state.bridged[*i] || state.y_locked[*i].is_some() {
                return None;
            }
            let old_y = state.y[*i];
            state.y[*i] += dy;
            Some(Backup::ShiftY { idx: *i, old_y })
        }
        Move::ShiftX(i, dx) => {
            if state.bridged[*i] {
                let old_active_idx = state.active_bridge_idx[*i];
                if try_bridged_shift_x(state, board, *i, *dx) {
                    Some(Backup::ShiftX {
                        idx: *i,
                        was_bridged: true,
                        old_x: 0,
                        old_active_idx,
                    })
                } else {
                    None
                }
            } else {
                let old_x = state.x[*i];
                state.x[*i] += dx;
                Some(Backup::ShiftX {
                    idx: *i,
                    was_bridged: false,
                    old_x,
                    old_active_idx: 0,
                })
            }
        }
        Move::ShiftGroup(indices) => {
            // 两段式: 先备份, 再 apply; 任一成员失败则还原全部
            let mut entries: Vec<(usize, bool, i32, usize)> = Vec::with_capacity(indices.len());
            let mut x_updates: Vec<(usize, i32)> = Vec::with_capacity(indices.len());
            let mut bridge_updates: Vec<(usize, usize)> = Vec::new();

            for &i in indices {
                if state.bridged[i] {
                    let cur = match state.active_bridge_pair(i) {
                        Some(p) => p,
                        None => {
                            // 失败: 还原已备份的项, 返 None
                            for &(idx, was_bridged, old_x, old_active_idx) in &entries {
                                if was_bridged {
                                    state.active_bridge_idx[idx] = old_active_idx;
                                } else {
                                    state.x[idx] = old_x;
                                }
                            }
                            return None;
                        }
                    };
                    let p = board.hole(cur[0].0).position;
                    let s = board.hole(cur[1].0).position;
                    let tgt_p = crate::circuit::Position { x: p.x - 1, y: p.y };
                    let tgt_s = crate::circuit::Position { x: s.x - 1, y: s.y };
                    let Some(j) = state.bridged_pin_pairs[i].iter().position(|pair| {
                        board.hole(pair[0].0).position == tgt_p
                            && board.hole(pair[1].0).position == tgt_s
                    }) else {
                        // 失败: 还原
                        for &(idx, was_bridged, old_x, old_active_idx) in &entries {
                            if was_bridged {
                                state.active_bridge_idx[idx] = old_active_idx;
                            } else {
                                state.x[idx] = old_x;
                            }
                        }
                        return None;
                    };
                    entries.push((i, true, 0, state.active_bridge_idx[i]));
                    bridge_updates.push((i, j));
                } else {
                    let new_x = state.x[i] - 1;
                    if new_x < 0 {
                        // 失败: 还原
                        for &(idx, was_bridged, old_x, old_active_idx) in &entries {
                            if was_bridged {
                                state.active_bridge_idx[idx] = old_active_idx;
                            } else {
                                state.x[idx] = old_x;
                            }
                        }
                        return None;
                    }
                    entries.push((i, false, state.x[i], 0));
                    x_updates.push((i, new_x));
                }
            }

            for (i, new_x) in x_updates {
                state.x[i] = new_x;
            }
            for (i, j) in bridge_updates {
                state.active_bridge_idx[i] = j;
            }
            Some(Backup::ShiftGroup { entries })
        }
        Move::ToggleBridging(i) => {
            let old_bridged = state.bridged[*i];
            let old_active_idx = state.active_bridge_idx[*i];
            state.bridged[*i] = !old_bridged;
            Some(Backup::ToggleBridging {
                idx: *i,
                old_bridged,
                old_active_idx,
            })
        }
        Move::ChangeBridgeCandidate(i, candidate) => {
            if !state.bridged[*i]
                || *candidate >= state.bridged_pin_pairs[*i].len()
                || *candidate == state.active_bridge_idx[*i]
            {
                return None;
            }
            let old_active_idx = state.active_bridge_idx[*i];
            state.active_bridge_idx[*i] = *candidate;
            Some(Backup::ChangeBridgeCandidate {
                idx: *i,
                old_active_idx,
            })
        }
    }
}

/// bridged 元件: 尝试在 cache 里找 (power+dx, signal+dx), 找不到时按
/// `dx + sign(dx)` 跳 gap 再试。两次都失败 → 返 `false` (state 不变)。
///
/// "跳 gap" 只表示 group 之间没有可插线的孔，并非底层导体断开。例如 x=4
/// 右移一步没有孔时，`ShiftX(+1)` 尝试 `+2` 落到下一 group 的 x=6；两处仍属于
/// 同一条天然导通的完整电源轨行。`ShiftGroup` 保持严格左移 1 列，不跳过无孔位置。
fn try_bridged_shift_x(state: &mut SAState, board: &Breadboard, i: usize, dx: i32) -> bool {
    let Some(target) = bridged_shift_target(state, board, i, dx) else {
        return false;
    };
    state.active_bridge_idx[i] = target;
    true
}

fn bridged_shift_target(state: &SAState, board: &Breadboard, i: usize, dx: i32) -> Option<usize> {
    let cur = state.active_bridge_pair(i)?;
    let old_power = board.hole(cur[0].0).position;
    let old_signal = board.hole(cur[1].0).position;

    if let Some(target) = shifted_candidate(state, board, i, old_power, old_signal, dx) {
        return Some(target);
    }
    if dx != 0 {
        let bumped = dx + dx.signum();
        return shifted_candidate(state, board, i, old_power, old_signal, bumped);
    }
    None
}

fn shifted_candidate(
    state: &SAState,
    board: &Breadboard,
    i: usize,
    old_power: crate::circuit::Position,
    old_signal: crate::circuit::Position,
    dx: i32,
) -> Option<usize> {
    let tgt_power = crate::circuit::Position {
        x: old_power.x + dx,
        y: old_power.y,
    };
    let tgt_signal = crate::circuit::Position {
        x: old_signal.x + dx,
        y: old_signal.y,
    };
    state.bridged_pin_pairs[i].iter().position(|pair| {
        board.hole(pair[0].0).position == tgt_power && board.hole(pair[1].0).position == tgt_signal
    })
}

/// ShiftGroup 应用: 两段式, 第一遍验证 + 收集更新, 第二遍落写。保证全原子
/// (任一成员失败 → 0 修改)。`dx` 通常是 `-1` (`ShiftGroup` 永远左移 1 列),
/// 但作为参数暴露便于复用 / 测试。
///
/// bridged 成员采用严格 dx (不跳 gap): 撞 gap 直接让该 group 整体拒掉。
/// "跳 gap" 是 `ShiftX` 单独的语义, 不在此函数扩散。
/// 测试专用: 保持原有的"任意 dx"接口供测试使用; 生产路径用 `apply_move`。
/// `apply_move` 的 ShiftGroup 分支写死了 dx=-1, 这里需要任意 dx 供测试覆盖。
#[cfg(test)]
fn apply_group_shift_x(
    state: &mut SAState,
    board: &Breadboard,
    indices: &[usize],
    dx: i32,
) -> bool {
    let mut x_updates: Vec<(usize, i32)> = Vec::with_capacity(indices.len());
    let mut bridge_updates: Vec<(usize, usize)> = Vec::new();

    for &i in indices {
        if state.bridged[i] {
            let cur = match state.active_bridge_pair(i) {
                Some(p) => p,
                None => return false,
            };
            let p = board.hole(cur[0].0).position;
            let s = board.hole(cur[1].0).position;
            let tgt_p = crate::circuit::Position {
                x: p.x + dx,
                y: p.y,
            };
            let tgt_s = crate::circuit::Position {
                x: s.x + dx,
                y: s.y,
            };
            let Some(j) = state.bridged_pin_pairs[i].iter().position(|pair| {
                board.hole(pair[0].0).position == tgt_p && board.hole(pair[1].0).position == tgt_s
            }) else {
                return false;
            };
            bridge_updates.push((i, j));
        } else {
            let new_x = state.x[i] + dx;
            if new_x < 0 {
                return false;
            }
            x_updates.push((i, new_x));
        }
    }

    for (i, new_x) in x_updates {
        state.x[i] = new_x;
    }
    for (i, j) in bridge_updates {
        state.active_bridge_idx[i] = j;
    }
    true
}

/// 推一个元件的"逻辑 rail_top": bridged 用 signal pin hole 的 y 找所在 rail
/// (反映它实际占用 main board 的哪一行), OnBoard 用 `state.y[i]`。两者一致
/// 的 rail_top 表示它们处在同一 vertical rail。
fn rail_top(state: &SAState, board: &Breadboard, i: usize) -> Option<i32> {
    let y = if let Some(pair) = state.active_bridge_pair(i) {
        board.hole(pair[1].0).position.y
    } else {
        state.y[i]
    };
    board.rail_rows(y).first().copied()
}

/// 推一个元件的"逻辑 x": bridged 用 signal pin hole 的 x, OnBoard 用 `state.x[i]`。
/// 用于按列排序和"该元件占的左位置"。
fn logical_x(state: &SAState, board: &Breadboard, i: usize) -> i32 {
    if let Some(pair) = state.active_bridge_pair(i) {
        board.hole(pair[1].0).position.x
    } else {
        state.x[i]
    }
}

/// 单一成员能否严格左移 1 列 (ShiftGroup 适用):
/// - OnBoard: `state.x >= 1`
/// - Bridged: cache 里有 `(power.x-1, signal.x-1)` 的 pair
///
/// 注意: `ShiftX` 在 `apply_move` 里有"跳 gap"的额外退路, 此函数**不**覆盖
/// 那条路径 — ShiftGroup 永远严格左移 1, 严格筛 gap。
fn can_shift_left_one(state: &SAState, board: &Breadboard, i: usize) -> bool {
    if state.bridged[i] {
        let cur = match state.active_bridge_pair(i) {
            Some(p) => p,
            None => return false,
        };
        let p = board.hole(cur[0].0).position;
        let s = board.hole(cur[1].0).position;
        let tgt_p = crate::circuit::Position { x: p.x - 1, y: p.y };
        let tgt_s = crate::circuit::Position { x: s.x - 1, y: s.y };
        state.bridged_pin_pairs[i].iter().any(|pair| {
            board.hole(pair[0].0).position == tgt_p && board.hole(pair[1].0).position == tgt_s
        })
    } else {
        state.x[i] >= 1
    }
}

// ============================================================
//  SA 主循环
// ============================================================

pub(super) enum SimulationProgress {
    Initial(SAState),
    Annealing {
        iteration: usize,
        current_cost: f64,
        best_cost: f64,
        metrics: AnnealMetrics,
        state: SAState,
    },
}

#[derive(Debug)]
pub(super) struct SimulationOutcome {
    pub state: SAState,
    pub metrics: AnnealMetrics,
}

pub(super) struct SimulationObserver<'a> {
    pub sample_every: usize,
    pub callback: &'a (dyn Fn(SimulationProgress) + Sync),
}

pub(super) struct SimulationControl<'a> {
    pub observer: Option<SimulationObserver<'a>>,
    pub cancellation: Option<&'a std::sync::atomic::AtomicBool>,
}

fn temperature_at_attempt(config: &SAConfig, attempt: usize) -> f64 {
    let start = if config.t_start.is_finite() && config.t_start > 0.0 {
        config.t_start
    } else {
        f64::MIN_POSITIVE
    };
    let end = if config.t_end.is_finite() && config.t_end > 0.0 {
        config.t_end
    } else {
        f64::MIN_POSITIVE
    };
    if config.max_iters <= 1 {
        return start;
    }
    let progress = attempt.min(config.max_iters - 1) as f64 / (config.max_iters - 1) as f64;
    start * (end / start).powf(progress)
}

/// 跑模拟退火, 返回最佳 [`SAState`] 和 attempt 分类计数。
///
/// 初始状态按 [`SAConfig::use_spectral`] 选 [`SAState::from_spectral`] 或
/// [`SAState::from_greedy`]; 两者都已经避免 pin 撞 / bbox 撞 / 列冲突,
/// (后续 `Flip` / `ShiftX` 偶尔会重新引入列短路, 由 cost 罚分优化掉)。
pub(super) fn simulate(
    placeable: Vec<ComponentId>,
    circuit: &Circuit,
    board: &Breadboard,
    config: &SAConfig,
    problem: &crate::layout::problem::AnnealProblem,
    preprocess: &crate::layout::preprocess::PreprocessResult,
    control: Option<SimulationControl<'_>>,
) -> Result<SimulationOutcome, crate::layout::LayoutError> {
    #[cfg(profile_sa)]
    let profile_init_started = std::time::Instant::now();
    let mut rng = fastrand::Rng::with_seed(config.seed);
    let mut state = if config.use_spectral {
        SAState::from_spectral(placeable, circuit, board, config.seed, preprocess, problem)
    } else {
        SAState::from_greedy(placeable, circuit, board, preprocess, problem)
    }?;
    let observer = control
        .as_ref()
        .and_then(|control| control.observer.as_ref());
    if config.use_spectral
        && let Some(observer) = observer
    {
        (observer.callback)(SimulationProgress::Initial(state.clone()));
    }
    // 从 board 抽 power net ids (绑定的正 / 负极), 然后填桥接字段。
    // 无绑定时 power_net_ids 为空, `populate_bridgeable_info` 内调用的
    // `propose_bridged_pairs` 返空 Vec, 没人会被标 bridgeable, Toggle 不会触发。
    let power_net_ids: Vec<NetId> = board
        .power_rail_bindings()
        .map(|bindings| bindings.iter().map(|(_, _, net)| net).collect())
        .unwrap_or_default();
    if config.bridge_policy != BridgePolicy::Disabled {
        populate_bridgeable_info(&mut state, circuit, board, &power_net_ids);
    }
    // 预计算 context (footprint pin offset, bbox) 和 reusable buffers
    let mut ctx = SAContext::new(circuit, &state.placeable);
    ctx.fill_bridged_bboxes(&state, circuit, board, &[]);
    ctx.fill_problem(problem);
    let mut buf = CostBuf::new(circuit.nets().len(), board.num_rails(), board.main_rows());
    initialize_bridging(
        &mut state,
        BridgeInitContext {
            circuit,
            board,
            bridged_pins: &[],
            weights: &config.weights,
            cost_context: &ctx,
            problem,
        },
        &mut buf,
        config.bridge_policy,
    )?;
    let mut current_cost = cost_fast(&state, circuit, board, &[], &config.weights, &ctx, &mut buf);
    #[cfg(profile_sa)]
    {
        PROF_INIT_NS.fetch_add(
            profile_init_started.elapsed().as_nanos() as u64,
            Ordering::Relaxed,
        );
    }
    let mut best_state = state.clone();
    let mut best_cost = current_cost;
    let mut metrics = AnnealMetrics::default();

    for iteration in 0..config.max_iters {
        if control.as_ref().is_some_and(|control| {
            control
                .cancellation
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        }) {
            break;
        }
        if let Some(observer) = observer
            && iteration % observer.sample_every.max(1) == 0
        {
            (observer.callback)(SimulationProgress::Annealing {
                iteration,
                current_cost,
                best_cost,
                metrics,
                state: state.clone(),
            });
        }
        metrics.attempted += 1;
        let temperature = temperature_at_attempt(config, iteration);
        prof_iter_inc!();
        #[cfg(profile_sa)]
        let t_generate_start = std::time::Instant::now();
        let generated = random_move(&state, &mut rng, temperature, config.t_start, config, board);
        #[cfg(profile_sa)]
        {
            prof_generate_add!(t_generate_start.elapsed().as_nanos() as u64);
        }
        let Some(m) = generated else {
            metrics.no_candidate += 1;
            continue;
        };
        // in-place apply: 返 Some(backup) 表成功, None 表该 move 在当前状态下不应落地
        // (state 未变, 不需 revert)。
        #[cfg(profile_sa)]
        let t_apply_start = std::time::Instant::now();
        let Some(backup) = apply_move(&mut state, &m, board) else {
            #[cfg(profile_sa)]
            {
                prof_apply_add!(t_apply_start.elapsed().as_nanos() as u64);
            }
            metrics.no_candidate += 1;
            debug_assert!(
                false,
                "state-aware generator returned {m:?}, but apply rejected it"
            );
            continue;
        };
        #[cfg(profile_sa)]
        {
            prof_apply_add!(t_apply_start.elapsed().as_nanos() as u64);
        }
        #[cfg(profile_sa)]
        let t_cost_start = std::time::Instant::now();
        let candidate_cost =
            cost_fast_if_legal(&state, circuit, board, &[], &config.weights, &ctx, &mut buf);
        #[cfg(profile_sa)]
        {
            prof_cost_add!(t_cost_start.elapsed().as_nanos() as u64);
        }
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            candidate_cost.is_some(),
            state_hard_legal(&state, circuit, board, problem),
            "cost-derived hard legality diverged after {m:?}: x={:?} y={:?} rotation={:?} bridged={:?}",
            state.x,
            state.y,
            state.rotation,
            state.bridged,
        );
        let Some(new_cost) = candidate_cost else {
            metrics.invalid += 1;
            backup.revert(&mut state);
            continue;
        };
        metrics.evaluated += 1;
        let delta = new_cost - current_cost;

        let accept = delta <= 0.0 || rng.f64() < (-delta / temperature).exp();
        if accept {
            metrics.accepted += 1;
            current_cost = new_cost;
            if current_cost < best_cost {
                best_cost = current_cost;
                // 只有"新最佳"才 clone — 罕见 (~100/seed), 不占用热路径
                #[cfg(profile_sa)]
                let tb = std::time::Instant::now();
                best_state = state.clone();
                #[cfg(profile_sa)]
                {
                    prof_best_add!(tb.elapsed().as_nanos() as u64);
                }
            }
            // accept: 不需要 revert, backup 直接 drop (小内存)
            drop(backup);
        } else {
            backup.revert(&mut state);
        }
    }

    debug_assert_eq!(
        metrics.attempted,
        metrics.no_candidate + metrics.invalid + metrics.evaluated
    );
    debug_assert!(metrics.accepted <= metrics.evaluated);
    Ok(SimulationOutcome {
        state: best_state,
        metrics,
    })
}

// ============================================================
//  测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::Breadboard;

    /// 构造一个最简电路: 2 个 2-pin 元件, pin1=net0, pin2=net0 (都连一起)
    fn simple_circuit() -> Circuit {
        let footprint = Footprint {
            id: FootprintId(0),
            name: "two".into(),
            pins: (1..=2)
                .map(|n| PhysicalPin {
                    name: n.to_string(),
                    offset: Position { x: n - 1, y: 0 },
                })
                .collect(),
        };
        let components = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..4)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(0)),
            })
            .collect();
        let nets = vec![Net {
            id: NetId(0),
            name: "shared".into(),
            pins: (0..4).map(PinId).collect(),
        }];
        Circuit {
            components,
            pins,
            nets,
            footprints: vec![footprint],
        }
    }

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn rng_deterministic() {
        let mut a = fastrand::Rng::with_seed(42);
        let mut b = fastrand::Rng::with_seed(42);
        for _ in 0..100 {
            assert_eq!(a.u64(..), b.u64(..));
        }
    }

    #[test]
    fn rng_different_seeds_differ() {
        let mut a = fastrand::Rng::with_seed(1);
        let mut b = fastrand::Rng::with_seed(2);
        let mut same = 0;
        for _ in 0..100 {
            if a.u64(..) == b.u64(..) {
                same += 1;
            }
        }
        assert!(same < 5, "两个不同 seed 不该几乎一样: same={same}");
    }

    #[test]
    fn random_move_returns_valid_index() {
        let state = SAState::from_greedy(
            vec![ComponentId(0), ComponentId(1)],
            &simple_circuit(),
            &board(),
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            &crate::layout::problem::AnnealProblem::default(),
        )
        .unwrap();
        let cfg = SAConfig::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let b = board();
        // T0=30, T 在 [0.3, 30] 区间走过, 涵盖 max_n=3/2/1 三档。
        for k in 0..200 {
            let t = 30.0_f64 * (1.0 - k as f64 / 200.0) + 0.3;
            let m = random_move(&state, &mut rng, t, 30.0, &cfg, &b).unwrap();
            match m {
                Move::Flip(i)
                | Move::ShiftX(i, _)
                | Move::ShiftY(i, _)
                | Move::ToggleBridging(i)
                | Move::ChangeBridgeCandidate(i, _) => {
                    assert!(i < state.n(), "index {i} out of range {}", state.n());
                }
                Move::ShiftGroup(indices) => {
                    for &i in &indices {
                        assert!(i < state.n(), "index {i} out of range {}", state.n());
                    }
                }
            }
        }
    }

    #[test]
    fn random_move_high_t_uses_larger_amplitude() {
        // 同一个 state, 同一个 seed, 在不同 t 下生成的 ShiftX 步长分布不同
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        let cfg = SAConfig::default();
        let mut rng_hi = fastrand::Rng::with_seed(0);
        let mut rng_lo = fastrand::Rng::with_seed(0);
        let mut hi_max = 0i32;
        let mut lo_max = 0i32;
        let b = board();
        for _ in 0..2000 {
            if let Some(Move::ShiftX(_, dx)) =
                random_move(&state, &mut rng_hi, 30.0, 30.0, &cfg, &b)
            {
                hi_max = hi_max.max(dx.abs());
            }
            if let Some(Move::ShiftX(_, dx)) = random_move(&state, &mut rng_lo, 0.5, 30.0, &cfg, &b)
            {
                lo_max = lo_max.max(dx.abs());
            }
        }
        assert!(hi_max >= 2, "T=30 应该出现 N=2 或 N=3, 最大观测 = {hi_max}");
        assert_eq!(lo_max, 1, "T=0.5 应该恒为 N=1, 最大观测 = {lo_max}");
    }

    #[test]
    fn apply_flip_toggles_rotation() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, &Move::Flip(0), &board());
        assert_eq!(state.rotation[0], Rotation::R180);
        apply_move(&mut state, &Move::Flip(0), &board());
        assert_eq!(state.rotation[0], Rotation::R0);
    }

    #[test]
    fn apply_shift_x_increments_x() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, &Move::ShiftX(0, 1), &board());
        assert_eq!(state.x[0], 1);
        apply_move(&mut state, &Move::ShiftX(0, -2), &board());
        assert_eq!(state.x[0], -1);
    }

    #[test]
    fn apply_shift_y_increments_y() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, &Move::ShiftY(0, 1), &board());
        assert_eq!(state.y[0], 3);
        apply_move(&mut state, &Move::ShiftY(0, -2), &board());
        assert_eq!(state.y[0], 1);
    }

    /// ToggleBridging 翻转 `state.bridged[i]`, 偶数次回到原值。
    #[test]
    fn apply_toggle_bridging_flips_bridged() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        assert!(!state.bridged[0], "初始 bridged 必为 false");
        apply_move(&mut state, &Move::ToggleBridging(0), &board());
        assert!(state.bridged[0], "Toggle 一次后应 = true");
        apply_move(&mut state, &Move::ToggleBridging(0), &board());
        assert!(!state.bridged[0], "Toggle 两次后应 = false");
    }

    // ============================================================
    //  bridged 模式下的扰动行为测试
    //  (Flip / ShiftY 应当静默丢弃; ShiftX 应在 cache 里查找 + 跳 gap;
    //   ShiftGroup 含 bridged 成员时可用 strict -1 语义)
    // ============================================================

    use crate::layout::breadboard::PowerRailBinding;

    /// 1 个 2-pin bridgeable 元件 + power rail binding + 启发式。
    /// 供下面填 cache / 验 shifted pair 行为。
    fn bridgable_fixture() -> (Circuit, Breadboard) {
        // 跨越 3 cols 的水平 footprint (Δ = (3, 0))。
        // R90 后 Δ = (0, 3) — signal pin y 上移 3, body 竖直走向
        // (x 不变、y 变)。这正好是 bridgeable 电阻 跨 power rail → main rail 的典型姿态。
        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 3, y: 0 },
                },
            ],
        };
        // 1 个 component: pin1 = net "+12V", pin2 = net "SIG"
        let comp = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: true,
        };
        let pins = vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: Some(NetId(0)),
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: Some(NetId(1)),
            },
        ];
        let nets = vec![
            Net {
                id: NetId(0),
                name: "+12V".into(),
                pins: vec![PinId(0)],
            },
            Net {
                id: NetId(1),
                name: "SIG".into(),
                pins: vec![PinId(1)],
            },
        ];
        let circuit = Circuit {
            components: vec![comp],
            pins,
            nets,
            footprints: vec![fp],
        };
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: Some(NetId(0)),
            negative: Some(NetId(1)),
        });
        (circuit, board)
    }

    /// 跑 populate_bridgeable_info 并启用 bridged 模式, 返回手动设置好的 state。
    fn bridgable_state_in_bridged(placeable: Vec<ComponentId>) -> SAState {
        let (circuit, board) = bridgable_fixture();
        let mut state = SAState::from_greedy(
            placeable,
            &circuit,
            &board,
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            &crate::layout::problem::AnnealProblem::default(),
        )
        .unwrap();
        populate_bridgeable_info(&mut state, &circuit, &board, &[NetId(0), NetId(1)]);
        assert!(state.is_bridgeable[0], "fixture 应能提供 bridged candidate");
        assert!(!state.bridged_pin_pairs[0].is_empty(), "cache 不该为空");
        state.bridged[0] = true;
        state
    }

    fn assert_state_fields_equal(actual: &SAState, expected: &SAState, context: &str) {
        assert_eq!(actual.placeable, expected.placeable, "{context}: placeable");
        assert_eq!(
            actual.is_bridgeable, expected.is_bridgeable,
            "{context}: is_bridgeable"
        );
        assert_eq!(actual.bridged, expected.bridged, "{context}: bridged");
        assert_eq!(
            actual.bridged_pin_pairs, expected.bridged_pin_pairs,
            "{context}: bridged_pin_pairs"
        );
        assert_eq!(
            actual.active_bridge_idx, expected.active_bridge_idx,
            "{context}: active_bridge_idx"
        );
        assert_eq!(actual.x, expected.x, "{context}: x");
        assert_eq!(actual.y, expected.y, "{context}: y");
        assert_eq!(actual.rotation, expected.rotation, "{context}: rotation");
        assert_eq!(actual.r90_only, expected.r90_only, "{context}: r90_only");
        assert_eq!(actual.y_locked, expected.y_locked, "{context}: y_locked");
    }

    #[test]
    fn every_rejected_move_restores_the_complete_state() {
        let plain_board = board();
        let mut plain = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[1, 1]);
        plain.x = vec![2, 4];
        for (name, movement) in [
            ("Flip", Move::Flip(0)),
            ("ShiftX", Move::ShiftX(0, 1)),
            ("ShiftY", Move::ShiftY(0, 1)),
            ("ShiftGroup", Move::ShiftGroup(vec![0, 1])),
        ] {
            let mut candidate = plain.clone();
            let before = candidate.clone();
            let backup = apply_move(&mut candidate, &movement, &plain_board)
                .unwrap_or_else(|| panic!("{name} fixture 应能应用"));
            backup.revert(&mut candidate);
            assert_state_fields_equal(&candidate, &before, name);
        }

        let (_circuit, bridge_board) = bridgable_fixture();
        let mut toggled = bridgable_state_in_bridged(vec![ComponentId(0)]);
        toggled.bridged[0] = false;
        toggled.active_bridge_idx[0] = 0;
        assert!(toggled.bridged_pin_pairs[0].len() > 1);
        let before = toggled.clone();
        let backup = apply_move(&mut toggled, &Move::ToggleBridging(0), &bridge_board).unwrap();
        backup.revert(&mut toggled);
        assert_state_fields_equal(&toggled, &before, "ToggleBridging");

        let mut changed = bridgable_state_in_bridged(vec![ComponentId(0)]);
        changed.active_bridge_idx[0] = 0;
        let target = changed.bridged_pin_pairs[0].len() - 1;
        let before = changed.clone();
        let backup = apply_move(
            &mut changed,
            &Move::ChangeBridgeCandidate(0, target),
            &bridge_board,
        )
        .unwrap();
        backup.revert(&mut changed);
        assert_state_fields_equal(&changed, &before, "ChangeBridgeCandidate");
    }

    #[test]
    fn every_none_move_leaves_the_complete_state_unchanged() {
        let (_circuit, bridge_board) = bridgable_fixture();
        let bridged = bridgable_state_in_bridged(vec![ComponentId(0)]);
        for (name, movement) in [("Flip", Move::Flip(0)), ("ShiftY", Move::ShiftY(0, 1))] {
            let mut candidate = bridged.clone();
            let before = candidate.clone();
            assert!(apply_move(&mut candidate, &movement, &bridge_board).is_none());
            assert_state_fields_equal(&candidate, &before, name);
        }

        let mut shifted = bridged.clone();
        let leftmost = shifted.bridged_pin_pairs[0]
            .iter()
            .position(|pair| bridge_board.hole(pair[0].0).position.x == 1)
            .expect("应能找到最左可用 power x=1 的候选");
        shifted.active_bridge_idx[0] = leftmost;
        let before = shifted.clone();
        assert!(apply_move(&mut shifted, &Move::ShiftX(0, -2), &bridge_board).is_none());
        assert_state_fields_equal(&shifted, &before, "ShiftX None");

        let mut onboard_change = bridged.clone();
        onboard_change.bridged[0] = false;
        let before = onboard_change.clone();
        assert!(
            apply_move(
                &mut onboard_change,
                &Move::ChangeBridgeCandidate(0, 1),
                &bridge_board,
            )
            .is_none()
        );
        assert_state_fields_equal(&onboard_change, &before, "ChangeCandidate None");

        let mut group = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[1, 1]);
        group.x = vec![2, 0];
        let before = group.clone();
        assert!(apply_move(&mut group, &Move::ShiftGroup(vec![0, 1]), &board()).is_none());
        assert_state_fields_equal(&group, &before, "ShiftGroup None");
    }

    /// Flip 命中 bridged 时 静默丢弃, state 完全不变。
    #[test]
    fn apply_flip_on_bridged_no_state_change() {
        let b = {
            let (_circuit, board) = bridgable_fixture();
            board
        };
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);
        let rotation_before = state.rotation[0];
        let bridged_before = state.bridged[0];
        let active_before = state.active_bridge_idx[0];
        let result = apply_move(&mut state, &Move::Flip(0), &b);
        assert!(result.is_none(), "Flip on bridged 必须返 None (不同意改)");
        assert_eq!(state.rotation[0], rotation_before, "rotation 未动");
        assert_eq!(state.bridged[0], bridged_before, "bridged 未动");
        assert_eq!(
            state.active_bridge_idx[0], active_before,
            "active_bridge_idx 未动"
        );
    }

    /// ShiftY 命中 bridged 时 静默丢弃。
    #[test]
    fn apply_shift_y_on_bridged_no_state_change() {
        let b = {
            let (_circuit, board) = bridgable_fixture();
            board
        };
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);
        let y_before = state.y[0];
        let result = apply_move(&mut state, &Move::ShiftY(0, 1), &b);
        assert!(result.is_none(), "ShiftY on bridged 必须返 None");
        assert_eq!(state.y[0], y_before, "y 未动");
    }

    /// ShiftX 命中 bridged 时在 cache 里查 pair; 找到则落地。
    #[test]
    fn apply_shift_x_on_bridged_finds_cache_match() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);
        let (cur_p_h, _) = state.active_bridge_pair(0).unwrap()[0];
        let p_pos = board.hole(cur_p_h).position;
        // 选一个不撞 gap 的 dx: power x = 0 (group 0-4 起点) → dx=-1 越界, dx=+1 落到 1 (同 group)。选 dx=+1。
        assert!(
            try_bridged_shift_x(&mut state, &board, 0, 1),
            "dx=+1 应在 cache 里找到"
        );
        let new_p_h = state.active_bridge_pair(0).unwrap()[0].0;
        assert_eq!(
            board.hole(new_p_h).position.x,
            p_pos.x + 1,
            "power pin 应右移 1"
        );
    }

    /// ShiftX 命中 bridged 时, dx 落到 gap → 自动跳到 dx+sign(dx) 逃逸。
    /// 标准板 power groups 是 0-4, 6-10, 12-16, ...
    ///     gap 个个在 5, 11, 17, ...
    ///     power pin 在 x=4 (group 末尾) dx=+1 → x=5 是 gap, 应自动跳到 x=6。
    #[test]
    fn apply_shift_x_on_bridged_skips_power_rail_gap() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);

        // 找一个 power pin 位于 x=4 的候选 (group 0-4 末), 设它为 active。
        let target_idx = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 4)
            .expect("应能找到 power x=4 的候选");
        state.active_bridge_idx[0] = target_idx;

        // dx = +1 — 扔到 gap x=5 会 miss。跳 gap 后落到 x=6 应命中。
        let ok = try_bridged_shift_x(&mut state, &board, 0, 1);
        assert!(ok, "跳 gap 后应能在 cache 找到 pair");
        let new_p_h = state.active_bridge_pair(0).unwrap()[0].0;
        assert_eq!(
            board.hole(new_p_h).position.x,
            6,
            "power pin 应跨 gap 落到 x=6"
        );

        // 逆向: power 在 x=6 (group 6-10 起始) dx=-1 → 跳到 x=4。
        // 先重定原位到 x=6:
        let target6 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 6)
            .expect("应能找到 power x=6 的候选");
        state.active_bridge_idx[0] = target6;

        let ok2 = try_bridged_shift_x(&mut state, &board, 0, -1);
        assert!(ok2, "跳 gap 逆向也应能成功");
        let new_p_h = state.active_bridge_pair(0).unwrap()[0].0;
        assert_eq!(
            board.hole(new_p_h).position.x,
            4,
            "power pin 应跨 gap 落到 x=4"
        );
    }

    /// ShiftX 双跳都不命中 (e.g. 起点 + dx 都越界) → 返 false, state 不变。
    #[test]
    fn apply_shift_x_on_bridged_rejects_when_no_target_anywhere() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);

        // x=0 被 preset RailTie 占用，不在 catalog。找到最左 x=1 候选，
        // dx=-2 及 gap fallback -3 都越界，必须拒绝。
        let target1 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 1)
            .expect("应能找到最左可用 power x=1 的候选");
        state.active_bridge_idx[0] = target1;
        let active_before = state.active_bridge_idx[0];

        let ok = try_bridged_shift_x(&mut state, &board, 0, -2);
        assert!(!ok, "dx = -2 越界 + fallback 依然越界, 必须返 false");
        assert_eq!(
            state.active_bridge_idx[0], active_before,
            "active_bridge_idx 不应被改"
        );
    }

    /// `logical_x` / `rail_top` 对 bridged 走 signal pin, OnBoard 走 state.x/y。
    #[test]
    fn logical_helpers_use_signal_pin_for_bridged() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);
        // 当前选了一个启发式 pair; 检查 logical_x 等于 signal pin hole 的 x
        let pair = state.active_bridge_pair(0).unwrap();
        let signal_pos = board.hole(pair[1].0).position;
        assert_eq!(
            logical_x(&state, &board, 0),
            signal_pos.x,
            "logical_x 应读 signal pin"
        );
        // rail_top 应为 signal_pos.y 所在 rail_top
        let expected_top = board.rail_rows(signal_pos.y).first().copied().unwrap();
        assert_eq!(
            rail_top(&state, &board, 0).unwrap(),
            expected_top,
            "rail_top 应读 signal pin y 所在 rail"
        );

        // 关 bridged 后, OnBoard 路径: state.x / state.y
        state.bridged[0] = false;
        assert_eq!(
            logical_x(&state, &board, 0),
            state.x[0],
            "OnBoard logical_x == state.x"
        );
        assert_eq!(
            rail_top(&state, &board, 0).unwrap(),
            board.rail_rows(state.y[0]).first().copied().unwrap(),
            "OnBoard rail_top == state.y 所在 rail_top"
        );
    }

    /// `can_shift_left_one`: OnBoard 看 state.x; bridged 看 cache。
    #[test]
    fn can_shift_left_one_handles_both_modes() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);
        // OnBoard 模式 (bridged=false): state.x=0 应不可左移, 1 可
        state.bridged[0] = false;
        state.x[0] = 0;
        assert!(!can_shift_left_one(&state, &board, 0));
        state.x[0] = 1;
        assert!(can_shift_left_one(&state, &board, 0));
        state.x[0] = -1; // 越界, 也不可左移
        assert!(!can_shift_left_one(&state, &board, 0));

        // Bridged 模式: 默认 RailTie 已移到最右，x=0 现在是最左合法候选。
        state.bridged[0] = true;
        let target0 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 0)
            .expect("power x=0 candidate");
        state.active_bridge_idx[0] = target0;
        assert!(
            !can_shift_left_one(&state, &board, 0),
            "bridged 起点 x=0 已在物理边界，不能继续左移"
        );

        let target1 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 1)
            .expect("power x=1 candidate");
        state.active_bridge_idx[0] = target1;
        assert!(
            can_shift_left_one(&state, &board, 0),
            "bridged 起点 x=1 应能命中左侧 x=0 candidate"
        );

        // 切到非 gap 边缘的 bridge (power x=3, 同 group):
        let target3 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 3)
            .expect("power x=3 candidate");
        state.active_bridge_idx[0] = target3;
        assert!(
            can_shift_left_one(&state, &board, 0),
            "bridged power x=3 有 -1 cache 命中"
        );
    }

    /// ShiftGroup 含 bridged 成员: 全部成员都能严格 -1 → 全组落地。
    /// 这里手造一个 OnBoard 邻居 + 同 rail 的 bridged 成员 (signal pin 与
    /// OnBoard 同 rail_top), 验证 group 允许包含两者且 apply 成功。
    #[test]
    fn shift_group_with_bridged_member_lands_all() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);

        // 追加一个手动造的 "OnBoard 邻居" (凑数看 ShiftGroup):
        state.x.push(2);
        state.y.push(0);
        state.rotation.push(Rotation::R0);
        state.is_bridgeable.push(false);
        state.bridged.push(false);
        state.bridged_pin_pairs.push(Vec::new());
        state.active_bridge_idx.push(0);

        // 找一个 power x=3, signal y=0 的 pair (R90 进 main board row 0);
        // power x=3 同 group 0-4 内, dx=-1 撞不到 gap (但会用到 cache 查找)。
        let mut target = None;
        for (i, pair) in state.bridged_pin_pairs[0].iter().enumerate() {
            let p = board.hole(pair[0].0).position;
            let s = board.hole(pair[1].0).position;
            if p.x == 3 && s.y == 0 {
                target = Some(i);
                break;
            }
        }
        let Some(idx) = target else {
            eprintln!("启发式未返 power x=3, signal y=0 pair, skip");
            return;
        };
        state.active_bridge_idx[0] = idx;

        let onboard_x_before = state.x[1];

        let ok = apply_group_shift_x(&mut state, &board, &[1usize, 0usize], -1);
        assert!(ok, "两个成员都应能严格左移 -1");
        assert_eq!(state.x[1], onboard_x_before - 1, "OnBoard -1");
        let pair = state.active_bridge_pair(0).unwrap();
        let p_after = board.hole(pair[0].0).position;
        let s_after = board.hole(pair[1].0).position;
        assert!(p_after.x >= 0 && s_after.x >= 0);
    }

    /// ShiftGroup 含撞 gap 的 bridged → 整组被拒 (per-member can_shift_left_one fail)。
    /// power x=6 (group 6-10 起始), dx=-1 → 落到 x=5 (gap), cache miss, 拒。
    #[test]
    fn shift_group_with_bridged_at_gap_rejects_entire_group() {
        let (_circuit, board) = bridgable_fixture();
        let mut state = bridgable_state_in_bridged(vec![ComponentId(0)]);
        state.x.push(2);
        state.y.push(0);
        state.rotation.push(Rotation::R0);
        state.is_bridgeable.push(false);
        state.bridged.push(false);
        state.bridged_pin_pairs.push(Vec::new());
        state.active_bridge_idx.push(0);

        // 选一个 power x=6 的 bridged pair (group 6-10 起始)。
        // -1 撞 gap → cache 查不到 → group 拒。
        let target6 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 6)
            .expect("power x=6 candidate");
        state.active_bridge_idx[0] = target6;

        // OnBoard x=2 -1 → x=1 (合法), 不应该拖累。
        let onboard_x_before = state.x[1];
        let active_before = state.active_bridge_idx[0];

        let ok = apply_group_shift_x(&mut state, &board, &[1usize, 0usize], -1);
        assert!(!ok, "bridged 撞 gap → group 必须被拒");
        assert_eq!(state.x[1], onboard_x_before, "OnBoard 也未动 (atomic)");
        assert_eq!(
            state.active_bridge_idx[0], active_before,
            "bridged active_bridge_idx 未动 (atomic)"
        );
    }

    #[test]
    fn random_move_toggle_falls_back_when_not_bridgeable() {
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        // is_bridgeable 默认全 false
        assert!(state.is_bridgeable.iter().all(|&b| !b));
        let cfg = SAConfig {
            p_toggle_bridge: 0.5, // 很大, 提高撞 Toggle 区间的概率
            ..SAConfig::default()
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let b = board();
        for _ in 0..2000 {
            let m = random_move(&state, &mut rng, 10.0, 10.0, &cfg, &b);
            assert!(
                !matches!(m, Some(Move::ToggleBridging(_))),
                "is_bridgeable=false 时 random_move 不应返 ToggleBridging"
            );
        }
    }

    /// OnBoard state 只在当前可用的 move classes 之间重新归一化权重。
    #[test]
    fn random_move_toggle_emits_when_bridgeable() {
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        state.is_bridgeable = vec![true, true];
        let cfg = SAConfig {
            p_toggle_bridge: 0.25, // 低于 clamp 0.45, 期望 ~25% Toggle
            ..SAConfig::default()
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let mut toggle_count = 0usize;
        let b = board();
        let samples = 20_000usize;
        for _ in 0..samples {
            if let Some(Move::ToggleBridging(_)) =
                random_move(&state, &mut rng, 10.0, 10.0, &cfg, &b)
            {
                toggle_count += 1;
            }
        }
        let standard_scale = 1.0 - cfg.p_toggle_bridge - cfg.p_change_bridge_candidate;
        let applicable_standard = standard_scale * (0.37 + 0.20 + 0.20) / 0.85;
        let expected =
            samples as f64 * cfg.p_toggle_bridge / (cfg.p_toggle_bridge + applicable_standard);
        assert!(
            (toggle_count as f64 - expected).abs() < 300.0,
            "state-aware normalized Toggle count expected {expected:.1}, got {toggle_count}"
        );
    }

    /// `p_toggle_bridge=0` 时不会生成 Toggle，其他当前可用 move 的权重重新归一化。
    #[test]
    fn random_move_toggle_disabled_when_p_zero() {
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        state.is_bridgeable = vec![true, true];
        let cfg = SAConfig {
            p_toggle_bridge: 0.0,
            ..SAConfig::default()
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let b = board();
        for _ in 0..2000 {
            let m = random_move(&state, &mut rng, 10.0, 10.0, &cfg, &b);
            assert!(
                !matches!(m, Some(Move::ToggleBridging(_))),
                "p_toggle_bridge=0 时不应返 Toggle"
            );
        }
    }

    #[test]
    fn generator_never_returns_a_mode_inapplicable_move() {
        let (_circuit, bridge_board) = bridgable_fixture();
        let bridged = bridgable_state_in_bridged(vec![ComponentId(0)]);
        let config = SAConfig::default();
        let mut rng = fastrand::Rng::with_seed(0x0BAD_5EED);
        for _ in 0..1_000 {
            let movement = random_move(&bridged, &mut rng, 10.0, 10.0, &config, &bridge_board)
                .expect("fixture should have at least one applicable move");
            let mut candidate = bridged.clone();
            assert!(
                apply_move(&mut candidate, &movement, &bridge_board).is_some(),
                "Bridged state generated an inapplicable move: {movement:?}"
            );
        }

        let mut locked = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        locked.y_locked[0] = Some(2);
        let locked_board = board();
        let mut rng = fastrand::Rng::with_seed(0x0010_C0ED);
        for _ in 0..1_000 {
            let movement = random_move(&locked, &mut rng, 10.0, 10.0, &config, &locked_board)
                .expect("fixture should have at least one applicable move");
            let mut candidate = locked.clone();
            assert!(
                apply_move(&mut candidate, &movement, &locked_board).is_some(),
                "y-locked state generated an inapplicable move: {movement:?}"
            );
        }
    }

    #[test]
    fn temperature_schedule_is_normalized_by_attempt_budget() {
        let quick = SAConfig {
            max_iters: 5_000,
            t_start: 40.0,
            t_end: 0.01,
            ..SAConfig::default()
        };
        let full = SAConfig {
            max_iters: 1_000_000,
            ..quick
        };
        assert_eq!(temperature_at_attempt(&quick, 0), 40.0);
        assert!((temperature_at_attempt(&quick, quick.max_iters - 1) - 0.01).abs() < 1e-12);
        assert!((temperature_at_attempt(&full, full.max_iters - 1) - 0.01).abs() < 1e-12);

        let quick_mid = temperature_at_attempt(&quick, (quick.max_iters - 1) / 2);
        let full_mid = temperature_at_attempt(&full, (full.max_iters - 1) / 2);
        assert!((quick_mid - full_mid).abs() < 0.01);
        assert!(temperature_at_attempt(&quick, 1) < temperature_at_attempt(&quick, 0));
    }

    #[test]
    fn every_attempt_is_classified_even_without_a_candidate_or_after_invalidity() {
        let circuit = simple_circuit();
        let preprocess = crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        };
        let config = SAConfig {
            max_iters: 64,
            bridge_policy: BridgePolicy::Disabled,
            ..SAConfig::default()
        };

        let empty = simulate(
            vec![],
            &circuit,
            &board(),
            &config,
            &crate::layout::problem::AnnealProblem::default(),
            &preprocess,
            None,
        )
        .unwrap();
        assert_eq!(empty.metrics.attempted, config.max_iters);
        assert_eq!(empty.metrics.no_candidate, config.max_iters);
        assert_eq!(empty.metrics.invalid, 0);
        assert_eq!(empty.metrics.evaluated, 0);

        // Two-pin footprint exactly fills this 2x1 board. Every generated Shift/Flip is
        // hard-invalid, but all 64 attempts must still advance the normalized schedule.
        let invalid = simulate(
            vec![ComponentId(0)],
            &circuit,
            &Breadboard::new(2, 1),
            &config,
            &crate::layout::problem::AnnealProblem::default(),
            &preprocess,
            None,
        )
        .unwrap();
        assert_eq!(invalid.metrics.attempted, config.max_iters);
        assert_eq!(invalid.metrics.invalid, config.max_iters);
        assert_eq!(invalid.metrics.no_candidate, 0);
        assert_eq!(invalid.metrics.evaluated, 0);

        let evaluated = simulate(
            vec![ComponentId(0)],
            &circuit,
            &board(),
            &config,
            &crate::layout::problem::AnnealProblem::default(),
            &preprocess,
            None,
        )
        .unwrap();
        assert_eq!(evaluated.metrics.attempted, config.max_iters);
        assert_eq!(
            evaluated.metrics.attempted,
            evaluated.metrics.no_candidate
                + evaluated.metrics.invalid
                + evaluated.metrics.evaluated
        );
        assert!(evaluated.metrics.evaluated > 0);
        assert!(evaluated.metrics.accepted <= evaluated.metrics.evaluated);
    }

    /// 验证 SAState::from_greedy 在标准板上不会试着把元件放中间 blocked row。
    #[test]
    fn from_greedy_avoids_blocked_rows() {
        let board = crate::layout::Breadboard::standard();
        let circuit = simple_circuit();
        let state = SAState::from_greedy(
            vec![ComponentId(0), ComponentId(1)],
            &circuit,
            &board,
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            &crate::layout::problem::AnnealProblem::default(),
        )
        .unwrap();
        for &y in &state.y {
            assert!(
                !board.is_blocked(y as usize),
                "from_greedy 把元件放到了 blocked row y={y}"
            );
        }
    }

    #[test]
    fn zero_iterations_do_not_force_bridging_when_onboard_is_equally_good() {
        let (circuit, board) = bridgable_fixture();
        let config = SAConfig {
            max_iters: 0,
            p_toggle_bridge: 0.0,
            bridge_policy: BridgePolicy::Explore {
                initial: crate::layout::cost::BridgeInitial::BestOfBoth,
            },
            weights: Weights {
                mst: 0.0,
                pin_overlap: 0.0,
                b_box_overlap: 0.0,
                column_conflict: 0.0,
                out_of_bounds: 0.0,
                compactness: 0.0,
                rail_crossing: 0.0,
                row_squash: 0.0,
                mst_congestion: 0.0,
            },
            ..SAConfig::default()
        };
        let outcome = simulate(
            vec![ComponentId(0)],
            &circuit,
            &board,
            &config,
            &crate::layout::problem::AnnealProblem::default(),
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            None,
        )
        .unwrap();

        assert_eq!(outcome.state.bridged, vec![false]);
    }

    #[test]
    fn bridge_policy_controls_catalog_initial_mode_and_toggle_availability() {
        use crate::layout::cost::BridgeInitial;

        let (circuit, board) = bridgable_fixture();
        let preprocess = crate::layout::preprocess::PreprocessResult {
            r90_only: std::collections::HashSet::new(),
            y_locked: std::collections::HashMap::new(),
        };
        let problem = crate::layout::problem::AnnealProblem::default();
        let run = |policy| {
            simulate(
                vec![ComponentId(0)],
                &circuit,
                &board,
                &SAConfig {
                    max_iters: 0,
                    bridge_policy: policy,
                    ..SAConfig::default()
                },
                &problem,
                &preprocess,
                None,
            )
            .unwrap()
        };

        let disabled = run(BridgePolicy::Disabled);
        assert_eq!(disabled.state.is_bridgeable, vec![false]);
        assert_eq!(disabled.state.bridged, vec![false]);
        assert!(disabled.state.bridged_pin_pairs[0].is_empty());

        let onboard = run(BridgePolicy::Explore {
            initial: BridgeInitial::OnBoard,
        });
        assert_eq!(onboard.state.is_bridgeable, vec![true]);
        assert_eq!(onboard.state.bridged, vec![false]);
        assert!(!onboard.state.bridged_pin_pairs[0].is_empty());

        let forced = run(BridgePolicy::Forced);
        assert_eq!(forced.state.is_bridgeable, vec![true]);
        assert_eq!(forced.state.bridged, vec![true]);

        let mut rng = fastrand::Rng::with_seed(7);
        let forced_config = SAConfig {
            bridge_policy: BridgePolicy::Forced,
            p_toggle_bridge: 0.45,
            p_change_bridge_candidate: 0.45,
            ..SAConfig::default()
        };
        let mut saw_candidate_change = false;
        for _ in 0..500 {
            let movement = random_move(&forced.state, &mut rng, 10.0, 10.0, &forced_config, &board);
            assert!(
                !matches!(movement, Some(Move::ToggleBridging(_))),
                "Forced policy must not emit a move back to OnBoard"
            );
            saw_candidate_change |= matches!(movement, Some(Move::ChangeBridgeCandidate(_, _)));
        }
        assert!(saw_candidate_change);
    }

    #[test]
    fn forced_bridge_policy_errors_without_a_legal_candidate() {
        let (circuit, _) = bridgable_fixture();
        let board = Breadboard::standard();
        let result = simulate(
            vec![ComponentId(0)],
            &circuit,
            &board,
            &SAConfig {
                max_iters: 0,
                bridge_policy: BridgePolicy::Forced,
                ..SAConfig::default()
            },
            &crate::layout::problem::AnnealProblem::default(),
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            None,
        );

        assert_eq!(
            result.unwrap_err(),
            crate::layout::LayoutError::NoLegalInitialPlacement {
                component: ComponentId(0)
            }
        );
    }

    #[test]
    fn cancellation_after_bridge_initialization_returns_a_complete_state() {
        use crate::layout::cost::BridgeInitial;

        let (circuit, board) = bridgable_fixture();
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let outcome = simulate(
            vec![ComponentId(0)],
            &circuit,
            &board,
            &SAConfig {
                max_iters: 100,
                bridge_policy: BridgePolicy::Explore {
                    initial: BridgeInitial::OnBoard,
                },
                ..SAConfig::default()
            },
            &crate::layout::problem::AnnealProblem::default(),
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            Some(SimulationControl {
                observer: None,
                cancellation: Some(&cancelled),
            }),
        )
        .unwrap();

        assert_eq!(outcome.state.placeable, vec![ComponentId(0)]);
        assert_eq!(outcome.state.is_bridgeable, vec![true]);
        assert_eq!(outcome.state.bridged, vec![false]);
        assert!(!outcome.state.bridged_pin_pairs[0].is_empty());
    }

    #[test]
    fn sa_converges_below_initial() {
        let circuit = simple_circuit();
        // 故意构造差初始: C0 @ (0,0), C1 @ (3,4) — 都在不同 row 远距离
        // 共享 net, MST = 8 (远大于 0, SA 应能压下来)
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        state.x = vec![0, 3];
        state.y = vec![0, 4];
        let initial_cost = cost(&state, &circuit, &board(), &[], &Weights::default());
        let config = SAConfig {
            max_iters: 2000,
            t_start: 5.0,
            t_end: 0.01,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        };
        let best = simulate(
            vec![ComponentId(0), ComponentId(1)],
            &circuit,
            &board(),
            &config,
            &crate::layout::problem::AnnealProblem::default(),
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            None,
        )
        .unwrap()
        .state;
        let best_cost = cost(&best, &circuit, &board(), &[], &Weights::default());
        assert!(
            best_cost <= initial_cost,
            "SA 应该不恶化: init={initial_cost} best={best_cost}"
        );
    }

    #[test]
    fn sa_finds_valid_layout_on_simple_circuit() {
        let circuit = simple_circuit();
        let config = SAConfig {
            max_iters: 1000,
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        };
        let best = simulate(
            vec![ComponentId(0), ComponentId(1)],
            &circuit,
            &board(),
            &config,
            &crate::layout::problem::AnnealProblem::default(),
            &crate::layout::preprocess::PreprocessResult {
                r90_only: std::collections::HashSet::new(),
                y_locked: std::collections::HashMap::new(),
            },
            None,
        )
        .unwrap()
        .state;
        // SA 输出本身就是 final 位置 (compact 删了, cost 里的 compactness 替代)
        let xs = best.x.clone();
        // 检查 pin 不撞
        let mut holes: HashSet<(i32, i32)> = HashSet::new();
        #[allow(clippy::needless_range_loop)]
        for idx in 0..best.n() {
            let comp = &circuit.components[best.placeable[idx].0];
            let footprint = &circuit.footprints[comp.footprint.unwrap().0];
            let rotation = best.rotation[idx];
            for &pin_id in &comp.pins {
                let pin = &circuit.pins[pin_id.0];
                let pp = footprint
                    .pins
                    .iter()
                    .find(|p| p.name() == pin.num())
                    .unwrap();
                let r = rotate(pp.offset, rotation);
                let hole = (xs[idx] + r.x, best.y[idx] + r.y);
                assert!(holes.insert(hole), "pin 撞了: {:?}", hole);
            }
        }
    }
}
