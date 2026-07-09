//! KiCad `.kicad_pcb` 文件解析器。
//!
//! 直接从 `.kicad_pcb` 的 `(footprint ...)` 块中同时提取
//! 元件信息、封装几何和网络连接, 一步到位构造 [`Circuit`]。
//! 不再需要分开的 `.net` 和 `.kicad_mod` 文件。

use std::collections::HashMap;

use super::footprint::split_footprint_ref;
use super::sexp::{ParseError, Sexp, parse};
use crate::circuit::{
    Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin, PinId,
    Position,
};

/// 面包板一个孔的间距 (mm)
const HOLE_SPACING_MM: f64 = 2.54;

/// 把 mm 坐标换算成"孔数"。能整除就四舍五入到最近整数, 不能整除就 panic。
fn mm_to_holes(mm: f64) -> i32 {
    let holes = mm / HOLE_SPACING_MM;
    let rounded = holes.round();
    if (rounded - holes).abs() > 1e-9 {
        panic!(
            "位置 {mm} mm 不能整除成面包板孔数 ({holes} 孔) — \
             暂时不接受半孔位置, 面包板网格是 {HOLE_SPACING_MM} mm/孔"
        );
    }
    rounded as i32
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
/// 4. unconnected net 保留 (与旧 .net 解析器行为一致)
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
    let mut net_pins: HashMap<String, Vec<PinId>> = HashMap::new();
    // footprint name → FootprintId (去重: 同名封装共享同一个 FootprintId)
    let mut fp_by_name: HashMap<String, FootprintId> = HashMap::new();

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

        for pad in &pads {
            let pid = PinId(pins.len());
            pins.push(Pin {
                id: pid,
                component: cid,
                num: pad.num.clone(),
                pinfunction: pad.pinfunction.clone(),
                net: None, // 后面建 Net 时回填
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

// ── helper ────────────────────────────────────────────────────

/// 在一个 sexp 列表中查找 `(property KEY VALUE ...)`, 返回 VALUE 字符串。
fn find_property<'a>(items: &'a [Sexp], key: &str) -> Option<&'a str> {
    for item in items {
        if let Sexp::List(sub) = item
            && sub.len() >= 3
            && matches!(&sub[0], Sexp::Atom(s) if s == "property")
            && matches!(&sub[1], Sexp::Atom(s) if s == key)
        {
            if let Sexp::Atom(val) = &sub[2] {
                return Some(val.as_str());
            }
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
        {
            if let Sexp::Atom(val) = &sub[1] {
                return Some(val.clone());
            }
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
    fn parse_h_bridge_pcb() {
        let text = std::fs::read_to_string("examples/inputs/h-bridge.kicad_pcb").unwrap();
        let circuit = parse_pcb(&text).unwrap();

        // 18 个元件: D1-4, Q1-6, R1-8
        assert_eq!(circuit.components.len(), 18);

        // 检查几个已知的 net
        let net_names: Vec<&str> = circuit.nets.iter().map(|n| n.name()).collect();
        assert!(net_names.contains(&"GND"));
        assert!(net_names.contains(&"+12V"));
        assert!(net_names.contains(&"Net-(Q1-C)"));
        // unconnected nets 保留 (跟旧 .net 解析器一致)
        assert!(net_names.iter().any(|n| n.starts_with("unconnected-")));
        assert_eq!(circuit.nets.len(), 16); // 12 真实 + 4 unconnected

        // 检查 R4 (有 unconnected pad)
        let r4 = circuit
            .components
            .iter()
            .find(|c| c.ref_() == "R4")
            .unwrap();
        assert_eq!(r4.kind(), "Resistor_THT");
        assert_eq!(r4.pins.len(), 2);

        // 检查 Q1 的 pinfunction
        let q1 = circuit
            .components
            .iter()
            .find(|c| c.ref_() == "Q1")
            .unwrap();
        let q1_pinfuncs: Vec<Option<&str>> = q1
            .pins
            .iter()
            .map(|&pid| circuit.pins[pid.0].pinfunction())
            .collect();
        assert!(q1_pinfuncs.contains(&Some("C_1")));
        assert!(q1_pinfuncs.contains(&Some("B_2")));
        assert!(q1_pinfuncs.contains(&Some("E_3")));
    }

    /// 对比 .kicad_pcb 解析结果 与 .net + .kicad_mod 解析结果。
    /// 两者应描述同一个电路 (h-bridge, 18 元件), 除 kind 字段外应完全一致。
    #[test]
    fn pcb_matches_netlist_plus_footprints() {
        use crate::input::footprint::parse_many;
        use crate::input::netlist::parse_netlist;

        // -- 旧方式: .net + .kicad_mod --
        let netlist_text = std::fs::read_to_string("examples/inputs/h-bridge.net").unwrap();
        let netlist = parse_netlist(&netlist_text).unwrap();

        let fp_dir = "examples/footprints";
        let mut fp_paths: Vec<String> = std::fs::read_dir(fp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("kicad_mod"))
            .filter_map(|p| p.to_str().map(String::from))
            .collect();
        fp_paths.sort();
        let fp_texts: Vec<String> = fp_paths
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        let footprints = parse_many(fp_texts).unwrap();
        let old = netlist.into_circuit(&footprints);

        // -- 新方式: .kicad_pcb --
        let pcb_text = std::fs::read_to_string("examples/inputs/h-bridge.kicad_pcb").unwrap();
        let new = parse_pcb(&pcb_text).unwrap();

        // 元件数, net 数, pin 数一致
        assert_eq!(old.components.len(), new.components.len());
        assert_eq!(old.nets.len(), new.nets.len());
        assert_eq!(old.pins.len(), new.pins.len());
        // 旧方式加载了 examples/footprints/ 下全部 6 个 .kicad_mod,
        // 但 h-bridge 只用其中 3 个。新方式只产生实际用到的 3 个。
        assert_eq!(old.footprints.len(), 6);
        assert_eq!(new.footprints.len(), 3);

        // 逐个对比 footprint (用到的 3 个封装应完全一致)
        let new_fp_by_name: HashMap<&str, &Footprint> =
            new.footprints.iter().map(|fp| (fp.name(), fp)).collect();
        for fp_old in &old.footprints {
            if let Some(fp_new) = new_fp_by_name.get(fp_old.name()) {
                assert_eq!(fp_old.pins().len(), fp_new.pins().len());
                for (pp_old, pp_new) in fp_old.pins().iter().zip(fp_new.pins().iter()) {
                    assert_eq!(pp_old.name(), pp_new.name());
                    assert_eq!(pp_old.offset(), pp_new.offset());
                }
            }
        }

        // 逐个对比 component (顺序可能不同, 按 ref 名查找)
        let old_by_ref: HashMap<&str, &Component> =
            old.components.iter().map(|c| (c.ref_(), c)).collect();
        for comp_new in &new.components {
            let comp_old = old_by_ref.get(comp_new.ref_()).unwrap();
            assert_eq!(comp_old.pins().len(), comp_new.pins().len());
            assert_eq!(
                comp_old.footprint().is_some(),
                comp_new.footprint().is_some()
            );
        }

        // 逐个对比 pin: num + pinfunction (按 component ref + pin num 查找)
        let old_pin_key: HashMap<(String, String), &Pin> = old
            .pins
            .iter()
            .map(|p| {
                let comp_ref = old.components[p.component().raw()].ref_().to_string();
                ((comp_ref, p.num().to_string()), p)
            })
            .collect();
        for pin_new in &new.pins {
            let comp_ref = new.components[pin_new.component().raw()].ref_().to_string();
            let key = (comp_ref, pin_new.num().to_string());
            let pin_old = old_pin_key.get(&key).unwrap();
            assert_eq!(pin_old.pinfunction(), pin_new.pinfunction());
        }

        // 逐个对比 net: 名字 + 包含的 pin 数 (按名字查找)
        let old_net_by_name: HashMap<&str, &Net> = old.nets.iter().map(|n| (n.name(), n)).collect();
        for net_new in &new.nets {
            let net_old = old_net_by_name.get(net_new.name()).unwrap();
            assert_eq!(net_old.pins().len(), net_new.pins().len());
        }

        // kind 对比: 仅打印差异, 不 assert (已知不同: 旧用 libsource part, 新用 footprint 库名)
        eprintln!("kind 字段对比 (旧 .net vs 新 .kicad_pcb):");
        for comp_new in &new.components {
            let comp_old = old_by_ref.get(comp_new.ref_()).unwrap();
            if comp_old.kind() != comp_new.kind() {
                eprintln!(
                    "  {:4}  {:6} -> {}",
                    comp_new.ref_(),
                    comp_old.kind(),
                    comp_new.kind()
                );
            }
        }
    }
}
