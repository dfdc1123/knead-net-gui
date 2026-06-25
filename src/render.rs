//! 把布局结果渲染成 SVG, 简陋但直观, 用于调试。
//!
//! 画布 (z-order, 从下到上):
//! 1. 米色背景
//! 2. 元件包围框 + ref 文字 (每个元件一个色相)
//! 3. 接线 (粗绿线)
//!    - **同行多线**: Z 形走通道, 每条线占一个垂直 lane, 错开避免重叠
//!    - **跨行线**: 保持直线
//! 4. 所有孔: 引脚蓝, 线端绿, 空白
//!
//! 不追求"画得像"电阻二极管, 标出是哪个元件 + 有几个引脚就行。

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use crate::circuit::{Circuit, ComponentId, Position};
use crate::layout::{Breadboard, HoleId, Layout, Occupant, Placement, Wire};

const CELL_X: f32 = 28.0; // 每列像素
const CELL_Y: f32 = 44.0; // 每行像素 (比列宽, 给通道留地方)
const RADIUS: f32 = 5.0; // 孔半径
const MARGIN: f32 = 12.0; // 外边距
const WIRE_W: f32 = 2.0; // 接线线宽 (线密了画细点)
const LANE_SPACING: f32 = 5.0; // 同行多线时垂直错开的像素
const CHANNEL_FRAC: f32 = 0.30; // 通道相对 row 中心的偏移 (向下)

/// 把整个布局渲染成 SVG 字符串。
///
/// `layout` 可以是不合法的 (没有 placement / wire 冲突), 此时仍会画板子和孔,
/// 只是引脚和线的颜色信息可能缺失。
pub fn to_svg(circuit: &Circuit, board: &Breadboard, layout: &Layout) -> String {
    let w = board.cols() as f32 * CELL_X + MARGIN * 2.0;
    let h = board.rows() as f32 * CELL_Y + MARGIN * 2.0;
    // 用 lossy 占用: 布局有冲突时, 仍能以“后覆盖先”填上 pin/wire,
    // 至少能看出 pin 位租和走线意图。 严格版会 return Err 导致孔全白。
    let occ = crate::layout::Occupancy::from_layout_lossy(layout, board);

    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="sans-serif">"#
    );
    let _ = writeln!(
        out,
        r##"<rect width="100%" height="100%" fill="#fafafa"/>"##
    );

    // 1) 元件包围框 + ref 文字
    for (i, slot) in layout.placements().iter().enumerate() {
        let Some(placement) = *slot else { continue };
        let component = &circuit.components()[i];
        if component.footprint().is_none() {
            continue;
        }
        let Some(bbox) = component_bbox(circuit, board, component.id(), placement) else {
            continue;
        };

        let bx = MARGIN + bbox.min_x as f32 * CELL_X;
        let by = MARGIN + bbox.min_y as f32 * CELL_Y;
        let bw = (bbox.max_x - bbox.min_x + 1) as f32 * CELL_X;
        let bh = (bbox.max_y - bbox.min_y + 1) as f32 * CELL_Y;
        let cx = bx + bw / 2.0;
        let cy = by + bh / 2.0;
        let color = component_color(i);

        let _ = writeln!(
            out,
            r#"<rect x="{bx:.1}" y="{by:.1}" width="{bw:.1}" height="{bh:.1}" rx="4" fill="{color}" fill-opacity="0.18" stroke="{color}" stroke-width="1.5"/>"#
        );
        let _ = writeln!(
            out,
            r#"<text x="{cx:.1}" y="{cy:.1}" font-size="13" font-weight="bold" fill="{color}" text-anchor="middle" dominant-baseline="central">{}</text>"#,
            html_escape(component.ref_())
        );
    }

    // 2) 接线 (同行 → Z 形通道, 跨行 → 直线)
    let plans = plan_wires(board, layout.wires());
    for plan in &plans {
        draw_wire(&mut out, plan, circuit);
    }

    // 3) 所有孔
    for hole in board.holes() {
        let (cx, cy) = hole_px_from_pos(hole.position);
        let (fill, stroke) = match occ.occupant_at(hole.id) {
            Some(Occupant::Pin(_)) => ("#2563eb", "#1e3a8a"),
            Some(Occupant::Wire(_)) => ("#16a34a", "#14532d"),
            None => ("#ffffff", "#cbd5e1"),
        };
        let _ = writeln!(
            out,
            r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{RADIUS}" fill="{fill}" stroke="{stroke}" stroke-width="1"/>"#
        );
    }

    let _ = writeln!(out, "</svg>");
    out
}

// ---------- 孔坐标 ----------

fn hole_px(board: &Breadboard, id: HoleId) -> (f32, f32) {
    hole_px_from_pos(board.hole(id).position)
}

fn hole_px_from_pos(pos: Position) -> (f32, f32) {
    (
        MARGIN + pos.x as f32 * CELL_X + CELL_X / 2.0,
        MARGIN + pos.y as f32 * CELL_Y + CELL_Y / 2.0,
    )
}

// ---------- 接线规划 ----------

struct WirePlan {
    /// true = Z 形 (走通道, 同行 wire), false = 直线 (跨行 wire)
    routed: bool,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    /// 只有 routed = true 时用: 通道水平段的 y 坐标
    y_channel: f32,
    /// net id (用于上色和标签)
    net_idx: usize,
}

/// 决定每根 wire 怎么画。
///
/// 算法:
/// 1. 把所有"两端 y 相同"的 wire 按行分组 (跨行的直接走直线)。
/// 2. 每组内按 from.x 排序, 居中分配 lane 偏移 (-N/2 .. +N/2 个 `LANE_SPACING`)。
/// 3. 通道 y = 行中心 y + `CELL_Y * CHANNEL_FRAC` + lane 偏移。
///    这样每根线在同行内垂直错开, 横向段不再重叠。
fn plan_wires(board: &Breadboard, wires: &[Wire]) -> Vec<WirePlan> {
    // 1) 收集同行 wire
    let mut by_row: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, w) in wires.iter().enumerate() {
        let p1 = board.hole(w.from).position;
        let p2 = board.hole(w.to).position;
        if p1.y == p2.y {
            by_row.entry(p1.y).or_default().push(i);
        }
    }

    // 2) 每组内按 from.x 排序, 居中分配 lane
    let mut lane_offset: HashMap<usize, f32> = HashMap::new();
    for indices in by_row.values() {
        let mut sorted: Vec<usize> = indices.clone();
        sorted.sort_by_key(|&i| board.hole(wires[i].from).position.x);
        let n = sorted.len() as f32;
        let center = (n - 1.0) / 2.0;
        for (lane, &i) in sorted.iter().enumerate() {
            lane_offset.insert(i, (lane as f32 - center) * LANE_SPACING);
        }
    }

    // 3) 构造 plan
    wires
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let p1 = board.hole(w.from).position;
            let p2 = board.hole(w.to).position;
            let (x1, y1) = hole_px_from_pos(p1);
            let (x2, y2) = hole_px_from_pos(p2);
            let net_idx = w.net.raw();

            if let Some(&off) = lane_offset.get(&i) {
                WirePlan {
                    routed: true,
                    x1,
                    y1,
                    x2,
                    y2,
                    y_channel: y1 + CELL_Y * CHANNEL_FRAC + off,
                    net_idx,
                }
            } else {
                WirePlan {
                    routed: false,
                    x1,
                    y1,
                    x2,
                    y2,
                    y_channel: 0.0,
                    net_idx,
                }
            }
        })
        .collect()
}

fn draw_wire(out: &mut String, plan: &WirePlan, circuit: &Circuit) {
    let color = net_color(plan.net_idx);
    let name = html_escape(circuit.nets()[plan.net_idx].name());

    if plan.routed {
        // Z 形: (x1, y1) → (x1, y_ch) → (x2, y_ch) → (x2, y2)
        let _ = writeln!(
            out,
            r##"<path d="M {x1:.1} {y1:.1} L {x1:.1} {yc:.1} L {x2:.1} {yc:.1} L {x2:.1} {y2:.1}" fill="none" stroke="{color}" stroke-width="{W}" stroke-linecap="round" stroke-linejoin="round" opacity="0.9"/>"##,
            x1 = plan.x1,
            y1 = plan.y1,
            yc = plan.y_channel,
            x2 = plan.x2,
            y2 = plan.y2,
            W = WIRE_W,
            color = color,
        );
        // 标签: 通道中段, 白色描边保证压在绿线上可读
        let mx = (plan.x1 + plan.x2) / 2.0;
        let my = plan.y_channel;
        let _ = writeln!(
            out,
            r##"<text x="{mx:.1}" y="{my:.1}" font-size="9" fill="{color}" stroke="#fafafa" stroke-width="3" paint-order="stroke" stroke-linejoin="round" text-anchor="middle" dominant-baseline="central">{name}</text>"##,
            mx = mx,
            my = my,
            color = color,
            name = name,
        );
    } else {
        let _ = writeln!(
            out,
            r##"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{color}" stroke-width="{W}" stroke-linecap="round" opacity="0.9"/>"##,
            x1 = plan.x1,
            y1 = plan.y1,
            x2 = plan.x2,
            y2 = plan.y2,
            W = WIRE_W,
            color = color,
        );
        // 标签: 线中点, 同样白描边
        let mx = (plan.x1 + plan.x2) / 2.0;
        let my = (plan.y1 + plan.y2) / 2.0;
        let _ = writeln!(
            out,
            r##"<text x="{mx:.1}" y="{my:.1}" font-size="9" fill="{color}" stroke="#fafafa" stroke-width="3" paint-order="stroke" stroke-linejoin="round" text-anchor="middle" dominant-baseline="central">{name}</text>"##,
            mx = mx,
            my = my,
            color = color,
            name = name,
        );
    }
}

// ---------- 小工具 ----------

/// 给元件分配一个稳定的色相 (按 id 散开)。黄金比例乘子 47 是经验值, 保证相邻 id 颜色差大。
fn component_color(idx: usize) -> String {
    let hue = (idx.wrapping_mul(47)) % 360;
    format!("hsl({hue},65%,42%)")
}

/// 给 net 分配色相, 乘子跟元件不同 (73 vs 47) + 较亮, 让线跟元件框区分开。
fn net_color(idx: usize) -> String {
    let hue = (idx.wrapping_mul(73)) % 360;
    format!("hsl({hue},65%,48%)")
}

/// HTML 实体转义, 防止 ref 含奇怪字符 (KiCad ref 实际只有字母+数字, 保险起见)。
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------- 元件包围盒 ----------

struct BBox {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
}

/// 算出该 placement 下, 元件所有 pin 在板上的最小包围盒。
/// 调 `Placement::apply`, 即使是旋转的 footprint 也能算对。
fn component_bbox(
    circuit: &Circuit,
    board: &Breadboard,
    cid: ComponentId,
    p: Placement,
) -> Option<BBox> {
    let comp = &circuit.components()[cid.raw()];
    let fid = comp.footprint()?;
    let fp = &circuit.footprints()[fid.raw()];
    let placed = p.apply(comp, fp, board, circuit.pins()).ok()?;
    let (mut min_x, mut max_x) = (i32::MAX, i32::MIN);
    let (mut min_y, mut max_y) = (i32::MAX, i32::MIN);
    for ph in &placed.pin_holes {
        let pos = board.hole(ph.hole).position;
        min_x = min_x.min(pos.x);
        max_x = max_x.max(pos.x);
        min_y = min_y.min(pos.y);
        max_y = max_y.max(pos.y);
    }
    Some(BBox {
        min_x,
        max_x,
        min_y,
        max_y,
    })
}
