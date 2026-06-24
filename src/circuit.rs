//! 领域模型: component / pin / net 以及链接它们的 ID。
//!
//! 这个模块是格式无关的 — JSON、KiCad .net 以及任何其他输入源
//! 都会转换 *成* 这些类型。具体的解析器见 `input::*`。

// 让 pin 有所属的 component
#[derive(Debug, Clone, Copy)]
pub struct ComponentId(pub(crate) usize);

#[derive(Debug, Clone, Copy)]
pub struct PinId(pub(crate) usize);

#[derive(Debug, Clone, Copy)]
pub struct NetId(pub(crate) usize);

#[derive(Debug)]
pub struct Circuit {
    pub(crate) components: Vec<Component>,
    pub(crate) pins: Vec<Pin>,
    pub(crate) nets: Vec<Net>,
}

#[derive(Debug)]
pub struct Component {
    pub(crate) id: ComponentId,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) pins: Vec<PinId>,
}

#[derive(Debug)]
pub struct Pin {
    pub(crate) id: PinId,

    pub(crate) component: ComponentId,

    pub(crate) name: String,

    pub(crate) net: Option<NetId>,
}

#[derive(Debug)]
pub struct Net {
    pub(crate) id: NetId,

    pub(crate) name: String,

    pub(crate) pins: Vec<PinId>,
}
