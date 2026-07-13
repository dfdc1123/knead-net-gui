//! KiCad `.kicad_sch` → SVG 渲染
//!
//! 按 `RENDER_SPEC.md` 实现。复用 knead-net 已有的 `lexpr` crate 做 S-Expression 解析。
//!
//! 数据流:
//!   1. lexpr 解析 → Value 树
//!   2. 提取 lib_symbols / junctions / wires / symbol instances
//!   3. 坐标变换 (Rotate → Mirror → Flip Y → Translate)
//!   4. 拼 SVG 字符串

use lexpr::Value;
use std::collections::{HashMap, HashSet};
use std::fs;

use knead_net::input::pcb::parse_pcb;

const SCALE: f64 = 10.0;

// KiCad 默认浅色原理图主题的核心配色。将颜色集中在这里，既避免 SVG
// 各处逐渐出现不一致的魔法值，也方便以后支持可切换主题。
const COLOR_BACKGROUND: &str = "#ffffff";
const COLOR_SYMBOL: &str = "#840000";
const COLOR_SYMBOL_FILL: &str = "#ffffc2";
const COLOR_PIN: &str = "#840000";
const COLOR_WIRE: &str = "#009600";
const COLOR_REFERENCE: &str = "#008484";
const COLOR_VALUE: &str = "#0000c2";

// ─────────────────────────── 数据结构 ───────────────────────────

#[derive(Debug, Clone)]
enum Graphic {
    Polyline {
        pts: Vec<(f64, f64)>,
        stroke: f64,
    },
    Rectangle {
        start: (f64, f64),
        end: (f64, f64),
        stroke: f64,
        fill: Fill,
    },
    Circle {
        center: (f64, f64),
        radius: f64,
        stroke: f64,
        fill: Fill,
    },
    Arc {
        start: (f64, f64),
        mid: (f64, f64),
        end: (f64, f64),
        stroke: f64,
        fill: Fill,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    None,
    Background,
}

#[derive(Debug, Clone)]
struct Pin {
    at: (f64, f64, f64), // x, y, direction(度)
    length: f64,
    #[expect(
        dead_code,
        reason = "Step 4 will display pin names in the rendered result"
    )]
    name: String,
    number: String,
}

#[derive(Debug, Default, Clone)]
struct SubSymbol {
    graphics: Vec<Graphic>,
    pins: Vec<Pin>,
}

/// lib_id → (unit → (body_style → SubSymbol))
type LibMap = HashMap<String, HashMap<u32, HashMap<u32, SubSymbol>>>;

#[derive(Debug, Clone)]
struct Property {
    key: String,
    value: String,
    at: (f64, f64, f64),
    hide: bool,
}

#[derive(Debug, Clone)]
struct Inst {
    lib_id: String,
    at: (f64, f64, f64),
    mirror_x: bool,
    mirror_y: bool,
    unit: u32,
    body_style: u32,
    properties: Vec<Property>,
}

// ─────────────────────────── lexpr 辅助 ───────────────────────────

fn list_items(v: &Value) -> impl Iterator<Item = &Value> {
    v.list_iter().into_iter().flatten()
}

fn as_symbol(v: &Value) -> Option<&str> {
    v.as_symbol()
}

fn as_str(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_number().and_then(|n| n.as_f64())
}

fn child<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    v.list_iter()
        .into_iter()
        .flatten()
        .find(|item| item.as_cons().and_then(|c| c.car().as_symbol()) == Some(name))
}

fn children<'a>(v: &'a Value, name: &str) -> Vec<&'a Value> {
    v.list_iter()
        .into_iter()
        .flatten()
        .filter(|item| item.as_cons().and_then(|c| c.car().as_symbol()) == Some(name))
        .collect()
}

/// `(name value)` → 跳过 name 符, 取 value 数字
fn parse_number_child(v: &Value) -> Option<f64> {
    let mut iter = list_items(v);
    iter.next()?; // 跳过 'name' symbol
    as_f64(iter.next()?)
}

/// `(at x y [rot])`
fn parse_at(v: &Value) -> Option<(f64, f64, f64)> {
    let mut iter = list_items(v);
    let _ = iter.next()?; // 'at'
    let x = as_f64(iter.next()?)?;
    let y = as_f64(iter.next()?)?;
    let rot = iter.next().and_then(as_f64).unwrap_or(0.0);
    Some((x, y, rot))
}

fn parse_xy(v: &Value) -> Option<(f64, f64)> {
    let mut iter = list_items(v);
    let _ = iter.next()?; // 'xy'
    let x = as_f64(iter.next()?)?;
    let y = as_f64(iter.next()?)?;
    Some((x, y))
}

fn parse_stroke(v: &Value) -> f64 {
    child(v, "stroke")
        .and_then(|s| child(s, "width"))
        .and_then(parse_number_child) // (width 0.254) → 0.254
        .unwrap_or(0.0)
}

fn parse_fill(v: &Value) -> Fill {
    child(v, "fill")
        .and_then(|f| f.list_iter().into_iter().flatten().nth(1))
        .and_then(as_symbol)
        .map(|s| match s {
            "background" => Fill::Background,
            _ => Fill::None,
        })
        .unwrap_or(Fill::None)
}

// ─────────────────────────── 提取 ───────────────────────────

fn extract_lib_symbols(root: &Value) -> Result<LibMap, String> {
    let mut libs = LibMap::new();
    let lib_symbols_node = child(root, "lib_symbols").ok_or("lib_symbols 节点不存在")?;

    for sym_node in children(lib_symbols_node, "symbol") {
        // 顶层 symbol: (symbol "NAME" ...)
        let mut iter = list_items(sym_node);
        let _ = iter.next();
        let name = iter.next().and_then(as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }

        let mut unit_map: HashMap<u32, HashMap<u32, SubSymbol>> = HashMap::new();

        // 子 symbol: (symbol "NAME_UNIT_STYLE" ... graphics/pins ...)
        for sub in children(sym_node, "symbol") {
            let mut siter = list_items(sub);
            let _ = siter.next();
            let sub_name = siter.next().and_then(as_str).unwrap_or_default();
            // 解析 "LM741_0_1" → ("LM741", 0, 1) —— rsplitn 从右边切
            let parts: Vec<&str> = sub_name.rsplitn(3, '_').collect();
            if parts.len() != 3 {
                continue;
            }
            let style: u32 = parts[0].parse().unwrap_or(1);
            let unit: u32 = parts[1].parse().unwrap_or(0);

            // 把 sub 转成可迭代 Value
            let body = sub.as_cons().unwrap().cdr().clone();
            let (graphics, pins) = extract_body(&body);

            unit_map
                .entry(unit)
                .or_default()
                .insert(style, SubSymbol { graphics, pins });
        }

        libs.insert(name, unit_map);
    }

    Ok(libs)
}

fn extract_body(v: &Value) -> (Vec<Graphic>, Vec<Pin>) {
    let mut graphics = Vec::new();
    let mut pins = Vec::new();
    for item in list_items(v) {
        let Some(cons) = item.as_cons() else { continue };
        match cons.car().as_symbol() {
            Some("polyline") => {
                let pts: Vec<(f64, f64)> = children(item, "pts")
                    .iter()
                    .flat_map(|pts_node| children(pts_node, "xy"))
                    .filter_map(parse_xy)
                    .collect();
                graphics.push(Graphic::Polyline {
                    pts,
                    stroke: parse_stroke(item),
                });
            }
            Some("rectangle") => {
                let start = child(item, "start")
                    .and_then(parse_xy)
                    .unwrap_or((0.0, 0.0));
                let end = child(item, "end").and_then(parse_xy).unwrap_or((0.0, 0.0));
                graphics.push(Graphic::Rectangle {
                    start,
                    end,
                    stroke: parse_stroke(item),
                    fill: parse_fill(item),
                });
            }
            Some("circle") => {
                let center = child(item, "center")
                    .and_then(parse_xy)
                    .unwrap_or((0.0, 0.0));
                let radius = child(item, "radius").and_then(as_f64).unwrap_or(0.0);
                graphics.push(Graphic::Circle {
                    center,
                    radius,
                    stroke: parse_stroke(item),
                    fill: parse_fill(item),
                });
            }
            Some("arc") => {
                let start = child(item, "start")
                    .and_then(parse_xy)
                    .unwrap_or((0.0, 0.0));
                let mid = child(item, "mid").and_then(parse_xy).unwrap_or((0.0, 0.0));
                let end = child(item, "end").and_then(parse_xy).unwrap_or((0.0, 0.0));
                graphics.push(Graphic::Arc {
                    start,
                    mid,
                    end,
                    stroke: parse_stroke(item),
                    fill: parse_fill(item),
                });
            }
            Some("pin") => {
                let at = child(item, "at")
                    .and_then(parse_at)
                    .unwrap_or((0.0, 0.0, 0.0));
                let length = child(item, "length")
                    .and_then(parse_number_child)
                    .unwrap_or(0.0);
                let name = child(item, "name")
                    .and_then(|value| value.list_iter().into_iter().flatten().nth(1))
                    .and_then(as_str)
                    .unwrap_or_default();
                let number = child(item, "number")
                    .and_then(|value| value.list_iter().into_iter().flatten().nth(1))
                    .and_then(as_str)
                    .unwrap_or_default();
                pins.push(Pin {
                    at,
                    length,
                    name,
                    number,
                });
            }
            _ => {}
        }
    }
    (graphics, pins)
}

fn extract_wires(root: &Value) -> Vec<Vec<(f64, f64)>> {
    children(root, "wire")
        .iter()
        .map(|w| {
            children(w, "pts")
                .iter()
                .flat_map(|pts| children(pts, "xy"))
                .filter_map(parse_xy)
                .collect()
        })
        .collect()
}

fn extract_junctions(root: &Value) -> Vec<(f64, f64)> {
    children(root, "junction")
        .iter()
        .filter_map(|j| child(j, "at").and_then(parse_xy))
        .collect()
}

fn extract_instances(root: &Value) -> Vec<Inst> {
    children(root, "symbol")
        .iter()
        .filter_map(|sym| {
            let lib_id = child(sym, "lib_id")
                .and_then(|v| v.list_iter().into_iter().flatten().nth(1))
                .and_then(as_str)?;
            let at = child(sym, "at")
                .and_then(parse_at)
                .unwrap_or((0.0, 0.0, 0.0));
            let mirror_x = sym.list_iter().into_iter().flatten().any(|n| {
                n.as_cons().and_then(|c| c.car().as_symbol()) == Some("mirror")
                    && n.list_iter()
                        .into_iter()
                        .flatten()
                        .nth(1)
                        .and_then(as_symbol)
                        == Some("x")
            });
            let mirror_y = sym.list_iter().into_iter().flatten().any(|n| {
                n.as_cons().and_then(|c| c.car().as_symbol()) == Some("mirror")
                    && n.list_iter()
                        .into_iter()
                        .flatten()
                        .nth(1)
                        .and_then(as_symbol)
                        == Some("y")
            });
            let unit = child(sym, "unit")
                .and_then(|v| v.list_iter().into_iter().flatten().nth(1))
                .and_then(as_f64)
                .map(|n| n as u32)
                .unwrap_or(0); // 默认 0: 单单元符号的本体在 unit=0 (如 R_0_1)
            let body_style = child(sym, "body_style")
                .and_then(|v| v.list_iter().into_iter().flatten().nth(1))
                .and_then(as_f64)
                .map(|n| n as u32)
                .unwrap_or(1);

            let properties = children(sym, "property")
                .iter()
                .map(|p| {
                    let mut iter = p.list_iter().into_iter().flatten();
                    iter.next(); // 'property'
                    let key = iter.next().and_then(as_str).unwrap_or_default();
                    let value = iter.next().and_then(as_str).unwrap_or_default();
                    let at = child(p, "at").and_then(parse_at).unwrap_or((0.0, 0.0, 0.0));
                    let hide = child(p, "hide").is_some();
                    Property {
                        key,
                        value,
                        at,
                        hide,
                    }
                })
                .collect();

            Some(Inst {
                lib_id,
                at,
                mirror_x,
                mirror_y,
                unit,
                body_style,
                properties,
            })
        })
        .collect()
}

// ─────────────────────────── 坐标变换 ───────────────────────────

fn transform(lx: f64, ly: f64, at: (f64, f64, f64), mx: bool, my: bool) -> (f64, f64) {
    let (ax, ay, rot) = at;
    let rad = rot * std::f64::consts::PI / 180.0;
    let (s, c) = rad.sin_cos();
    let rx = lx * c - ly * s;
    let ry = lx * s + ly * c;
    let ry = if mx { -ry } else { ry };
    let rx = if my { -rx } else { rx };
    (rx + ax, ay - ry)
}

fn to_svg(gx: f64, gy: f64, ox: f64, oy: f64) -> (f64, f64) {
    ((gx - ox) * SCALE, (gy - oy) * SCALE)
}

// ─────────────────────────── BBox ───────────────────────────

fn polyline_bbox(
    pts: &[(f64, f64)],
    at: (f64, f64, f64),
    mx: bool,
    my: bool,
) -> (f64, f64, f64, f64) {
    let mut it = pts.iter().map(|&(x, y)| transform(x, y, at, mx, my));
    match it.next() {
        Some(first) => it.fold(
            (first.0, first.1, first.0, first.1),
            |(minx, miny, maxx, maxy), (x, y)| (minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)),
        ),
        None => (0.0, 0.0, 0.0, 0.0),
    }
}

fn bbox_of_graphic(g: &Graphic, at: (f64, f64, f64), mx: bool, my: bool) -> (f64, f64, f64, f64) {
    match g {
        Graphic::Polyline { pts, .. } => polyline_bbox(pts, at, mx, my),
        Graphic::Rectangle { start, end, .. } => {
            // 旋转/镜像后矩形变一般四边形
            let (sx, sy) = transform(start.0, start.1, at, mx, my);
            let (ex, ey) = transform(end.0, end.1, at, mx, my);
            let (ux, uy) = transform(start.0, end.1, at, mx, my);
            let (vx, vy) = transform(end.0, start.1, at, mx, my);
            let xs = [sx, ex, ux, vx];
            let ys = [sy, ey, uy, vy];
            (
                xs.iter().cloned().fold(f64::INFINITY, f64::min),
                ys.iter().cloned().fold(f64::INFINITY, f64::min),
                xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            )
        }
        Graphic::Circle { center, radius, .. } => {
            let (cx, cy) = transform(center.0, center.1, at, mx, my);
            (cx - radius, cy - radius, cx + radius, cy + radius)
        }
        Graphic::Arc {
            start, mid, end, ..
        } => {
            let s = transform(start.0, start.1, at, mx, my);
            let m = transform(mid.0, mid.1, at, mx, my);
            let e = transform(end.0, end.1, at, mx, my);
            let (mut minx, mut miny) = (s.0.min(e.0).min(m.0), s.1.min(e.1).min(m.1));
            let (mut maxx, mut maxy) = (s.0.max(e.0).max(m.0), s.1.max(e.1).max(m.1));
            // 保守估计：把圆心也包进来
            if let Some(((cx, cy), _)) = arc_center(s, m, e) {
                minx = minx.min(cx);
                miny = miny.min(cy);
                maxx = maxx.max(cx);
                maxy = maxy.max(cy);
            }
            (minx, miny, maxx, maxy)
        }
    }
}

fn compute_bbox(
    wires: &[Vec<(f64, f64)>],
    junctions: &[(f64, f64)],
    insts: &[Inst],
    libs: &LibMap,
) -> Result<(f64, f64, f64, f64), String> {
    let mut bbs: Vec<(f64, f64, f64, f64)> = Vec::new();

    for w in wires {
        if let Some(&(x, y)) = w.first() {
            bbs.push((x, y, x, y));
            for &(x, y) in w {
                if let Some(last) = bbs.last_mut() {
                    last.0 = last.0.min(x);
                    last.1 = last.1.min(y);
                    last.2 = last.2.max(x);
                    last.3 = last.3.max(y);
                }
            }
        }
    }
    for &(x, y) in junctions {
        bbs.push((x, y, x, y));
    }
    for inst in insts {
        if let Some(unit) = libs.get(&inst.lib_id) {
            // 复用 render 时的 lookup 逻辑: 优先 (unit, style), graphics 为空时回退 unit=0
            let sub_opt = unit
                .get(&inst.unit)
                .and_then(|styles| styles.get(&inst.body_style))
                .filter(|ss| !ss.graphics.is_empty())
                .or_else(|| unit.get(&0).and_then(|styles| styles.get(&inst.body_style)));

            if let Some(sub) = sub_opt {
                for g in &sub.graphics {
                    bbs.push(bbox_of_graphic(g, inst.at, inst.mirror_x, inst.mirror_y));
                }
            }
            // pins: 取 unit 的所有 style
            if let Some(styles) = unit.get(&inst.unit).or_else(|| unit.get(&0)) {
                for sub in styles.values() {
                    for p in &sub.pins {
                        let (gx, gy) =
                            transform(p.at.0, p.at.1, inst.at, inst.mirror_x, inst.mirror_y);
                        bbs.push((gx, gy, gx, gy));
                    }
                }
            }
        }
    }

    if bbs.is_empty() {
        return Ok((0.0, 0.0, 100.0, 100.0));
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = bbs.remove(0);
    for (a, b, c, d) in bbs {
        minx = minx.min(a);
        miny = miny.min(b);
        maxx = maxx.max(c);
        maxy = maxy.max(d);
    }
    Ok((minx, miny, maxx, maxy))
}

// ─────────────────────────── 弧线求圆心 ───────────────────────────

fn arc_center(s: (f64, f64), m: (f64, f64), e: (f64, f64)) -> Option<((f64, f64), f64)> {
    let a1 = 2.0 * (m.0 - s.0);
    let b1 = 2.0 * (m.1 - s.1);
    let c1 = m.0 * m.0 + m.1 * m.1 - s.0 * s.0 - s.1 * s.1;
    let a2 = 2.0 * (e.0 - m.0);
    let b2 = 2.0 * (e.1 - m.1);
    let c2 = e.0 * e.0 + e.1 * e.1 - m.0 * m.0 - m.1 * m.1;
    let det = a1 * b2 - a2 * b1;
    if det.abs() < 1e-10 {
        return None;
    }
    let cx = (c1 * b2 - c2 * b1) / det;
    let cy = (a1 * c2 - a2 * c1) / det;
    let r = ((s.0 - cx).powi(2) + (s.1 - cy).powi(2)).sqrt();
    Some(((cx, cy), r))
}

// ─────────────────────────── SVG 生成 ───────────────────────────

fn stroke_w(width: f64) -> f64 {
    let w = if width <= 0.0 { 0.254 } else { width };
    (w * SCALE).max(1.0)
}

fn fmt_pts(pts: &[(f64, f64)]) -> String {
    pts.iter()
        .map(|(x, y)| format!("{:.2},{:.2}", x, y))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Escape untrusted KiCad text before embedding it in an SVG text node.
///
/// KiCad files can be supplied by the user, and the generated SVG is inserted
/// into the webview as HTML. Escaping all XML-significant characters here keeps
/// property values as text instead of allowing them to create SVG/HTML nodes.
fn escape_xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn property_value<'a>(inst: &'a Inst, key: &str) -> Option<&'a str> {
    inst.properties
        .iter()
        .find(|property| property.key == key)
        .map(|property| property.value.as_str())
}

fn instance_pins<'a>(inst: &Inst, libs: &'a LibMap) -> Vec<&'a Pin> {
    let Some(units) = libs.get(&inst.lib_id) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    units
        .get(&inst.unit)
        .or_else(|| units.get(&0))
        .map(|styles| {
            styles
                .values()
                .flat_map(|sub| sub.pins.iter())
                .filter(|pin| {
                    seen.insert((pin.number.clone(), pin.at.0.to_bits(), pin.at.1.to_bits()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn point_on_segment(point: (f64, f64), start: (f64, f64), end: (f64, f64)) -> bool {
    let cross = (point.0 - start.0) * (end.1 - start.1) - (point.1 - start.1) * (end.0 - start.0);
    if cross.abs() > 1e-5 {
        return false;
    }
    let dot = (point.0 - start.0) * (end.0 - start.0) + (point.1 - start.1) * (end.1 - start.1);
    let length_sq = (end.0 - start.0).powi(2) + (end.1 - start.1).powi(2);
    dot >= -1e-5 && dot <= length_sq + 1e-5
}

fn point_on_wire(point: (f64, f64), wire: &[(f64, f64)]) -> bool {
    wire.windows(2)
        .any(|segment| point_on_segment(point, segment[0], segment[1]))
}

fn wire_net_names(
    wires: &[Vec<(f64, f64)>],
    instances: &[Inst],
    libs: &LibMap,
    pcb_path: Option<&str>,
) -> Vec<Option<String>> {
    let Some(pcb_path) = pcb_path else {
        return vec![None; wires.len()];
    };
    let Ok(text) = fs::read_to_string(pcb_path) else {
        return vec![None; wires.len()];
    };
    let Ok(circuit) = parse_pcb(&text) else {
        return vec![None; wires.len()];
    };

    let mut pin_nets = HashMap::new();
    for component in circuit.components() {
        for pin_id in component.pins() {
            let pin = &circuit.pins()[pin_id.raw()];
            if let Some(net_id) = pin.net() {
                pin_nets.insert(
                    (component.ref_().to_string(), pin.num().to_string()),
                    circuit.nets()[net_id.raw()].name().to_string(),
                );
            }
        }
    }

    // KiCad 会把一条逻辑网络拆成多个 wire 节点。只要两段共享端点（包括
    // T 形连接落在另一段中间），就把它们并入同一个连通分量。
    let mut parents: Vec<usize> = (0..wires.len()).collect();
    fn root(parents: &mut [usize], mut index: usize) -> usize {
        while parents[index] != index {
            parents[index] = parents[parents[index]];
            index = parents[index];
        }
        index
    }
    for left in 0..wires.len() {
        for right in (left + 1)..wires.len() {
            let touches = wires[left]
                .iter()
                .any(|&point| point_on_wire(point, &wires[right]))
                || wires[right]
                    .iter()
                    .any(|&point| point_on_wire(point, &wires[left]));
            if touches {
                let left_root = root(&mut parents, left);
                let right_root = root(&mut parents, right);
                parents[right_root] = left_root;
            }
        }
    }

    let mut component_nets: HashMap<usize, String> = HashMap::new();
    for inst in instances {
        let Some(reference) = property_value(inst, "Reference") else {
            continue;
        };
        for pin in instance_pins(inst, libs) {
            let Some(net_name) = pin_nets.get(&(reference.to_string(), pin.number.clone())) else {
                continue;
            };
            let connection = transform(pin.at.0, pin.at.1, inst.at, inst.mirror_x, inst.mirror_y);
            for (index, wire) in wires.iter().enumerate() {
                if point_on_wire(connection, wire) {
                    let group = root(&mut parents, index);
                    component_nets
                        .entry(group)
                        .or_insert_with(|| net_name.clone());
                }
            }
        }
    }

    (0..wires.len())
        .map(|index| {
            let group = root(&mut parents, index);
            component_nets.get(&group).cloned()
        })
        .collect()
}

fn render_graphic(svg: &mut String, g: &Graphic, inst: &Inst, ox: f64, oy: f64) {
    match g {
        Graphic::Polyline { pts, stroke } => {
            let svg_pts: Vec<(f64, f64)> = pts
                .iter()
                .map(|&(x, y)| {
                    let (gx, gy) = transform(x, y, inst.at, inst.mirror_x, inst.mirror_y);
                    to_svg(gx, gy, ox, oy)
                })
                .collect();
            svg.push_str(&format!(
                r##"<polyline points="{}" fill="none" stroke="{}" stroke-width="{:.2}"/>"##,
                fmt_pts(&svg_pts),
                COLOR_SYMBOL,
                stroke_w(*stroke)
            ));
        }
        Graphic::Rectangle {
            start,
            end,
            stroke,
            fill,
        } => {
            let pts = [
                transform(start.0, start.1, inst.at, inst.mirror_x, inst.mirror_y),
                transform(end.0, start.1, inst.at, inst.mirror_x, inst.mirror_y),
                transform(end.0, end.1, inst.at, inst.mirror_x, inst.mirror_y),
                transform(start.0, end.1, inst.at, inst.mirror_x, inst.mirror_y),
            ];
            let svg_pts: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| to_svg(x, y, ox, oy)).collect();
            let fill_str = if *fill == Fill::Background {
                COLOR_SYMBOL_FILL
            } else {
                "none"
            };
            svg.push_str(&format!(
                r##"<polygon points="{}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                fmt_pts(&svg_pts),
                fill_str,
                COLOR_SYMBOL,
                stroke_w(*stroke)
            ));
        }
        Graphic::Circle {
            center,
            radius,
            stroke,
            fill,
        } => {
            let (cx, cy) = transform(center.0, center.1, inst.at, inst.mirror_x, inst.mirror_y);
            let (sx, sy) = to_svg(cx, cy, ox, oy);
            let fill_str = if *fill == Fill::Background {
                COLOR_SYMBOL_FILL
            } else {
                "none"
            };
            svg.push_str(&format!(
                r##"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                sx,
                sy,
                radius * SCALE,
                fill_str,
                COLOR_SYMBOL,
                stroke_w(*stroke)
            ));
        }
        Graphic::Arc {
            start,
            mid,
            end,
            stroke,
            fill,
        } => {
            let s = transform(start.0, start.1, inst.at, inst.mirror_x, inst.mirror_y);
            let m = transform(mid.0, mid.1, inst.at, inst.mirror_x, inst.mirror_y);
            let e = transform(end.0, end.1, inst.at, inst.mirror_x, inst.mirror_y);
            let Some(((cx, cy), r)) = arc_center(s, m, e) else {
                return;
            };
            let cross_se = (s.0 - cx) * (e.1 - cy) - (s.1 - cy) * (e.0 - cx);
            let cross_sm = (s.0 - cx) * (m.1 - cy) - (s.1 - cy) * (m.0 - cx);
            let large_arc = if cross_se * cross_sm < 0.0 { 1 } else { 0 };
            let sweep = if cross_se >= 0.0 { 1 } else { 0 };
            let (sx, sy) = to_svg(s.0, s.1, ox, oy);
            let (ex, ey) = to_svg(e.0, e.1, ox, oy);
            let fill_str = if *fill == Fill::Background {
                COLOR_SYMBOL_FILL
            } else {
                "none"
            };
            svg.push_str(&format!(
                r##"<path d="M {:.2},{:.2} A {:.2},{:.2} 0 {},{} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                sx, sy, r * SCALE, r * SCALE, large_arc, sweep, ex, ey, fill_str,
                COLOR_SYMBOL, stroke_w(*stroke)
            ));
        }
    }
}

fn render_instance(svg: &mut String, inst: &Inst, libs: &LibMap, ox: f64, oy: f64) {
    let Some(unit) = libs.get(&inst.lib_id) else {
        return;
    };

    // 本体图形:优先 (unit, style), 找到但 graphics 为空则回退 unit=0 (单单元符号的本体所在)
    let body_graphics: Vec<&Graphic> = unit
        .get(&inst.unit)
        .and_then(|styles| styles.get(&inst.body_style))
        .filter(|ss| !ss.graphics.is_empty())
        .or_else(|| unit.get(&0).and_then(|styles| styles.get(&inst.body_style)))
        .map(|ss| ss.graphics.iter().collect())
        .unwrap_or_default();

    // 引脚:取所有 style 并按 (number, at) 去重 (同样回退到 unit=0)
    let mut seen = HashSet::new();
    let pins: Vec<&Pin> = unit
        .get(&inst.unit)
        .or_else(|| unit.get(&0))
        .map(|styles| {
            styles
                .values()
                .flat_map(|ss| ss.pins.iter())
                .filter(|p| seen.insert((p.number.clone(), p.at.0.to_bits(), p.at.1.to_bits())))
                .collect()
        })
        .unwrap_or_default();

    for g in &body_graphics {
        render_graphic(svg, g, inst, ox, oy);
    }

    for p in &pins {
        let pin_at = p.at;
        let rad = pin_at.2 * std::f64::consts::PI / 180.0;
        let tip = (
            pin_at.0 + p.length * rad.cos(),
            pin_at.1 + p.length * rad.sin(),
        );
        let (gx1, gy1) = transform(pin_at.0, pin_at.1, inst.at, inst.mirror_x, inst.mirror_y);
        let (gx2, gy2) = transform(tip.0, tip.1, inst.at, inst.mirror_x, inst.mirror_y);
        let (sx1, sy1) = to_svg(gx1, gy1, ox, oy);
        let (sx2, sy2) = to_svg(gx2, gy2, ox, oy);
        svg.push_str(&format!(
            r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
            sx1, sy1, sx2, sy2, COLOR_PIN
        ));
    }

    // 文本标注
    for prop in &inst.properties {
        if prop.hide || (prop.key != "Reference" && prop.key != "Value") {
            continue;
        }
        let (sx, sy) = to_svg(prop.at.0, prop.at.1, ox, oy);
        let value = escape_xml_text(&prop.value);
        let color = if prop.key == "Reference" {
            COLOR_REFERENCE
        } else {
            COLOR_VALUE
        };
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="10" fill="{}">{}</text>"##,
            sx, sy, color, value
        ));
    }
}

fn render_junction(svg: &mut String, x: f64, y: f64, ox: f64, oy: f64) {
    let (sx, sy) = to_svg(x, y, ox, oy);
    svg.push_str(&format!(
        r##"<circle cx="{:.2}" cy="{:.2}" r="2.5" fill="{}"/>"##,
        sx, sy, COLOR_WIRE
    ));
}

// ─────────────────────────── 入口 ───────────────────────────

pub fn render(path: &str) -> Result<String, String> {
    render_with_pcb(path, None)
}

pub fn render_with_pcb(path: &str, pcb_path: Option<&str>) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读 .sch 失败: {e}"))?;
    let root = lexpr::from_str(&text).map_err(|e| format!("S-Expression 解析失败: {e}"))?;

    let libs = extract_lib_symbols(&root)?;
    let wires = extract_wires(&root);
    let junctions = extract_junctions(&root);
    let instances = extract_instances(&root);
    let wire_nets = wire_net_names(&wires, &instances, &libs, pcb_path);

    let (min_x, min_y, max_x, max_y) = compute_bbox(&wires, &junctions, &instances, &libs)?;
    let margin = 5.0;
    let ox = min_x - margin;
    let oy = min_y - margin;
    let w_mm = (max_x - min_x) + 2.0 * margin;
    let h_mm = (max_y - min_y) + 2.0 * margin;

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {:.2} {:.2}" preserveAspectRatio="xMidYMid meet" style="width:100%;height:100%;display:block">"##,
        w_mm * SCALE,
        h_mm * SCALE
    );
    svg.push_str(&format!(
        r##"<rect width="100%" height="100%" fill="{}"/>"##,
        COLOR_BACKGROUND
    ));

    // 导线
    for (wire_index, pts) in wires.iter().enumerate() {
        for w in pts.windows(2) {
            let (x1, y1) = to_svg(w[0].0, w[0].1, ox, oy);
            let (x2, y2) = to_svg(w[1].0, w[1].1, ox, oy);
            if let Some(net_name) = &wire_nets[wire_index] {
                let net_name = escape_xml_text(net_name);
                svg.push_str(&format!(
                    r##"<line class="sch-net-hit" data-net="{}" x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="transparent" stroke-width="12"/><line class="sch-net-line" data-net="{}" x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
                    net_name, x1, y1, x2, y2, net_name, x1, y1, x2, y2, COLOR_WIRE
                ));
            } else {
                svg.push_str(&format!(
                    r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
                    x1, y1, x2, y2, COLOR_WIRE
                ));
            }
        }
    }

    // 节点
    for &(x, y) in &junctions {
        render_junction(&mut svg, x, y, ox, oy);
    }

    // 符号实例
    for inst in &instances {
        if let Some(reference) = property_value(inst, "Reference") {
            svg.push_str(&format!(
                r##"<g class="sch-component" data-component="{}">"##,
                escape_xml_text(reference)
            ));
        } else {
            svg.push_str("<g>");
        }
        render_instance(&mut svg, inst, &libs, ox, oy);
        svg.push_str("</g>");
    }

    svg.push_str("</svg>");
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_text_is_xml_escaped_before_svg_insertion() {
        let malicious = r#"<script>alert("owned")</script>&'"#;
        let inst = Inst {
            lib_id: "Device:R".into(),
            at: (0.0, 0.0, 0.0),
            mirror_x: false,
            mirror_y: false,
            unit: 0,
            body_style: 1,
            properties: vec![
                Property {
                    key: "Reference".into(),
                    value: malicious.into(),
                    at: (0.0, 0.0, 0.0),
                    hide: false,
                },
                Property {
                    key: "Value".into(),
                    value: "R1 > R2 & R3".into(),
                    at: (0.0, 1.0, 0.0),
                    hide: false,
                },
            ],
        };
        let libs = HashMap::from([(
            "Device:R".into(),
            HashMap::from([(0, HashMap::from([(1, SubSymbol::default())]))]),
        )]);
        let mut svg = String::new();

        render_instance(&mut svg, &inst, &libs, 0.0, 0.0);

        assert!(!svg.contains("<script>"));
        assert!(!svg.contains(malicious));
        assert!(svg.contains("&lt;script&gt;alert(&quot;owned&quot;)&lt;/script&gt;&amp;&apos;"));
        assert!(svg.contains("R1 &gt; R2 &amp; R3"));
        assert!(svg.contains(r##"fill="#008484">&lt;script"##));
        assert!(svg.contains(r##"fill="#0000c2">R1"##));
    }

    #[test]
    fn pin_connection_end_has_no_junction_dot() {
        let inst = Inst {
            lib_id: "Connector:Test".into(),
            at: (0.0, 0.0, 0.0),
            mirror_x: false,
            mirror_y: false,
            unit: 1,
            body_style: 1,
            properties: vec![],
        };
        let pin = Pin {
            at: (0.0, 0.0, 0.0),
            length: 2.54,
            name: "IN".into(),
            number: "1".into(),
        };
        let libs = HashMap::from([(
            "Connector:Test".into(),
            HashMap::from([(
                1,
                HashMap::from([(
                    1,
                    SubSymbol {
                        graphics: vec![],
                        pins: vec![pin],
                    },
                )]),
            )]),
        )]);
        let mut svg = String::new();

        render_instance(&mut svg, &inst, &libs, 0.0, 0.0);

        assert!(svg.contains(r##"stroke="#840000""##));
        assert!(!svg.contains("<circle"));
    }

    #[test]
    fn explicit_junction_is_still_rendered_in_wire_color() {
        let mut svg = String::new();

        render_junction(&mut svg, 1.0, 2.0, 0.0, 0.0);

        assert_eq!(
            svg,
            r##"<circle cx="10.00" cy="20.00" r="2.5" fill="#009600"/>"##
        );
    }
}
