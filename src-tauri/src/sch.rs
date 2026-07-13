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

const SCALE: f64 = 10.0;

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
                    .filter_map(|xy| parse_xy(xy))
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
                let name = child(item, "name").and_then(as_str).unwrap_or_default();
                let number = child(item, "number").and_then(as_str).unwrap_or_default();
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
                .filter_map(|xy| parse_xy(xy))
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
                r##"<polyline points="{}" fill="none" stroke="#000" stroke-width="{:.2}"/>"##,
                fmt_pts(&svg_pts),
                stroke_w(*stroke)
            ));
        }
        Graphic::Rectangle {
            start,
            end,
            stroke,
            fill,
        } => {
            let corners = [
                (*start, (start.0, end.1)),
                (*start, (end.0, start.1)),
                (*end, (end.0, start.1)),
                (*end, (start.0, end.1)),
            ];
            let svg_pts: Vec<(f64, f64)> = corners
                .iter()
                .map(|&(a, b)| {
                    let (gx, gy) = transform(a.0, a.1, inst.at, inst.mirror_x, inst.mirror_y);
                    let (gx2, gy2) = transform(b.0, b.1, inst.at, inst.mirror_x, inst.mirror_y);
                    // 用两个角定矩形的"两侧",但简化:只画四条边
                    to_svg(gx, gy, ox, oy).0; // dummy
                    to_svg(gx2, gy2, ox, oy).0; // dummy
                    (0.0, 0.0)
                })
                .collect();
            let _ = svg_pts;
            // 实际:直接用 transform 把 4 个角点变换
            let pts = [
                transform(start.0, start.1, inst.at, inst.mirror_x, inst.mirror_y),
                transform(end.0, start.1, inst.at, inst.mirror_x, inst.mirror_y),
                transform(end.0, end.1, inst.at, inst.mirror_x, inst.mirror_y),
                transform(start.0, end.1, inst.at, inst.mirror_x, inst.mirror_y),
            ];
            let svg_pts: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| to_svg(x, y, ox, oy)).collect();
            let fill_str = if *fill == Fill::Background {
                "#fff"
            } else {
                "none"
            };
            svg.push_str(&format!(
                r##"<polygon points="{}" fill="{}" stroke="#000" stroke-width="{:.2}"/>"##,
                fmt_pts(&svg_pts),
                fill_str,
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
                "#fff"
            } else {
                "none"
            };
            svg.push_str(&format!(
                r##"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="#000" stroke-width="{:.2}"/>"##,
                sx,
                sy,
                radius * SCALE,
                fill_str,
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
                "#fff"
            } else {
                "none"
            };
            svg.push_str(&format!(
                r##"<path d="M {:.2},{:.2} A {:.2},{:.2} 0 {},{} {:.2},{:.2}" fill="{}" stroke="#000" stroke-width="{:.2}"/>"##,
                sx, sy, r * SCALE, r * SCALE, large_arc, sweep, ex, ey, fill_str, stroke_w(*stroke)
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
            r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="#000" stroke-width="1"/>"##,
            sx1, sy1, sx2, sy2
        ));
        svg.push_str(&format!(
            r##"<circle cx="{:.2}" cy="{:.2}" r="2" fill="#000"/>"##,
            sx1, sy1
        ));
    }

    // 文本标注
    for prop in &inst.properties {
        if prop.hide || (prop.key != "Reference" && prop.key != "Value") {
            continue;
        }
        let (sx, sy) = to_svg(prop.at.0, prop.at.1, ox, oy);
        svg.push_str(&format!(
                    r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="10" fill="#000">{}: {}</text>"##,
                    sx, sy, prop.key, prop.value
                ));
    }
}

// ─────────────────────────── 入口 ───────────────────────────

pub fn render(path: &str) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读 .sch 失败: {e}"))?;
    let root = lexpr::from_str(&text).map_err(|e| format!("S-Expression 解析失败: {e}"))?;

    let libs = extract_lib_symbols(&root)?;
    let wires = extract_wires(&root);
    let junctions = extract_junctions(&root);
    let instances = extract_instances(&root);

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
    svg.push_str(r##"<rect width="100%" height="100%" fill="#ffffff"/>"##);

    // 导线
    for pts in &wires {
        for w in pts.windows(2) {
            let (x1, y1) = to_svg(w[0].0, w[0].1, ox, oy);
            let (x2, y2) = to_svg(w[1].0, w[1].1, ox, oy);
            svg.push_str(&format!(
                r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="#000" stroke-width="1"/>"##,
                x1, y1, x2, y2
            ));
        }
    }

    // 节点
    for &(x, y) in &junctions {
        let (sx, sy) = to_svg(x, y, ox, oy);
        svg.push_str(&format!(
            r##"<circle cx="{:.2}" cy="{:.2}" r="2.5" fill="#000"/>"##,
            sx, sy
        ));
    }

    // 符号实例
    for inst in &instances {
        render_instance(&mut svg, inst, &libs, ox, oy);
    }

    svg.push_str("</svg>");
    Ok(svg)
}
