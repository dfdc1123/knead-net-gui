//! 面包板布局: 把 Circuit 投影到 Breadboard 上。
//!
//! 模块组织:
//! - [`breadboard`][]: 物理结构 (main board + 电源轨, 尺寸参数化;
//!   `standard()` 默认 30×12 main + 上下各两组 5×5 电源轨)
//! - [`placement`]: 摆放 (位置 + 旋转) → 投影到具体 HoleId
//! - [`occupancy`][]: 当前孔占用 (派生, 不缓存)
//! - [`cost`]: SA 成本函数 (MST / pin 碰撞 / bbox / 紧凑度 / 桥接启发式等)
//! - [`sa`][]: 模拟退火求解器
//! - [`routing`][]: 接线 (Wire, Router trait + PathFinder 实现)
//! - [`Layout`]: 顶层容器, 持有 Circuit 引用 + placements + wires

pub mod breadboard;
pub mod cost;
pub mod occupancy;
pub mod preprocess;
pub mod placement;
pub mod routing;
pub mod sa;

pub use breadboard::{
    Breadboard, Hole, HoleId, Polarity, PowerRail, PowerRailBinding, PowerRails, PowerStrip,
    Region, standard_power_rails,
};
pub use cost::Weights;
pub use occupancy::{Occupancy, Occupant};
pub use placement::{BBox, PinHole, PlacedFootprint, Placement, Rotation};
pub use routing::{PathFinderRouter, Router, Wire, WireId};
pub use sa::SAConfig;

use crate::circuit::{Circuit, ComponentId, NetId, PinId, Position};

mod debug;
mod layout_impl;
#[cfg(test)]
mod tests;

/// 频谱布局调试输出: 返回 (v₂, v₃, round 后 placement), 并打印频谱值和格点映射。
///
/// **注意**: `v₂` / `v₃` 字段**目前是占位** (`vec![0.0; n]`), 只打印 round
/// 后的 `(x, y)` (按 x 排序)。频谱值的具体内容还需要进一步接线到 SAState。
pub fn spectral_debug_positions(
    circuit: &Circuit,
    board: &Breadboard,
    preprocess: &crate::layout::preprocess::PreprocessResult,
) -> (Vec<f64>, Vec<f64>, Vec<Option<Placement>>) {
    use crate::circuit::Position;
    use crate::layout::cost::SAState;

    let placeable: Vec<ComponentId> = circuit
        .components()
        .iter()
        .filter_map(|c| {
            c.footprint()?;
            Some(c.id())
        })
        .collect();

    if placeable.is_empty() {
        return (vec![], vec![], vec![None; circuit.components().len()]);
    }

    // 调试用: 用一个固定 seed 让跨进程可复现。调成 0 / 1 / 42 跟生产不同。
    let state = SAState::from_spectral(placeable, circuit, board, 0xDEAD_BEEF, preprocess);
    let n = state.n();

    // 打印表格 (按 round 后的 x 排序)
    eprintln!("\n=== 频谱嵌入 + round 结果 (按 x 排序) ===");
    eprintln!("{:<6} {:<14}", "ref", "round (x,y)");
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| state.x[i]);
    for &idx in &order {
        let comp = &circuit.components()[state.placeable[idx].raw()];
        eprintln!(
            "{:<6}    ({:>3},{:>3})",
            comp.ref_(),
            state.x[idx],
            state.y[idx]
        );
    }
    eprintln!();

    let mut placements: Vec<Option<Placement>> = vec![None; circuit.components().len()];
    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        placements[comp_id.raw()] = Some(Placement::OnBoard {
            position: Position {
                x: state.x[idx],
                y: state.y[idx],
            },
            rotation: state.rotation[idx],
        });
    }

    let v2: Vec<f64> = vec![0.0; n]; // placeholder
    let v3: Vec<f64> = vec![0.0; n]; // placeholder
    (v2, v3, placements)
}

/// 一列上的某个 pin / wire 端点, 捎带它的 net 信息。
///
/// [`LayoutError::ColumnConflict`] 用这个告诉你 "col X 的 a 和 b 被纵向 rail 连起来了,
/// 它们不在同一 net, 算短路" —— 拿这个 net 信息能直接看出是哪个 net 被连到了哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnEndpoint {
    /// pin + 所属 net (`None` = 未连接, e.g. unconnected-pad)
    Pin { pin: PinId, net: Option<NetId> },
    /// wire 端点, 必有 net
    Wire { wire: WireId, net: NetId },
}

/// 布局错误。`apply` / `validate` / `from_layout` 都会返回这个。
///
/// `apply` 产生 `OutOfBounds` / `NoFootprintPad` (Bridged 路径还会产生 `PinCollision`)。
/// `validate` / `from_layout` 在跨 placement 层面额外产生
/// `NoFootprint` / `BBoxOverlap` / `WireConflict` / `ColumnConflict`,
/// 并透传 `apply` 的全部错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// Component 没有 footprint
    NoFootprint { component: ComponentId },
    /// 某个 pin 算出来落在板外
    OutOfBounds {
        component: ComponentId,
        pin: PinId,
        hole: Position,
    },
    /// pin 跟已摆的元件的 pin 撞同一个孔
    PinCollision {
        component: ComponentId,
        pin: PinId,
        hole: HoleId,
    },
    /// wire path 跟已占用的孔冲突 (跟 pin 或别的 wire)
    WireConflict { wire: WireId, hole: HoleId },
    /// Component 引用了 footprint 里不存在的 pad (按 num 找)
    NoFootprintPad {
        component: ComponentId,
        pin: PinId,
        pad_name: String,
    },
    /// 同列上两个不同 net 的 pin / wire 被面包板纵向 rail 短路。
    /// 面包板的每列内部连通, 所以同列不同 row 上的 pin 也会被连起来。
    ColumnConflict {
        column: i32,
        a: ColumnEndpoint,
        b: ColumnEndpoint,
    },
    /// 两个元件的包围盒在板上重叠 — 元件本体互相碰撞 (不论是 pin 撞本体、
    /// 本体撞 pin 还是本体撞本体都报这个)。报告发生冲突的第一个孔。
    BBoxOverlap {
        a: ComponentId,
        b: ComponentId,
        hole: HoleId,
    },
}

/// 顶层布局: 持有 Circuit 引用 + 每个 component 的 placement + 所有 wire。
///
/// 跟 Circuit 本身**解耦**: Component 不携带 placement, Layout 单独管理。
/// 这让 Circuit 可以独立 serialize / 在不同 layout 间切换。
#[derive(Debug)]
pub struct Layout<'c> {
    pub(crate) circuit: &'c Circuit,
    pub(crate) placements: Vec<Option<Placement>>,
    pub(crate) wires: Vec<Wire>,
}
