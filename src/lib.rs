//! knead-net: 电路解析与建模。
//!
//! 模块组织:
//! - [`circuit`]: 领域模型 (格式无关,只有数据结构)
//! - [`input`]:   各种文件格式的解析器, 每个都把自己的格式转成 [`Circuit`]

pub mod circuit;
pub mod input;

// 把最常用的领域类型提到 crate 根, 用 `knead_net::Circuit` 就能拿到
pub use circuit::{Circuit, Component, ComponentId, Net, NetId, Pin, PinId};
