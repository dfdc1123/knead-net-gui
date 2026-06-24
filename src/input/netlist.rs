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
                    name: pin_num.clone(),
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
                name: comp_in.ref_,
                kind: comp_in.libsource_part,
                pins: comp_pin_ids,
                footprint,
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
    let mut footprint_ref = None;
    let mut pin_nums = Vec::new();

    for item in &items[1..] {
        if let Sexp::List(sub) = item {
            if let Some(Sexp::Atom(head)) = sub.first() {
                match head.as_str() {
                    "ref" => ref_ = atom_value(&sub[1..]),
                    "footprint" => footprint_ref = atom_value(&sub[1..]),
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
    for item in items {
        if let Sexp::List(sub) = item {
            if let Some(Sexp::Atom(head)) = sub.first() {
                match head.as_str() {
                    "ref" => ref_ = atom_value(&sub[1..]),
                    "pin" => pin_num = atom_value(&sub[1..]),
                    _ => {}
                }
            }
        }
    }
    Some(NetlistNode {
        ref_: ref_?,
        pin_num: pin_num?,
    })
}

fn atom_value(items: &[Sexp]) -> Option<String> {
    if let Some(Sexp::Atom(s)) = items.first() {
        Some(s.clone())
    } else {
        None
    }
}
