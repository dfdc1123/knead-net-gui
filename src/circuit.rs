//! 领域模型: component / pin / net / footprint 以及链接它们的 ID。
//!
//! 这个模块是格式无关的 — JSON、KiCad .net 以及任何其他输入源
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
/// 1 单位 = 1 个面包板孔 = 2.54mm。`.kicad_mod` 里的 mm 坐标
/// 在解析时会自动除以 2.54 换算成"孔数", 不能整除会 panic
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
    pub(crate) value: Option<String>,
    pub(crate) pins: Vec<PinId>,
    pub(crate) footprint: Option<FootprintId>,
}

#[derive(Debug)]
pub struct Pin {
    pub(crate) id: PinId,

    pub(crate) component: ComponentId,

    /// pin num, 跟 .kicad_mod 里 (pad "X") 的 X 一致
    pub(crate) num: String,

    /// KiCad netlist 里的 (pinfunction "B"/"C"/"E"/"K"/"A" ...), 用来识别极性
    pub(crate) pinfunction: Option<String>,

    pub(crate) net: Option<NetId>,
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
}

impl Circuit {
    /// 替换整个 footprint 注册表
    ///
    /// 主流程是: `From<CircuitInput> for Circuit` (或 `NetlistInput::into_circuit`)
    /// 得到 Circuit, 然后调用本方法把 footprint 注册表灌进去。
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

    pub fn footprint(&self) -> Option<FootprintId> {
        self.footprint
    }

    pub fn pins(&self) -> &[PinId] {
        &self.pins
    }
}
