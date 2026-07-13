//! 与 UI 框架无关的布局进度快照。
//!
//! 回调可能从 Rayon worker 线程触发；GUI 适配层应把事件送入 channel，
//! 再由自己的主线程/异步任务发布，而不是在回调里直接操作窗口。

use super::{Placement, Wire};

/// 一帧可直接用于渲染的布局，不暴露 SA 的内部缓存。
#[derive(Debug, Clone)]
pub struct LayoutSnapshot {
    pub placements: Vec<Option<Placement>>,
    pub wires: Vec<Wire>,
}

/// Step 3 所需的几个稳定阶段。
#[derive(Debug, Clone)]
pub enum LayoutProgress {
    /// 展示 seed 的频谱初排；此时尚未开始退火。
    SpectralInitial { seed: u64, snapshot: LayoutSnapshot },
    /// 展示 seed 的退火抽样帧。`iteration` 从 0 开始。
    Annealing {
        seed: u64,
        iteration: usize,
        total_iterations: usize,
        current_cost: f64,
        best_cost: f64,
        snapshot: LayoutSnapshot,
    },
    /// 所有 seed 完成后选出的全局最优布局。
    PlacementComplete {
        seed: u64,
        cost: f64,
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
