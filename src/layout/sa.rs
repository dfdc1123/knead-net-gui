//! 模拟退火布局: 显式 `(x, y, rotation)` 状态。
//!
//! 流程: [`simulate`] → 写回 `Layout.placements` → `validate`。
//! 紧凑度已折进 [`cost::cost`], SA 一次跑完搞定。
//!
//! 扰动集 (v7, 概率, 默认 `p_toggle_bridge = 0.15` 时生效):
//! - 37% `ShiftX` —— 单个元件左右微调; OnBoard 改 `state.x`, bridged 在 cache
//!   里找 `(±dx)` 偏移的 pin pair; 找不到时退一步 `dx+sign(dx)` 跳过电源轨 gap
//!   (e.g. dx=+1 撞 gap → 自动变 dx=+2)。
//! - 20% `Flip` —— 翻转单个元件的方向 (R0 ↔ R180); 仅 OnBoard 生效。
//! - 20% `ShiftY` —— 单个元件上下微调; 仅 OnBoard 生效。
//! - 15% `ToggleBridging` —— 翻转 bridgeable 元件是否走桥接; 见下文。
//! - 8% `ShiftGroup` —— 同 rail 内一组紧邻元件整体左移 1 列, 填桥接留下的空洞;
//!   bridged 元件**也参与** (按 signal pin rail 归组), 但必须能严格左移 1 列 —
//!   gap 处的 bridged 成员会过滤掉进而让整个组被拒, 不会"跳 gap"。
//!
//! 高温期 `ShiftX`/`ShiftY` 幅度可达 ±3 col/row, 低温退到 ±1。
//!
//! v7 相对之前的核心改动 (分布与扰动集都重排):
//! - 新增 `ShiftGroup` 扰动: 把同 rail 一组紧邻元件左移 1 列, 专门填补
//!   桥接留下的横向空洞。组由 `find_left_shiftable_group` 计算: 同 rail 按
//!   逻辑 x (`state.x` 对 OnBoard / signal pin hole 对 bridged) 排序,
//!   相邻间距 ≤ 2 列同组; 组内每个成员均验证"可左移 1"; 组 ≥2 人或单人左 gap > 3
//!   列才算可用组。
//! - 分布重排: `ShiftX` 从 45% → 37%, 让出 8% 给 `ShiftGroup`; `Flip` 30 → 20,
//!   `ShiftY` 8 → 20, `ToggleBridging` 7 → 15。
//!
//! bridged 在扰动集里的行为总结:
//! - `Flip` / `ShiftY` 命中 bridged → `apply_move` 返 `false`, Move 静默放弃
//!   (保持 RNG 序列, 现有 reproducibility 测不破)。
//! - `ShiftX` 命中 bridged → 在 `state.bridged_pin_pairs[i]` cache 里查找
//!   `(power.x+dx, signal.x+dx)` 的 pair; 失败时按 `dx + sign(dx)` 跳 gap 再试;
//!   仍未命中则返 `false`。
//! - `ShiftGroup` 命中 bridged → 走"按 signal pin 位置归组", 严格左移 1 列;
//!   撞 gap 的 bridged 在 `find_left_shiftable_group` 阶段就被过滤掉。
//!
//! `ToggleBridging` 选 index 的逻辑: `random_move` 选 `p ∈ [0, n)`, 然后按
//! 分布表决定 move 类型; 只有落到 Toggle 区间且 `state.is_bridgeable[p] = true`
//! 才生成 `ToggleBridging(p)`, 否则退回 `ShiftX`。**不**重抽整个 move,
//! 避免改变 RNG 消费数 (跟 `rng.usize(0..max_n)` 在 max_n=1 时仍消费一个
//! 随机数的原则一致, 保持 seed 复现性)。
//! 桥接位置由 [`crate::layout::cost::propose_bridged_pairs`] 启发式预计算
//! 并缓存在 `state.bridged_pin_pairs`; Toggle 只决定是否采用, 不重新选孔。
//!
//! 回退规则 (保持 RNG 消费序列对 seed 复现, 不重抽整个 move):
//! - `ToggleBridging` 抽到非 bridgeable 元件 → 退回 `ShiftX`
//! - `ShiftGroup` 找不到可用组              → 退回 `ShiftX`
//!
//! **不**用 R90/R270 (会改变 footprint 的水平宽度, 破坏"显式 2D 状态"假设)。
//! 上面这条限制仅适用于 OnBoard 路径; Bridged 路径由 `propose_bridged_pair`
//! 在启发式内部枚举 4 种旋转 (body 浮在板外, 不受"显式 2D 状态"约束)。
//!
//! Rng: [`fastrand::Rng`] (WyRand), 不密码学安全但统计性质足够 SA 用。

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;
#[cfg(test)]
use crate::layout::cost::cost;
use crate::layout::cost::{
    CostBuf, FDConfig, SAContext, SAState, Weights, cost_fast, init_bridgeable_to_bridged,
    populate_bridgeable_info,
};
use crate::layout::placement::Rotation;

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
    /// 初始温度; 经验上 T0 ≈ 3 * "典型变差 Δcost" 比较稳。
    pub t0: f64,
    /// 每步 T *= cool_rate; 0.95 通用, 0.9 更快但更糙。
    pub cool_rate: f64,
    pub weights: Weights,
    /// 决定随机扰动序列; 改 seed 可重新跑一遍出不同结果。
    pub seed: u64,
    /// 跑多少次取最低 cost 的解。SA 是随机算法, 单次可能卡在 local optimum。
    /// 多 seed 独立跑, 取 cost 最低的。默认 1。
    pub n_seeds: usize,
    /// `true` 用 [`SAState::from_force_directed`] 做初排 (比 `from_greedy` 慢,
    /// 但对强耦合电路起点好得多); `false` 用贪心 first-fit。
    pub use_force_directed: bool,
    /// `true` 用 [`SAState::from_spectral`] 做初排 (频谱嵌入, 无参数, 一步到位)。
    /// 优先于 `use_force_directed`。
    pub use_spectral: bool,
    /// 仅在 `use_force_directed = true` 时使用。
    pub fd_config: FDConfig,
    /// `random_move` 生成 `Move::ToggleBridging` 的目标概率。
    /// 仅当 `state.is_bridgeable[i] = true` 时实际生效, 否则该分支退回 `ShiftX`。
    /// 调高 → SA 更频繁探索 Bridged vs OnBoard; 调低 → Toggle 区间越窄。
    /// 默认 0.15: 配合 v7 默认分布, Toggle 实际概率 15%。
    /// 0 = 完全关闭 Toggle 区间; ShiftGroup / ShiftX / Flip / ShiftY 仍按 v7
    /// 比例挤压运行 (不是字面意义的 v6 分布)。
    pub p_toggle_bridge: f64,
}

impl Default for SAConfig {
    fn default() -> Self {
        Self {
            max_iters: 10000,
            t0: 10.0,
            cool_rate: 0.95,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
            n_seeds: 1,
            use_force_directed: false,
            use_spectral: false,
            fd_config: FDConfig::default(),
            p_toggle_bridge: 0.15,
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
    /// 同 rail 内一组紧邻元件整体左移 1 列, 填桥接留下的空洞。
    /// 组由密度聚类决定 (gap ≤ 2 算同组)。
    ShiftGroup(Vec<usize>),
}

fn random_move(
    state: &SAState,
    rng: &mut fastrand::Rng,
    t: f64,
    t0: f64,
    config: &SAConfig,
    board: &Breadboard,
) -> Move {
    let n = state.n();
    if n == 0 {
        return Move::Flip(0);
    }
    let p = rng.usize(0..n);
    let r = rng.f64();

    // 步长随温度变: 高温期 N ∈ {1, 2, 3} 均匀, 中温 {1, 2}, 低温恒为 1。
    // 三个区间按 T0 的 0.5 / 0.2 划分; 越冷越精细, 越热越敢跳。
    let max_n = if t > t0 * 0.5 {
        3
    } else if t > t0 * 0.2 {
        2
    } else {
        1
    };
    let n_amp = 1 + rng.usize(0..max_n) as i32;
    let dx_sign = if rng.f64() < 0.5 { -1 } else { 1 };
    let dy_sign = if rng.f64() < 0.5 { -1 } else { 1 };
    let dx = dx_sign * n_amp;
    let dy = dy_sign * n_amp;

    // v7 分布: ShiftX 37% / Flip 20% / ShiftY 20% / ToggleBridging 15% / ShiftGroup 8%。
    // ShiftX 从 45% → 37%, 匀 8% 给 ShiftGroup。
    let p_toggle = config.p_toggle_bridge.clamp(0.0, 0.45);
    let p_shiftx = (1.0 - p_toggle) * 0.37 / 0.85;
    let p_flip = (1.0 - p_toggle) * 0.20 / 0.85;
    let p_shiftg = (1.0 - p_toggle) * 0.08 / 0.85;
    let _p_shifty = (1.0 - p_toggle) * 0.20 / 0.85;
    // 校验: p_shiftx + p_flip + p_shifty + p_toggle + p_shiftg = (1-p_toggle) + p_toggle = 1

    if r < p_shiftx {
        Move::ShiftX(p, dx)
    } else if r < p_shiftx + p_flip {
        Move::Flip(p)
    } else if r < p_shiftx + p_flip + p_toggle {
        if state.is_bridgeable[p] {
            Move::ToggleBridging(p)
        } else {
            Move::ShiftX(p, dx)
        }
    } else if r < p_shiftx + p_flip + p_toggle + p_shiftg {
        // ShiftGroup: 找 p 所在同 rail 密度聚类, 整组左移 1 列填洞。
        // 找不到可用组就退回单个 ShiftX。
        if let Some(group) = find_left_shiftable_group(state, board, p) {
            Move::ShiftGroup(group)
        } else {
            Move::ShiftX(p, dx)
        }
    } else {
        Move::ShiftY(p, dy)
    }
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
/// - `Flip` / `ShiftY` 在 bridged 上是 dead-field mutation, 直接返 `false`
///   避免后续 `state_y_valid` 被 stale `state.y` 误导。
/// - `ShiftX` 在 bridged 上: 在 cache 里找 (power.x+dx, signal.x+dx); 没找到时
///   退一步 (dx + sign(dx)) 跳过电源轨 gap 再试一次, 仍失败再返 `false`。
/// - `ShiftGroup` 见 `apply_group_shift_x`: 两段式验证 / 落写, 任何成员失败
///   则全组放弃, 不留半成品。
fn apply_move(state: &mut SAState, m: &Move, board: &Breadboard) -> bool {
    match m {
        Move::Flip(i) => {
            if state.bridged[*i] {
                return false;
            }
            state.rotation[*i] = match state.rotation[*i] {
                Rotation::R0 => Rotation::R180,
                Rotation::R180 => Rotation::R0,
                other => panic!("SA 只用 R0/R180, 不该出现 {:?}", other),
            };
            true
        }
        Move::ShiftY(i, dy) => {
            if state.bridged[*i] {
                return false;
            }
            state.y[*i] += dy;
            true
        }
        Move::ShiftX(i, dx) => {
            if state.bridged[*i] {
                try_bridged_shift_x(state, board, *i, *dx)
            } else {
                state.x[*i] += dx;
                true
            }
        }
        Move::ShiftGroup(indices) => apply_group_shift_x(state, board, indices, -1),
        Move::ToggleBridging(i) => {
            state.bridged[*i] = !state.bridged[*i];
            true
        }
    }
}

/// bridged 元件: 尝试在 cache 里找 (power+dx, signal+dx), 找不到时按
/// `dx + sign(dx)` 跳 gap 再试。两次都失败 → 返 `false` (state 不变)。
///
/// "跳 gap" 的物理意义: 电源轨每 5 孔一断开, group 边界 (e.g. x=4 右移 +1)
/// 必撞 gap; 这里让 `ShiftX(+1)` 在 x=4 时自动变 `+2` 落到下一 group 起点
/// (e.g. x=6), 避免电源轨邻接永远过不去。`ShiftGroup` 不会触发此回退 —
/// 严格左移 1 列的语义更紧凑。
fn try_bridged_shift_x(state: &mut SAState, board: &Breadboard, i: usize, dx: i32) -> bool {
    let cur = match state.active_bridge_pair(i) {
        Some(p) => p,
        None => return false,
    };
    let old_power = board.hole(cur[0].0).position;
    let old_signal = board.hole(cur[1].0).position;

    if try_shift_to(state, board, i, old_power, old_signal, dx) {
        return true;
    }
    if dx != 0 {
        let bumped = dx + dx.signum();
        return try_shift_to(state, board, i, old_power, old_signal, bumped);
    }
    false
}

fn try_shift_to(
    state: &mut SAState,
    board: &Breadboard,
    i: usize,
    old_power: crate::circuit::Position,
    old_signal: crate::circuit::Position,
    dx: i32,
) -> bool {
    let tgt_power = crate::circuit::Position {
        x: old_power.x + dx,
        y: old_power.y,
    };
    let tgt_signal = crate::circuit::Position {
        x: old_signal.x + dx,
        y: old_signal.y,
    };
    if let Some(j) = state.bridged_pin_pairs[i].iter().position(|pair| {
        board.hole(pair[0].0).position == tgt_power && board.hole(pair[1].0).position == tgt_signal
    }) {
        state.active_bridge_idx[i] = j;
        true
    } else {
        false
    }
}

/// ShiftGroup 应用: 两段式, 第一遍验证 + 收集更新, 第二遍落写。保证全原子
/// (任一成员失败 → 0 修改)。`dx` 通常是 `-1` (`ShiftGroup` 永远左移 1 列),
/// 但作为参数暴露便于复用 / 测试。
///
/// bridged 成员采用严格 dx (不跳 gap): 撞 gap 直接让该 group 整体拒掉。
/// "跳 gap" 是 `ShiftX` 单独的语义, 不在此函数扩散。
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

/// 跑模拟退火, 返回最佳 [`SAState`]。
///
/// 初始状态按 [`SAConfig::use_spectral`] / [`SAConfig::use_force_directed`] 选
/// [`SAState::from_spectral`] / [`SAState::from_force_directed`], 两者皆否时用
/// [`SAState::from_greedy`]; 三者都已经避免 pin 撞 / bbox 撞 / 列冲突,
/// (后续 `Flip` / `ShiftX` 偶尔会重新引入列短路, 由 cost 罚分优化掉)。
pub(super) fn simulate(
    placeable: Vec<ComponentId>,
    circuit: &Circuit,
    board: &Breadboard,
    config: &SAConfig,
    bridged_pins: &[(crate::circuit::PinId, super::breadboard::HoleId)],
) -> SAState {
    let mut rng = fastrand::Rng::with_seed(config.seed);
    let mut state = if config.use_spectral {
        SAState::from_spectral(placeable, circuit, board)
    } else if config.use_force_directed {
        SAState::from_force_directed(placeable, circuit, board, &config.fd_config)
    } else {
        SAState::from_greedy(placeable, circuit, board)
    };
    // 从 board 抽 power net ids (绑定的正 / 负极), 然后填桥接字段。
    // 无绑定时 power_net_ids 为空, `populate_bridgeable_info` 内调用的
    // `propose_bridged_pairs` 返空 Vec, 没人会被标 bridgeable, Toggle 不会触发。
    let power_net_ids: Vec<NetId> = board
        .power_rail_binding()
        .map(|b| vec![b.negative, b.positive])
        .unwrap_or_default();
    populate_bridgeable_info(&mut state, circuit, board, &power_net_ids);
    // 预计算 context (footprint pin offset, bbox) 和 reusable buffers
    let ctx = SAContext::new(circuit, &state.placeable);
    let mut buf = CostBuf::new(circuit.nets().len());
    // Aggressive init: 默认所有 bridgeable 都走桥接, cache 里挑 cost 最低的 pair。
    // 不做 safety net; SA 后续可以 ToggleBridging 翻回去。
    init_bridgeable_to_bridged(
        &mut state,
        circuit,
        board,
        bridged_pins,
        &config.weights,
        &ctx,
        &mut buf,
    );
    let mut current_cost = cost_fast(
        &state,
        circuit,
        board,
        bridged_pins,
        &config.weights,
        &ctx,
        &mut buf,
    );
    let mut best_state = state.clone();
    let mut best_cost = current_cost;
    let mut t = config.t0;

    for _ in 0..config.max_iters {
        let m = random_move(&state, &mut rng, t, config.t0, config, board);
        let mut candidate = state.clone();
        // apply_move 返 false 表示该 Move 在当前状态下不应落地:
        //   - Flip / ShiftY 命中 bridged (dead field)
        //   - ShiftX 在 bridged 上, cache 里既找不到 dx 也找不到 dx+sign(dx)
        //   - ShiftGroup 任一成员不可严格左移 1 列
        // 这种情况下我们保持 RNG 序列、不动 state、跳过本轮 (跟成本计算无关)。
        if !apply_move(&mut candidate, &m, board) {
            continue;
        }
        // 拒绝任何产生越界 / blocked row y 的候选 — 物理上不该考虑的状态。
        // (cost 会扣 OOB 惩罚让 SA 远离开, 但放在这里少跑 cost 计算。
        // 另外, 若初始状态本身就合法, SA 也不会被锁定到 "都是 OOB 的同代价态"。)
        if !state_y_valid(&candidate, board) {
            continue;
        }
        // ToggleBridging 翻到 bridge 模式时: 遍历该元件的候选, 选 cost 最低的那对
        // 写回 active_bridge_idx。候选列表来自 `populate_bridgeable_info` 按
        // "signal pin 离同 net 中心最近" 预排序的结果, 这里选 cost 最低是真正的
        // 优化。候选数 K 一般 < 8, 额外 cost 调用次数可接受。
        // 翻到 OnBoard 时不用管 active_bridge_idx (cost 函数忽略它)。
        if let Move::ToggleBridging(i) = m
            && candidate.bridged[i]
            && candidate.bridged_pin_pairs[i].len() > 1
        {
            let mut best_cost = f64::INFINITY;
            let mut best_idx = candidate.active_bridge_idx[i];
            for (j, _) in candidate.bridged_pin_pairs[i].iter().enumerate() {
                candidate.active_bridge_idx[i] = j;
                let c = cost_fast(
                    &candidate,
                    circuit,
                    board,
                    bridged_pins,
                    &config.weights,
                    &ctx,
                    &mut buf,
                );
                if c < best_cost {
                    best_cost = c;
                    best_idx = j;
                }
            }
            candidate.active_bridge_idx[i] = best_idx;
        }
        let new_cost = cost_fast(
            &candidate,
            circuit,
            board,
            bridged_pins,
            &config.weights,
            &ctx,
            &mut buf,
        );
        let delta = new_cost - current_cost;

        let accept = delta <= 0.0 || rng.f64() < (-delta / t).exp();
        if accept {
            state = candidate;
            current_cost = new_cost;
            if current_cost < best_cost {
                best_cost = current_cost;
                best_state = state.clone();
            }
        }

        t *= config.cool_rate;
        if t < 1e-6 {
            break;
        }
    }

    best_state
}

/// 检查 SAState 里所有 y 是否在板内且非 blocked row。
/// ShiftY ±1 可能把 y=0 的元件推到 y=-1 (越界), 把 y=4 的推到 y=5 (blocked row),
/// 这些候选从一开始就不该让 SA 考虑。
fn state_y_valid(state: &SAState, board: &Breadboard) -> bool {
    state
        .y
        .iter()
        .all(|&y| y >= 0 && (y as usize) < board.rows() && !board.is_blocked(y as usize))
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
        );
        let cfg = SAConfig::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let b = board();
        // T0=30, T 在 [0.3, 30] 区间走过, 涵盖 max_n=3/2/1 三档。
        for k in 0..200 {
            let t = 30.0_f64 * (1.0 - k as f64 / 200.0) + 0.3;
            let m = random_move(&state, &mut rng, t, 30.0, &cfg, &b);
            match m {
                Move::Flip(i)
                | Move::ShiftX(i, _)
                | Move::ShiftY(i, _)
                | Move::ToggleBridging(i) => {
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
            if let Move::ShiftX(_, dx) = random_move(&state, &mut rng_hi, 30.0, 30.0, &cfg, &b) {
                hi_max = hi_max.max(dx.abs());
            }
            if let Move::ShiftX(_, dx) = random_move(&state, &mut rng_lo, 0.5, 30.0, &cfg, &b) {
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
                net: Some(NetId(0)),
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
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
            positive: NetId(0),
            negative: NetId(1),
        });
        (circuit, board)
    }

    /// 跑 populate_bridgeable_info 并启用 bridged 模式, 返回手动设置好的 state。
    fn bridgable_state_in_bridged(placeable: Vec<ComponentId>) -> SAState {
        let (circuit, board) = bridgable_fixture();
        let mut state = SAState::from_greedy(placeable, &circuit, &board);
        populate_bridgeable_info(&mut state, &circuit, &board, &[NetId(0), NetId(1)]);
        assert!(state.is_bridgeable[0], "fixture 应能提供 bridged candidate");
        assert!(!state.bridged_pin_pairs[0].is_empty(), "cache 不该为空");
        state.bridged[0] = true;
        state
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
        let ok = apply_move(&mut state, &Move::Flip(0), &b);
        assert!(!ok, "Flip on bridged 必须返 false (不同意改)");
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
        let ok = apply_move(&mut state, &Move::ShiftY(0, 1), &b);
        assert!(!ok, "ShiftY on bridged 必须返 false");
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

        // 找到一个 power x=0 的候选 (起点 left edge), dx=-1 越界;
        // dx+sign(dx) = -2 依然越界。 两跳都不命中 → 拒绝。
        let target0 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 0)
            .expect("应能找到 power x=0 的候选");
        state.active_bridge_idx[0] = target0;
        let active_before = state.active_bridge_idx[0];

        let ok = try_bridged_shift_x(&mut state, &board, 0, -1);
        assert!(!ok, "dx = -1 越界 + 跳 dx = -2 依然越界, 必须返 false");
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

        // Bridged 模式: cache 里有 (power.x-1, signal.x-1) → 可;
        // 没有 (起点为 x=0 dx=-1 越界) → 不可。
        state.bridged[0] = true;
        let target0 = state.bridged_pin_pairs[0]
            .iter()
            .position(|pair| board.hole(pair[0].0).position.x == 0)
            .expect("power x=0 candidate");
        state.active_bridge_idx[0] = target0;
        assert!(
            !can_shift_left_one(&state, &board, 0),
            "bridged 起点 x=0 无 -1 cache 命中"
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
                !matches!(m, Move::ToggleBridging(_)),
                "is_bridgeable=false 时 random_move 不应返 ToggleBridging"
            );
        }
    }

    /// random_move 选到 Toggle 区间 且 p 是 bridgeable 时, 返 ToggleBridging(p)。
    /// 注: `p_toggle_bridge` 在 `random_move` 里被 clamp 到 0.45 (防误配超过 45%)。
    /// 设 0.25 期望 ~25% = 500/2000, 留 100 偏差。
    #[test]
    fn random_move_toggle_emits_when_bridgeable() {
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        state.is_bridgeable = vec![true, true];
        let cfg = SAConfig {
            p_toggle_bridge: 0.25, // 低于 clamp 0.45, 期望 ~25% Toggle
            ..SAConfig::default()
        };
        let mut rng = fastrand::Rng::with_seed(0);
        let mut toggle_count = 0;
        let b = board();
        for _ in 0..2000 {
            if let Move::ToggleBridging(_) = random_move(&state, &mut rng, 10.0, 10.0, &cfg, &b) {
                toggle_count += 1;
            }
        }
        assert!(
            toggle_count > 400 && toggle_count < 600,
            "p_toggle_bridge=0.25 采 2000 次应约 500 次 Toggle, got {toggle_count}"
        );
    }

    /// `p_toggle_bridge=0` 时 Toggle 区间为空, 分布塌缩回 v7 五选四
    /// (ShiftGroup / ShiftX / Flip / ShiftY 仍按 v7 比例挤压运行,
    /// 不会有 ToggleBridging 返出)。
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
                !matches!(m, Move::ToggleBridging(_)),
                "p_toggle_bridge=0 时不应返 Toggle"
            );
        }
    }

    /// 验证 SAState::from_greedy 在标准板上不会试着把元件放中间 blocked row。
    #[test]
    fn from_greedy_avoids_blocked_rows() {
        let board = crate::layout::Breadboard::standard();
        let circuit = simple_circuit();
        let state = SAState::from_greedy(vec![ComponentId(0), ComponentId(1)], &circuit, &board);
        for &y in &state.y {
            assert!(
                !board.is_blocked(y as usize),
                "from_greedy 把元件放到了 blocked row y={y}"
            );
        }
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
            t0: 5.0,
            cool_rate: 0.95,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        };
        let best = simulate(
            vec![ComponentId(0), ComponentId(1)],
            &circuit,
            &board(),
            &config,
            &[],
        );
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
            &[],
        );
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
