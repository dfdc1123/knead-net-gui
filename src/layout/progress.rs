//! 与 UI 框架无关的布局进度快照。
//!
//! 回调可能从 Rayon worker 线程触发；GUI 适配层应把事件送入 channel，
//! 再由自己的主线程/异步任务发布，而不是在回调里直接操作窗口。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Placement, Wire};

/// 可跨线程共享的协作式取消标记。
///
/// 取消不会丢弃已有结果：每个 SA seed 会返回自己的 best-so-far，布局层仍从中
/// 选全局最低 cost，调用方随后可以照常 routing。
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.0
    }
}

/// 一帧可直接用于渲染的布局，不暴露 SA 的内部缓存。
#[derive(Debug, Clone)]
pub struct LayoutSnapshot {
    pub placements: Vec<Option<Placement>>,
    pub wires: Vec<Wire>,
}

/// Counters for one annealing seed. Every attempt lands in exactly one of
/// `no_candidate`, `invalid`, or `evaluated`; accepted is a subset of evaluated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnnealMetrics {
    pub attempted: usize,
    pub no_candidate: usize,
    pub invalid: usize,
    pub evaluated: usize,
    pub accepted: usize,
}

/// Strategy used to construct one annealing seed's legal initial placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializerFamily {
    Greedy,
    Spectral,
    ForceDirected,
    RandomizedGreedy,
}

/// Step 3 所需的几个稳定阶段。
#[derive(Debug, Clone)]
pub enum LayoutProgress {
    /// 展示 seed 完成 bridge 初始化后的真实退火起点及其成本。
    InitialPlacement {
        seed: u64,
        initializer: InitializerFamily,
        cost: f64,
        snapshot: LayoutSnapshot,
    },
    /// 展示 seed 的退火抽样帧。`iteration` 从 0 开始。
    Annealing {
        seed: u64,
        iteration: usize,
        total_iterations: usize,
        current_cost: f64,
        best_cost: f64,
        metrics: AnnealMetrics,
        snapshot: LayoutSnapshot,
    },
    /// 一个并行 seed 已完成；快照是该 seed 的最终候选，可用于维护当前全局最佳预览。
    SeedComplete {
        seed: u64,
        cost: f64,
        completed: usize,
        total: usize,
        observed: bool,
        snapshot: LayoutSnapshot,
    },
    /// 所有 seed 完成后选出的全局最优布局。
    PlacementComplete {
        seed: u64,
        cost: f64,
        metrics: AnnealMetrics,
        cancelled: bool,
        snapshot: LayoutSnapshot,
    },
    /// 路由完成后的最终布局。
    RoutingComplete { snapshot: LayoutSnapshot },
}

/// 进度展示策略。它只控制拷贝/回调频率，不参与算法决策。
#[derive(Debug, Clone, Copy)]
pub struct ProgressOptions {
    /// 展示第几个 seed；0 表示 `SAConfig::seed` 对应的第一个 seed。
    pub display_seed: usize,
    /// 每隔多少次 SA 迭代产生一帧；0 会被视为 1。
    pub sample_every: usize,
}

impl Default for ProgressOptions {
    fn default() -> Self {
        Self {
            display_seed: 0,
            sample_every: 1_000,
        }
    }
}
