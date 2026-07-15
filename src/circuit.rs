//! 领域模型: component / pin / net / footprint 以及链接它们的 ID。
//!
//! 这个模块是格式无关的 — KiCad `.kicad_pcb` 以及任何其他输入源
//! 都会转换 *成* 这些类型。具体的解析器见 `input::*`。

// 让 pin 有所属的 component
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComponentId(pub(crate) usize);

impl ComponentId {
    pub fn raw(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinId(pub(crate) usize);

impl PinId {
    pub fn raw(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetId(pub(crate) usize);

impl NetId {
    pub fn raw(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FootprintId(pub(crate) usize);

impl FootprintId {
    pub fn raw(self) -> usize {
        self.0
    }
}

/// 物理位置 (x, y), 整数坐标
///
/// 1 单位 = 1 个面包板孔 = 2.54mm。KiCad 格式里的 mm 坐标
/// 在解析时会先四舍五入到最近孔 (容差 1e-9), 完全对齐不到整数倍孔距才 panic。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug)]
pub struct Circuit {
    pub(crate) components: Vec<Component>,
    pub(crate) pins: Vec<Pin>,
    pub(crate) nets: Vec<Net>,
    pub(crate) footprints: Vec<Footprint>,
}

#[derive(Debug)]
pub struct Component {
    pub(crate) id: ComponentId,
    /// KiCad ref, 例如 "R1", "Q1", "D1"
    pub(crate) ref_: String,
    /// libsource 里的 part, 例如 "R", "NPN", "LED"
    pub(crate) kind: String,
    /// KiCad (value ...) 字段: 电阻的阻值 ("220"), IC 的型号等
    #[allow(dead_code)]
    pub(crate) value: Option<String>,
    pub(crate) pins: Vec<PinId>,
    pub(crate) footprint: Option<FootprintId>,
    /// **是否允许桥接**: true 表示这个元件是 2-pin 跨接的候选
    /// (例如从 power rail 跨到主区的电阻)。本身**只**是 flag —
    /// 是否真的走 [`Placement::Bridged`] 由 [`Layout::place_sa`] 内的
    /// `ToggleBridging` 扰动决定。
    /// 由 pcb 解析层的 [`crate::input::pcb::auto_mark_bridgeable`]
    /// 按当前 power binding 从零重算 (规则: 2 pin + 一腿 power net + 另一腿 signal net)。
    /// 若调用方需要手动 override，应在最后一次 layout preparation 之后设置。
    pub bridgeable: bool,
}

impl Default for Component {
    fn default() -> Self {
        Self {
            id: ComponentId(0),
            ref_: String::new(),
            kind: String::new(),
            value: None,
            pins: Vec::new(),
            footprint: None,
            bridgeable: false,
        }
    }
}

#[derive(Debug)]
pub struct Pin {
    pub(crate) id: PinId,

    pub(crate) component: ComponentId,

    /// pin num, 跟 KiCad pad 编号一致
    pub(crate) num: String,

    /// KiCad netlist 里的 (pinfunction "B"/"C"/"E"/"K"/"A" ...), 用来识别极性
    pub(crate) pinfunction: Option<String>,

    pub(crate) net: Option<NetId>,

    /// 在所属 footprint 的 `Footprint::pins` 里的下标, 显式建立 1:1 对应。
    /// 解析阶段 (`pcb.rs`) 填入; 之后 lookup physical pin 时不再按名字匹配。
    pub(crate) physical_pin_index: usize,
}

impl Pin {
    pub fn id(&self) -> PinId {
        self.id
    }

    pub fn component(&self) -> ComponentId {
        self.component
    }

    /// KiCad netlist 里的 (pin (num "X")) — 元件库里的引脚编号
    pub fn num(&self) -> &str {
        &self.num
    }

    /// KiCad netlist 里的 (pinfunction "K"/"A"/"C"/"B"/"E"/...) — 语义名
    pub fn pinfunction(&self) -> Option<&str> {
        self.pinfunction.as_deref()
    }

    /// 该 pin 连接到的 net; None = unconnected
    pub fn net(&self) -> Option<NetId> {
        self.net
    }

    /// 在所属 footprint 的 physical pins 数组里的下标
    pub fn physical_pin_index(&self) -> usize {
        self.physical_pin_index
    }
}

#[derive(Debug)]
pub struct Net {
    pub(crate) id: NetId,

    pub(crate) name: String,

    pub(crate) pins: Vec<PinId>,
}

impl Net {
    pub fn id(&self) -> NetId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pins(&self) -> &[PinId] {
        &self.pins
    }
}

/// 物理封装上的一个 pin, 包含名字和在封装局部坐标里的偏移
#[derive(Debug, Clone)]
pub struct PhysicalPin {
    pub(crate) name: String,
    pub(crate) offset: Position,
}

impl PhysicalPin {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn offset(&self) -> Position {
        self.offset
    }
}

/// 物理封装: 一种元件的"焊盘 / 引脚布局"模板
///
/// 例如 "TO92" 是一种封装, 上面有 B / C / E 三个引脚,
/// 在封装局部坐标里分别位于 (1, 0) / (0, 0) / (2, 0)。
#[derive(Debug, Clone)]
pub struct Footprint {
    pub(crate) id: FootprintId,
    pub(crate) name: String,
    pub(crate) pins: Vec<PhysicalPin>,
}

impl Footprint {
    pub fn id(&self) -> FootprintId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pins(&self) -> &[PhysicalPin] {
        &self.pins
    }

    /// 通过 Pin 里记录的 [`Pin::physical_pin_index`] 拿到对应的物理引脚。
    ///
    /// 这是 Pin → PhysicalPin 的显式 1:1 映射, 替代了之前按名字的 `.find()`。
    pub fn physical_pin_for(&self, pin: &Pin) -> Option<&PhysicalPin> {
        self.pins.get(pin.physical_pin_index)
    }
}

impl Circuit {
    /// 创建一个空电路 (0 元件, 0 pin, 0 net, 0 footprint)。
    /// 主要给“只渲染板子几何”的调试场景用。
    pub fn empty() -> Self {
        Self {
            components: Vec::new(),
            pins: Vec::new(),
            nets: Vec::new(),
            footprints: Vec::new(),
        }
    }

    /// 替换整个 footprint 注册表
    ///
    /// `NetlistInput::into_circuit` 通过参数 `&[Footprint]` 直接吃 footprint 注册表,
    /// 走 netlist 主流程时**不需要**额外再调本方法; 本方法主要用于
    /// 不通过 `NetlistInput` 的入口, 或测试时手动灌入。
    pub fn set_footprints(&mut self, footprints: impl IntoIterator<Item = Footprint>) {
        self.footprints = footprints.into_iter().collect();
    }

    pub fn components(&self) -> &[Component] {
        &self.components
    }

    pub fn pins(&self) -> &[Pin] {
        &self.pins
    }

    pub fn nets(&self) -> &[Net] {
        &self.nets
    }

    pub fn footprints(&self) -> &[Footprint] {
        &self.footprints
    }
}

impl Component {
    pub fn id(&self) -> ComponentId {
        self.id
    }

    /// KiCad ref, 例如 "R1", "Q1", "D1"
    pub fn ref_(&self) -> &str {
        &self.ref_
    }

    /// libsource 里的 part, 例如 "R", "NPN", "LED"
    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    pub fn footprint(&self) -> Option<FootprintId> {
        self.footprint
    }

    pub fn pins(&self) -> &[PinId] {
        &self.pins
    }
}
