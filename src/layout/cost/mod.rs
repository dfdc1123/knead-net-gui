//! 模拟退火用的成本函数: MST 走线估算 + pin 碰撞 + bbox 碰撞 + 越界 + 列冲突 + 紧凑度。
//!
//! 设计要点:
//! - **MST (Minimum Spanning Tree) on pin positions**: 每个 net 算一次 Kruskal
//!   MST, 边长按 breadboard 物理距离计算:
//!   - **同列同 rail: 0** (rail 短接, 无需 wire)
//!   - **同 rail 不同 col: |Δcol|** (走 jumper wire 在同一行)
//!   - **同 col 不同 rail: |Δrow|** (跨中央通道)
//!   - **不同 col 不同 rail: |Δcol| + |Δrow|** (Manhattan)
//!
//!   比 2D HPWL 准 — 普通 HPWL 把 "同列不同 row 同 rail" 算成 Δrow, MST 直接 0,
//!   推动 SA 主动寻找 rail 短接的低跳线数布局。
//! - **紧凑度**: 按 rail 分组算 union bbox 水平跨度加和, 阻止 SA 停在"零冲突但水平留白大"的状态。
//! - 成本是各项**加权和**, 权在 [`Weights`] 里调。
//! - `SAState` 是 SA 内部状态, 只在 layout 子模块内共享; v2 起每个元件显式
//!   持有 `(x, y, rotation)`, 不再由 order 推 x。

use crate::circuit::Circuit;
use crate::layout::breadboard::Breadboard;

mod bridge;
pub(crate) mod context;
mod cost_fast;
mod mst;
#[macro_use]
mod profile;
mod spectral;
mod state;
#[cfg(test)]
mod tests;

// --- 公共 API 重导出 (供 layout/, sa.rs, main.rs 使用) ---
pub(crate) use bridge::{
    BridgeInitContext, initialize_bridging, populate_bridgeable_info, state_hard_legal,
};
pub use bridge::{BridgeInitial, BridgePolicy};
pub(crate) use context::CostBuf;
pub use context::{CompInfo, SAContext};
pub(crate) use cost_fast::{cost_breakdown_with_problem, cost_fast};
pub use profile::{dump_cost_profile, reset_cost_profile};
pub use state::SAState;

// profile 模式时, 把 profile::cost_profile 提到 cost:: 层级, 让宏里的
// `$crate::layout::cost::cost_profile::X` 路径保持原样可解析。
#[cfg(profile_cost)]
pub(crate) use profile::cost_profile;

// `cp_*` 宏通过 `#[macro_export]` 导出到 crate 根, 在 cost_fast.rs / mod.rs 里直接调用
// (不需 use, 宏在 crate 根的解析由 macro_export 负责)。

// 测试专用 re-export: mst_wire_length 和 propose_bridged_pair 本身都是 #[cfg(test)] 的,
#[cfg(test)]
pub(crate) use bridge::propose_bridged_pair;
#[cfg(test)]
pub(crate) use mst::mst_wire_length;

/// SA 成本函数的九项权重。
///
/// 成本 = `mst * MST_sum + pin_overlap * pin_pin_碰撞 + b_box_overlap * bbox_重叠格数
///       + column_conflict * rail owner 最少移出 endpoint 数 + out_of_bounds * 越界 pin 数
///       + compactness * (按 rail 分组的 union bbox 面积之和)
///       + row_squash * Σ max(0, n_comps - unique_min_y) (按 rail, 推元件散布到不同行)
///       + rail_crossing * [用了 ≥2 个 rail]
///       + mst_congestion * 超出 rail 空孔容量的 MST degree`
///
/// 默认值见 [`Weights::default`], 经验起点; 真用时按板子拥挤程度调。
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    /// MST (Minimum Spanning Tree) 走线总长的权重。
    /// 每个 net 跑一次 Kruskal, 边权按 breadboard 物理距离 (同 rail = 0,
    /// 不同 rail = Manhattan), sum 起来乘以本权重。
    /// 比纯 HPWL 准: HPWL 算同列不同 row 的距离仍按 Δrow, MST 直接 0。
    pub mst: f64,
    /// pin-pin 碰撞对数 (`cost_fast` 中数 n-1 + ... + 1)。
    pub pin_overlap: f64,
    /// bbox 碰撞总格数 (本体撞 pin 也算)。一般比 pin_overlap 略高 — 本体挤到
    /// 其它元件身体上比 pin 互相碰还要糟糕 (后面 wire 还会避开本体)。
    pub b_box_overlap: f64,
    /// 同 rail owner 不一致时，为使剩余 owner 一致至少要移走的 endpoint 数。
    /// 即每条 rail 的 `endpoint_count - dominant_owner_count`，与遍历顺序无关。
    pub column_conflict: f64,
    /// 越界 pin 数 (在 rail_id == `u32::MAX` 的"板外"孔上)。
    pub out_of_bounds: f64,
    /// 紧凑度: 按 rail 分组, 每组算 union bbox 水平跨度 `(max_x - min_x + 1)`,
    /// 各 rail 加和。只算 x, 不再算 y — y 方向由 row_squash 管。
    pub compactness: f64,
    /// 同时使用 2+ 个 rail 时的额外固定惩罚, 鼓励同 rail 排布。
    /// 跨 rail 至少要一根 ~3 孔 jumper, 这项比单 cell 紧凑更贵。
    pub rail_crossing: f64,
    /// 纵向利用率惩罚: 同一 rail 内, 元件数量 vs 实际占用的行数。
    /// `penalty = Σ max(0, n_comps - unique_min_y) (按 rail 分组, n_comps 是元件数, unique_min_y 是该 rail 上 bbox min_y 的不同行数)`, 推 SA 把元件散布到不同行,
    /// 避免所有元件挤在同一行导致水平跨度过大。
    pub row_squash: f64,
    /// MST 拥塞惩罚: 对 MST 中每条超过 rail 容量 (空孔数) 的边加罚。
    /// 推动 SA 产出的布局对容量约束友好, 避免后续路由产生 relay 列。
    /// 0 = 不启用。
    pub mst_congestion: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            // 一根 5 孔 wire 省下 ~25 成本 (mst=5); 大权重推动 SA 压低跳线数。
            mst: 5.0,
            // 一次 pin 碰撞 = 让 SA 宁愿多绕 50-100 孔也不撞
            pin_overlap: 100.0,
            // bbox 重叠基本也当硬约束, 跟 pin 碰撞同量级 (一个孔算 1)。
            b_box_overlap: 100.0,
            // 同列不同 net 的 pin 会被面包板竖向 rail 短接, 这是物理电气短路,
            // 不能让走线"治愈"。惩罚拉到 out_of_bounds 同级, 让 SA 当作硬约束。
            column_conflict: 1_000_000.0,
            // 越界基本不允许; 巨大惩罚让 SA 直接拒绝
            out_of_bounds: 1_000_000.0,
            // 紧凑度: 1 cell 水平跨度 ≈ 0.5 MST cell 的代价, 让 MST 仍有空间优化跨列 net,
            // 但水平空隙会被这股力挤掉。
            compactness: 0.5,
            // 跨 rail = 多一根 jumper + 视觉割裂, 取约 5 cell MST, 比单 cell 紧凑贵
            // 但比 column_conflict 软得多, 不会让 SA 为了"必须跨 rail 的电路"去撞列冲突。
            rail_crossing: 5.0,
            // 纵向挤压: 同一 rail 内元件挤在少量行 → 加罚。
            // 1.0 等价于 ~2 cell² 紧凑度, 比 MST 的 5.0 轻, 给 SA 温和推力。
            row_squash: 1.0,
            mst_congestion: 2.0,
        }
    }
}

pub fn cost(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, crate::layout::breadboard::HoleId)],
    w: &Weights,
) -> f64 {
    let mut ctx = SAContext::new(circuit, &state.placeable);
    ctx.fill_bridged_bboxes(state, circuit, board, bridged_pins);
    let mut buf = CostBuf::new(circuit.nets().len(), board.num_rails(), board.main_rows());
    cost_fast(state, circuit, board, bridged_pins, w, &ctx, &mut buf)
}

pub(crate) fn cost_with_problem(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    problem: &crate::layout::problem::AnnealProblem,
    w: &Weights,
) -> f64 {
    let mut ctx = SAContext::new(circuit, &state.placeable);
    ctx.fill_bridged_bboxes(state, circuit, board, &[]);
    ctx.fill_problem(problem);
    let mut buf = CostBuf::new(circuit.nets().len(), board.num_rails(), board.main_rows());
    cost_fast(state, circuit, board, &[], w, &ctx, &mut buf)
}
