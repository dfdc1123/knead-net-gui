//! KiCad 简化 netlist (`.net`) 解析器。
//!
//! 只关心电路结构需要的那部分:
//! - `(components (comp ...))` — 元件
//! - `(nets (net ... (node ...)))` — 连线
//!
//! `(libparts ...)` 这里**不解析**——我们用 .net 里 instance 的 pin num
//! 直接当 Pin.name, 跟 .kicad_mod 里的 pad 编号对得上。
//! 以后想拿 libpart 的功能名 (B/C/E/K/A), 再补 libpart 查表。

use std::collections::HashMap;

use super::sexp::{ParseError, Sexp, parse};
use crate::circuit::{
    Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, Pin, PinId,
};

pub struct NetlistInput {
    pub components: Vec<NetlistComp>,
    pub nets: Vec<NetlistNet>,
}

pub struct NetlistComp {
    /// KiCad ref, 例如 "R1", "Q1"
    pub ref_: String,
    /// libsource 里的 part, 例如 "R", "NPN", "LED" — 当作 Component.kind
    pub libsource_part: String,
    /// KiCad (value ...) 字段, 例如电阻的 "220", 拿去当 Component.value
    pub value: Option<String>,
    /// 完整 footprint 引用, 例如 "LED_THT:LED_D5.0mm"
    pub footprint_ref: String,
    /// instance 的 pin num 列表 (来自 units/unit/pins/pin/num)
    pub pin_nums: Vec<String>,
}

pub struct NetlistNet {
    pub name: String,
    pub nodes: Vec<NetlistNode>,
}

pub struct NetlistNode {
    pub ref_: String,
    /// KiCad 里这个字段叫 "pin", 但内容是 num 不是 name
    pub pin_num: String,
    /// KiCad node 里的 (pinfunction "B"/"C"/"E"/"K"/"A" ...), 传给 Pin.pinfunction
    pub pinfunction: Option<String>,
}

impl NetlistInput {
    /// 把 NetlistInput 变成 Circuit, 用 footprints 解析 Component.footprint
    pub fn into_circuit(self, footprints: &[Footprint]) -> Circuit {
        // 1. 建 footprint 名字 → FootprintId 的索引
        let footprint_by_name: HashMap<&str, FootprintId> = footprints
            .iter()
            .map(|fp| (fp.name.as_str(), fp.id))
            .collect();

        // 2. 建 component + pin
        let mut components: Vec<Component> = Vec::with_capacity(self.components.len());
        let mut pins: Vec<Pin> = Vec::new();
        // (ref, pin_num) → PinId, 给后面解析 net 的 node 用
        let mut pin_lookup: HashMap<(String, String), PinId> = HashMap::new();

        for comp_in in self.components {
            let component_id = ComponentId(components.len());
            let mut comp_pin_ids: Vec<PinId> = Vec::with_capacity(comp_in.pin_nums.len());

            for pin_num in &comp_in.pin_nums {
                let pin_id = PinId(pins.len());
                pins.push(Pin {
                    id: pin_id,
                    component: component_id,
                    num: pin_num.clone(),
                    pinfunction: None,
                    net: None,
                });
                pin_lookup.insert((comp_in.ref_.clone(), pin_num.clone()), pin_id);
                comp_pin_ids.push(pin_id);
            }

            // 解析 footprint ref: "LIB:Name" → "Name", 然后在注册表里查
            let footprint = footprint_by_name
                .get(strip_library_prefix(&comp_in.footprint_ref).as_str())
                .copied();

            components.push(Component {
                id: component_id,
                ref_: comp_in.ref_,
                kind: comp_in.libsource_part,
                value: comp_in.value,
                pins: comp_pin_ids,
                footprint,
                bridgeable: false, // 由 auto_mark_bridgeable 后处理
            });
        }

        // 3. 建 net + 连接
        let mut nets: Vec<Net> = Vec::with_capacity(self.nets.len());
        for net_in in self.nets {
            let net_id = NetId(nets.len());
            let mut net_pins: Vec<PinId> = Vec::with_capacity(net_in.nodes.len());

            for node in net_in.nodes {
                let pin_id = pin_lookup
                    .get(&(node.ref_.clone(), node.pin_num.clone()))
                    .copied()
                    .unwrap_or_else(|| {
                        panic!(
                            "net '{}' 引用了未知的 pin '{}.{}'",
                            net_in.name, node.ref_, node.pin_num
                        )
                    });
                // 在 Pass 2 里 pin.pinfunction 暂为 None, 这里用 node 的 pinfunction 回填
                if pins[pin_id.0].pinfunction.is_none() {
                    pins[pin_id.0].pinfunction = node.pinfunction.clone();
                }
                pins[pin_id.0].net = Some(net_id);
                net_pins.push(pin_id);
            }

            nets.push(Net {
                id: net_id,
                name: net_in.name,
                pins: net_pins,
            });
        }

        Circuit {
            components,
            pins,
            nets,
            footprints: footprints.to_vec(),
        }
    }
}

/// 自动标记**可桥接**元件: 2 pin 元件, 一腿在 power net, 另一腿在 signal net。
///
/// 规则: 2 pin + (属于 power net) XOR (另一 pin 属于 power net) = true。
/// - `power_net_names`: 被认为是 power net 的名字列表, 比如 `&["GND", "+12V", "VCC", "5V"]`。
///   这里**用名字**匹配 (而不是 `NetId`), 因为 binding 配置 (`PowerRails.positive_names`)
///   也是按名字存。
///
/// 设置 `Component.bridgeable = true` 后, 调用方可以决定是手动用
/// `Placement::Bridged` 摆它, 还是后续给 SA 加 Toggle 扰动去探索。
pub fn auto_mark_bridgeable(circuit: &mut Circuit, power_net_names: &[&str]) {
    for comp in &mut circuit.components {
        // 仅 2 pin 元件考虑 (电阻 / LED / 电容 / 二极管等)
        if comp.pins.len() != 2 {
            continue;
        }
        let mut nets = comp.pins.iter().filter_map(|&pid| circuit.pins[pid.0].net);
        let Some(n1) = nets.next() else { continue };
        let Some(n2) = nets.next() else { continue };
        let name1 = circuit.nets[n1.0].name();
        let name2 = circuit.nets[n2.0].name();
        let n1_is_power = power_net_names.contains(&name1);
        let n2_is_power = power_net_names.contains(&name2);
        // XOR: 正好一个在 power net
        if n1_is_power != n2_is_power {
            comp.bridgeable = true;
        }
    }
}

/// "LED_THT:LED_D5.0mm" → "LED_D5.0mm"
fn strip_library_prefix(footprint_ref: &str) -> String {
    footprint_ref
        .rsplit(':')
        .next()
        .unwrap_or(footprint_ref)
        .to_string()
}

// ── 从 Sexp 树提取 NetlistInput ───────────────────────────

pub fn parse_netlist(text: &str) -> Result<NetlistInput, ParseError> {
    let sexp = parse(text)?;
    let top = match sexp {
        Sexp::List(items) => items,
        _ => {
            return Err(ParseError {
                message: "expected list at top of .net".into(),
            });
        }
    };

    let mut components = Vec::new();
    let mut nets = Vec::new();

    for item in &top {
        if let Sexp::List(items) = item {
            if let Some(Sexp::Atom(head)) = items.first() {
                match head.as_str() {
                    "components" => {
                        for sub in &items[1..] {
                            if let Some(comp) = extract_comp(sub) {
                                components.push(comp);
                            }
                        }
                    }
                    "nets" => {
                        for sub in &items[1..] {
                            if let Some(net) = extract_net(sub) {
                                nets.push(net);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(NetlistInput { components, nets })
}

fn extract_comp(sexp: &Sexp) -> Option<NetlistComp> {
    let items = match sexp {
        Sexp::List(items) => items,
        _ => return None,
    };
    if !matches!(items.first(), Some(Sexp::Atom(s)) if s == "comp") {
        return None;
    }

    let mut ref_ = None;
    let mut libsource_part = None;
    let mut value = None;
    let mut footprint_ref = None;
    let mut pin_nums = Vec::new();

    for item in &items[1..] {
        if let Sexp::List(sub) = item {
            if let Some(Sexp::Atom(head)) = sub.first() {
                match head.as_str() {
                    "ref" => ref_ = atom_value(&sub[1..]),
                    "footprint" => footprint_ref = atom_value(&sub[1..]),
                    "value" => value = atom_value(&sub[1..]),
                    "libsource" => libsource_part = extract_libsource_part(item),
                    "units" => pin_nums = extract_pin_nums(item),
                    _ => {}
                }
            }
        }
    }

    Some(NetlistComp {
        ref_: ref_?,
        libsource_part: libsource_part?,
        value,
        footprint_ref: footprint_ref?,
        pin_nums,
    })
}

fn extract_libsource_part(sexp: &Sexp) -> Option<String> {
    let items = match sexp {
        Sexp::List(items) => items,
        _ => return None,
    };
    for sub in items {
        if let Sexp::List(sub_items) = sub {
            if matches!(sub_items.first(), Some(Sexp::Atom(s)) if s == "part") {
                return atom_value(&sub_items[1..]);
            }
        }
    }
    None
}

/// 递归找所有 (pin (num "X")) 里的 num
fn extract_pin_nums(sexp: &Sexp) -> Vec<String> {
    let mut out = Vec::new();
    walk_pins(sexp, &mut out);
    out
}

fn walk_pins(sexp: &Sexp, out: &mut Vec<String>) {
    if let Sexp::List(items) = sexp {
        if matches!(items.first(), Some(Sexp::Atom(s)) if s == "pin") {
            // 这个 list 就是一个 (pin ...) form
            for sub in &items[1..] {
                if let Sexp::List(sub_items) = sub {
                    if matches!(sub_items.first(), Some(Sexp::Atom(s)) if s == "num") {
                        if let Some(n) = atom_value(&sub_items[1..]) {
                            out.push(n);
                        }
                    }
                }
            }
            return;
        }
        // 不是 (pin ...) form, 继续往子节点里找
        for sub in items {
            walk_pins(sub, out);
        }
    }
}

fn extract_net(sexp: &Sexp) -> Option<NetlistNet> {
    let items = match sexp {
        Sexp::List(items) => items,
        _ => return None,
    };
    if !matches!(items.first(), Some(Sexp::Atom(s)) if s == "net") {
        return None;
    }

    let mut name = None;
    let mut nodes = Vec::new();

    for item in &items[1..] {
        if let Sexp::List(sub) = item {
            if let Some(Sexp::Atom(head)) = sub.first() {
                match head.as_str() {
                    "name" => name = atom_value(&sub[1..]),
                    "node" => {
                        if let Some(node) = extract_node(item) {
                            nodes.push(node);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Some(NetlistNet { name: name?, nodes })
}

fn extract_node(sexp: &Sexp) -> Option<NetlistNode> {
    let items = match sexp {
        Sexp::List(items) => items,
        _ => return None,
    };
    let mut ref_ = None;
    let mut pin_num = None;
    let mut pinfunction = None;
    for item in items {
        if let Sexp::List(sub) = item {
            if let Some(Sexp::Atom(head)) = sub.first() {
                match head.as_str() {
                    "ref" => ref_ = atom_value(&sub[1..]),
                    "pin" => pin_num = atom_value(&sub[1..]),
                    "pinfunction" => pinfunction = atom_value(&sub[1..]),
                    _ => {}
                }
            }
        }
    }
    Some(NetlistNode {
        ref_: ref_?,
        pin_num: pin_num?,
        pinfunction,
    })
}

fn atom_value(items: &[Sexp]) -> Option<String> {
    if let Some(Sexp::Atom(s)) = items.first() {
        Some(s.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{ComponentId, Footprint, FootprintId, PhysicalPin, Pin, PinId};

    /// 2 pin 元件, 一腿 power 一腿 signal → 标记 bridgeable
    #[test]
    fn auto_marks_2pin_one_power_one_signal() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: crate::circuit::Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: crate::circuit::Position { x: 5, y: 0 },
                },
            ],
        };
        let r1 = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pin1 = Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(0)), // GND
        };
        let pin2 = Pin {
            id: PinId(1),
            component: ComponentId(0),
            num: "2".into(),
            pinfunction: None,
            net: Some(NetId(1)), // signal
        };
        let gnd = Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0)],
        };
        let sig = Net {
            id: NetId(1),
            name: "n1".into(),
            pins: vec![PinId(1)],
        };
        let mut circuit = Circuit {
            components: vec![r1],
            pins: vec![pin1, pin2],
            nets: vec![gnd, sig],
            footprints: vec![fp],
        };
        auto_mark_bridgeable(&mut circuit, &["GND"]);
        assert!(circuit.components[0].bridgeable, "R1 应该被标记 bridgeable");
    }

    /// 2 pin 元件, 两腿都 power (例如 0 欧跨接) → 不标记
    #[test]
    fn auto_marks_2pin_both_power_skipped() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: crate::circuit::Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: crate::circuit::Position { x: 5, y: 0 },
                },
            ],
        };
        let r1 = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pin1 = Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let pin2 = Pin {
            id: PinId(1),
            component: ComponentId(0),
            num: "2".into(),
            pinfunction: None,
            net: Some(NetId(0)), // 同 net
        };
        let gnd = Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0), PinId(1)],
        };
        let mut circuit = Circuit {
            components: vec![r1],
            pins: vec![pin1, pin2],
            nets: vec![gnd],
            footprints: vec![fp],
        };
        auto_mark_bridgeable(&mut circuit, &["GND"]);
        assert!(
            !circuit.components[0].bridgeable,
            "两腿同 power net 不该被标 bridgeable"
        );
    }

    /// 3 pin 元件 (三极管) → 不标, 不管 nets 是啥
    #[test]
    fn auto_marks_skips_3pin_components() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "TO92".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: crate::circuit::Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: crate::circuit::Position { x: 1, y: 0 },
                },
                PhysicalPin {
                    name: "3".into(),
                    offset: crate::circuit::Position { x: 2, y: 0 },
                },
            ],
        };
        let q1 = Component {
            id: ComponentId(0),
            ref_: "Q1".into(),
            kind: "NPN".into(),
            value: None,
            pins: vec![PinId(0), PinId(1), PinId(2)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pin0 = Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let pin1 = Pin {
            id: PinId(1),
            component: ComponentId(0),
            num: "2".into(),
            pinfunction: None,
            net: Some(NetId(1)),
        };
        let pin2 = Pin {
            id: PinId(2),
            component: ComponentId(0),
            num: "3".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let gnd = Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0), PinId(2)],
        };
        let sig = Net {
            id: NetId(1),
            name: "n1".into(),
            pins: vec![PinId(1)],
        };
        let mut circuit = Circuit {
            components: vec![q1],
            pins: vec![pin0, pin1, pin2],
            nets: vec![gnd, sig],
            footprints: vec![fp],
        };
        auto_mark_bridgeable(&mut circuit, &["GND"]);
        assert!(
            !circuit.components[0].bridgeable,
            "3 pin 元件不该被标 bridgeable (规则只覆盖 2 pin)"
        );
    }

    /// 多个元件, 混合场景: 只有 2 pin + 一腿 power 一腿 signal 的才标
    #[test]
    fn auto_marks_only_qualifying_components() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: crate::circuit::Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: crate::circuit::Position { x: 5, y: 0 },
                },
            ],
        };
        // R1: 2 pin, GND + signal → 标
        let r1 = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        // R2: 2 pin, signal + signal → 不标
        let r2 = Component {
            id: ComponentId(1),
            ref_: "R2".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(2), PinId(3)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        // Q1: 3 pin → 不标
        let q1 = Component {
            id: ComponentId(2),
            ref_: "Q1".into(),
            kind: "NPN".into(),
            value: None,
            pins: vec![PinId(4), PinId(5), PinId(6)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pin0 = Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let pin1 = Pin {
            id: PinId(1),
            component: ComponentId(0),
            num: "2".into(),
            pinfunction: None,
            net: Some(NetId(1)),
        };
        let pin2 = Pin {
            id: PinId(2),
            component: ComponentId(1),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(1)),
        };
        let pin3 = Pin {
            id: PinId(3),
            component: ComponentId(1),
            num: "2".into(),
            pinfunction: None,
            net: Some(NetId(2)),
        };
        let pin4 = Pin {
            id: PinId(4),
            component: ComponentId(2),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let pin5 = Pin {
            id: PinId(5),
            component: ComponentId(2),
            num: "2".into(),
            pinfunction: None,
            net: Some(NetId(1)),
        };
        let pin6 = Pin {
            id: PinId(6),
            component: ComponentId(2),
            num: "3".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let gnd = Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0), PinId(4), PinId(6)],
        };
        let sig1 = Net {
            id: NetId(1),
            name: "n1".into(),
            pins: vec![PinId(1), PinId(2), PinId(5)],
        };
        let sig2 = Net {
            id: NetId(2),
            name: "n2".into(),
            pins: vec![PinId(3)],
        };
        let mut circuit = Circuit {
            components: vec![r1, r2, q1],
            pins: vec![pin0, pin1, pin2, pin3, pin4, pin5, pin6],
            nets: vec![gnd, sig1, sig2],
            footprints: vec![fp],
        };
        auto_mark_bridgeable(&mut circuit, &["GND"]);
        assert!(circuit.components[0].bridgeable, "R1 (GND+sig) 应当标");
        assert!(!circuit.components[1].bridgeable, "R2 (sig+sig) 不该标");
        assert!(!circuit.components[2].bridgeable, "Q1 (3 pin) 不该标");
    }
}
