//! KiCad `.kicad_pcb` 文件解析器。
//!
//! 直接把 `.kicad_pcb` 文件一步到位构造 [`Circuit`]。

use std::collections::BTreeMap;

use super::sexp::{ParseError, Sexp, parse};
use crate::circuit::{
    Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin, PinId,
    Position,
};

/// 面包板一个孔的间距 (mm)
const HOLE_SPACING_MM: f64 = 2.54;

/// 把 mm 坐标换算成"孔数", 四舍五入到最近整数孔。
///
/// 容许小偏差 (例如 2.50mm 电容间距 → 1 孔 = 2.54mm),
/// 实际插面包板时引脚弹性足够吸收这个误差。
fn mm_to_holes(mm: f64) -> i32 {
    let holes = mm / HOLE_SPACING_MM;
    holes.round() as i32
}

/// 从单个 pad 提取出的数据
struct PadInfo {
    num: String,
    offset: Position,
    net_name: Option<String>,
    pinfunction: Option<String>,
}

/// 解析 `.kicad_pcb` 文本, 直接构造 [`Circuit`]。
///
/// 解析过程:
/// 1. 遍历所有 `(footprint "Lib:Name" ...)` 顶层块
/// 2. 从每个块提取 Reference / Value / pads 几何 / net 连接 / pinfunction
/// 3. 按 net 名把 pads 分组, 构造 Net 列表
/// 4. unconnected net 保留
pub fn parse_pcb(text: &str) -> Result<Circuit, ParseError> {
    let sexp = parse(text)?;
    let top = match &sexp {
        Sexp::List(items) => items,
        _ => {
            return Err(ParseError {
                message: "expected list at top of .kicad_pcb".into(),
            });
        }
    };

    if top.is_empty() || !matches!(&top[0], Sexp::Atom(s) if s == "kicad_pcb") {
        return Err(ParseError {
            message: "expected (kicad_pcb ...) at top".into(),
        });
    }

    let mut footprints: Vec<Footprint> = Vec::new();
    let mut components: Vec<Component> = Vec::new();
    let mut pins: Vec<Pin> = Vec::new();
    // net_name → Vec<PinId>
    let mut net_pins: BTreeMap<String, Vec<PinId>> = BTreeMap::new();
    // footprint name → FootprintId (去重: 同名封装共享同一个 FootprintId)
    let mut fp_by_name: BTreeMap<String, FootprintId> = BTreeMap::new();

    for item in &top[1..] {
        let fp_items = match item {
            Sexp::List(items) if matches!(items.first(), Some(Sexp::Atom(s)) if s == "footprint") => {
                items
            }
            _ => continue,
        };
        if fp_items.len() < 2 {
            continue;
        }

        // 提取 footprint 库名和封装名
        let fp_ref = match &fp_items[1] {
            Sexp::Atom(s) => s.as_str(),
            _ => continue,
        };
        let (lib, name) = split_footprint_ref(fp_ref);

        // 提取属性
        let ref_ = find_property(fp_items, "Reference")
            .unwrap_or("?")
            .to_string();
        let value = find_property(fp_items, "Value").map(|s| s.to_string());

        // Kind = 库名 (如 "Resistor_THT", "Package_TO_SOT_THT", "Diode_THT")
        let kind = lib.to_string();

        // 提取所有 thru_hole 焊盘; 遇到 SMD 直接 panic
        let pads = extract_pads(fp_items, fp_ref, &ref_);

        if pads.is_empty() {
            continue; // 没有焊盘的元件 (机械层等), 跳过
        }

        // 构造 Footprint (同名封装去重, 共享同一个 FootprintId)
        let fid = *fp_by_name.entry(name.to_string()).or_insert_with(|| {
            let id = FootprintId(footprints.len());
            let physical_pins: Vec<PhysicalPin> = pads
                .iter()
                .map(|p| PhysicalPin {
                    name: p.num.clone(),
                    offset: p.offset,
                })
                .collect();
            footprints.push(Footprint {
                id,
                name: name.to_string(),
                pins: physical_pins,
            });
            id
        });

        // 构造 Component 和 Pin
        let cid = ComponentId(components.len());
        let mut comp_pin_ids: Vec<PinId> = Vec::new();

        for (pad_idx, pad) in pads.iter().enumerate() {
            let pid = PinId(pins.len());
            pins.push(Pin {
                id: pid,
                component: cid,
                num: pad.num.clone(),
                pinfunction: pad.pinfunction.clone(),
                net: None, // 后面建 Net 时回填
                physical_pin_index: pad_idx,
            });
            comp_pin_ids.push(pid);

            if let Some(ref net_name) = pad.net_name {
                net_pins.entry(net_name.clone()).or_default().push(pid);
            }
        }

        components.push(Component {
            id: cid,
            ref_,
            kind,
            value,
            pins: comp_pin_ids,
            footprint: Some(fid),
            bridgeable: false, // 由 auto_mark_bridgeable 后处理
        });
    }

    // 构造 Net 列表, 并回填 Pin.net
    let mut nets: Vec<Net> = Vec::new();
    for (name, net_pin_ids) in net_pins {
        let nid = NetId(nets.len());
        for &pid in &net_pin_ids {
            pins[pid.0].net = Some(nid);
        }
        nets.push(Net {
            id: nid,
            name,
            pins: net_pin_ids,
        });
    }

    Ok(Circuit {
        components,
        pins,
        nets,
        footprints,
    })
}

/// 把 "LIB:NAME" 形式的 footprint ref 拆成 `(LIB, NAME)`。
/// 例如 `"LED_THT:LED_D5.0mm" → ("LED_THT", "LED_D5.0mm")`。
pub fn split_footprint_ref(footprint_ref: &str) -> (&str, &str) {
    match footprint_ref.rsplit_once(':') {
        Some((l, n)) => (l, n),
        None => (footprint_ref, footprint_ref),
    }
}

/// 自动标记**可桥接**元件: 2 pin 元件, 一腿在 power net, 另一腿在 signal net。
///
/// 规则: 2 pin + (一 pin 属于 power net) XOR (另一 pin 属于 power net) = true。
pub fn auto_mark_bridgeable(circuit: &mut Circuit, power_net_names: &[&str]) {
    for comp in &mut circuit.components {
        // eligibility 是当前 power binding 的派生值；每次调用都必须从 false 重算，
        // 不能让上一次 prepare 的 true 残留。
        comp.bridgeable = false;
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
        comp.bridgeable = n1_is_power != n2_is_power;
    }
}

// ── helper ────────────────────────────────────────────────────

/// 在一个 sexp 列表中查找 `(property KEY VALUE ...)`, 返回 VALUE 字符串。
fn find_property<'a>(items: &'a [Sexp], key: &str) -> Option<&'a str> {
    for item in items {
        if let Sexp::List(sub) = item
            && sub.len() >= 3
            && matches!(&sub[0], Sexp::Atom(s) if s == "property")
            && matches!(&sub[1], Sexp::Atom(s) if s == key)
            && let Sexp::Atom(val) = &sub[2]
        {
            return Some(val.as_str());
        }
    }
    None
}

/// 在 footprint block 里提取所有 thru_hole 焊盘。
/// 遇到 SMD 焊盘会 panic。
fn extract_pads(items: &[Sexp], fp_ref: &str, ref_: &str) -> Vec<PadInfo> {
    let mut pads = Vec::new();
    for item in items {
        let sub = match item {
            Sexp::List(sub) if matches!(sub.first(), Some(Sexp::Atom(s)) if s == "pad") => sub,
            _ => continue,
        };
        if sub.len() < 3 {
            continue;
        }

        let num = match &sub[1] {
            Sexp::Atom(s) => s.clone(),
            _ => continue,
        };

        let pad_type = match &sub[2] {
            Sexp::Atom(s) => s.as_str(),
            _ => continue,
        };

        if pad_type == "smd" {
            panic!(
                "footprint '{fp_ref}' (ref {ref_}) 包含 SMD 焊盘 (pad {num}), \
                 面包板只能用直插 (through_hole) 元件"
            );
        }
        if pad_type != "thru_hole" {
            // np_thru_hole / connect — 暂时跳过, 面包板上不常见
            continue;
        }

        // 找 (at X Y)
        let offset = match find_at(sub) {
            Some((x, y)) => Position {
                x: mm_to_holes(x),
                y: mm_to_holes(y),
            },
            None => continue, // 没有位置的焊盘, 跳过
        };

        let net_name = find_atom_value(sub, "net");
        let pinfunction = find_atom_value(sub, "pinfunction");

        pads.push(PadInfo {
            num,
            offset,
            net_name,
            pinfunction,
        });
    }
    pads
}

/// 在一个列表里找 `(at X Y)`, 返回 (x, y)。
fn find_at(items: &[Sexp]) -> Option<(f64, f64)> {
    for item in items {
        if let Sexp::List(sub) = item
            && sub.len() >= 3
            && matches!(sub.first(), Some(Sexp::Atom(s)) if s == "at")
        {
            let x = parse_f64(&sub[1])?;
            let y = parse_f64(&sub[2])?;
            return Some((x, y));
        }
    }
    None
}

/// 在一个列表里找 `(KEY "VALUE")`, 返回 VALUE。
fn find_atom_value(items: &[Sexp], key: &str) -> Option<String> {
    for item in items {
        if let Sexp::List(sub) = item
            && sub.len() >= 2
            && matches!(sub.first(), Some(Sexp::Atom(s)) if s == key)
            && let Sexp::Atom(val) = &sub[1]
        {
            return Some(val.clone());
        }
    }
    None
}

fn parse_f64(sexp: &Sexp) -> Option<f64> {
    if let Sexp::Atom(s) = sexp {
        s.parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_bridge_h_bridge_pcb() {
        let text = std::fs::read_to_string("examples/h-bridge/h-bridge.kicad_pcb").unwrap();
        let mut circuit = parse_pcb(&text).unwrap();

        assert_eq!(circuit.components.len(), 18);
        assert_eq!(circuit.nets.len(), 16); // 12 真实 + 4 unconnected

        auto_mark_bridgeable(&mut circuit, &["GND", "+12V", "VCC", "5V", "3V3"]);
        let bridgeable: Vec<&str> = circuit
            .components
            .iter()
            .filter(|c| c.bridgeable)
            .map(|c| c.ref_())
            .collect();
        assert!(bridgeable.contains(&"D1"));
        assert!(bridgeable.contains(&"R2"));
        // Q1 是 3-pin, 不应桥接
        let q1 = circuit
            .components
            .iter()
            .find(|c| c.ref_() == "Q1")
            .unwrap();
        assert!(!q1.bridgeable);
    }

    #[test]
    fn split_ref_basic() {
        assert_eq!(
            split_footprint_ref("LED_THT:LED_D5.0mm"),
            ("LED_THT", "LED_D5.0mm")
        );
    }

    #[test]
    fn split_ref_no_colon() {
        assert_eq!(split_footprint_ref("nocolon"), ("nocolon", "nocolon"));
    }
}
