//! knead-net: 电路解析与建模 + 面包板布局。
//!
//! 模块组织:
//! - [`circuit`][]: 领域模型 (格式无关,只有数据结构)
//! - [`input`]:   各种文件格式的解析器, 每个都把自己的格式转成 [`Circuit`]
//! - [`layout`][]:  面包板布局 (把 Circuit 投影到 Breadboard 上)
//! - [`render`][]: 把布局结果渲染成 SVG (调试用)

pub mod circuit;
pub mod input;
pub mod layout;
pub mod render;

// 把最常用的领域类型提到 crate 根, 用 `knead_net::Circuit` 就能拿到
pub use circuit::{
    Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin, PinId,
    Position,
};
pub use layout::{
    AnnealMetrics, Breadboard, BridgeInitial, BridgePolicy, CancellationToken, Hole, HoleId,
    INTER_BOARD_GAP_COLS, InitializerFamily, Layout, LayoutError, LayoutPreparation,
    LayoutProgress, LayoutSnapshot, Occupancy, Occupant, PathFinderRouter, PinHole,
    PlacedFootprint, Placement, Polarity, PowerRail, PowerRailBinding, PowerRailBindings,
    PowerRailMatch, PowerRailSide, PowerRails, PowerStrip, Preset, ProgressOptions, Region,
    Rotation, Router, SAConfig, Weights, Wire, WireId, prepare_for_layout,
    prepare_for_layout_with_individual_power_nets, prepare_for_layout_with_power_nets,
    spectral_debug_positions, standard_power_rails, wide_power_rails_800,
};
