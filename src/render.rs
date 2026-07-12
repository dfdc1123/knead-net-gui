//! 把布局结果渲染成 SVG, 简陋但直观, 用于调试。
//!
//! 画布 (z-order, 从下到上):
//! 1. 近白中性灰背景 (`#fafafa`)
//! 2. 元件包围框 + ref 文字 (每个元件一个色相)
//! 3. 接线 (按 net 上色, 半透明粗线)
//!    - **同行多线**: Z 形走通道, 每条线占一个垂直 lane, 错开避免重叠
//!    - **跨行线**: 保持直线
//! 4. 所有孔: 引脚蓝, 线端绿, 元件本体 (`Blocked`) 用对应元件色, 空白
//!
//! 不追求画出逼真的电阻 / LED / 二极管图标; 给出 ref 标签 + 几个引脚就够调试用。

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use crate::circuit::{Circuit, ComponentId, Position};
use crate::layout::placement::BBox;
use crate::layout::{Breadboard, HoleId, Layout, Occupant, Placement, Polarity, Region, Wire};

const CELL_X: f32 = 28.0; // 每列像素
const CELL_Y: f32 = 44.0; // 每行像素 (比列宽, 给通道留地方)
const RADIUS: f32 = 5.0; // 孔半径
const MARGIN: f32 = 12.0; // 外边距
const WIRE_W: f32 = 2.0; // 接线线宽 (线密了画细点)
const LANE_SPACING: f32 = 5.0; // 同行多线时垂直错开的像素
const CHANNEL_FRAC: f32 = 0.30; // 通道相对 row 中心的偏移 (向下)

/// 把整个布局渲染成 SVG 字符串。
///
/// `layout` 可以是部分不合法的 (未摆放的 component / wire 跟 pin 撞):
/// 用 lossy occupancy 渲染, 未摆放的 component 不画, 撞孔时后到者覆盖先到者。
/// 颜色信息本身不会"缺失" — net/线色取自 [`Circuit::nets`] 和 `net_color()`,
/// pin 覆盖关系才是 lossless / lossy 的差别。
/// 渲染一个空板 (没元件、没接线) 到 SVG。仅画板几何 + 电源轨色带 + 所有孔。
///
/// 给 `--render-empty-boards` 这类验证场景用 — 能一眼看清板子尺寸、blocked row、
/// 电源轨 group 划分对不对。
pub fn to_svg_board(board: &Breadboard) -> String {
    use crate::circuit::Circuit;
    use crate::layout::Layout;
    let circuit = Circuit::empty();
    let layout = Layout::new(&circuit);
    to_svg(&circuit, board, &layout)
}

pub fn to_svg(circuit: &Circuit, board: &Breadboard, layout: &Layout) -> String {
    let w = board.cols() as f32 * CELL_X + MARGIN * 2.0;
    // 画布高度 = 从板上最小 y 到最大 y 覆盖 (含 power rail 的负 y 和 >= main_rows)
    let (y_min, y_max) = y_extent(board);
    let h = (y_max - y_min + 1) as f32 * CELL_Y + MARGIN * 2.0;
    let y_offset = -y_min; // 把所有 y 偏移到非负, 用于 hole_px_from_pos
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

    // 画中央通道 (blocked row 拼接的带) — 面包板中线的简化表示。
    if let Some((gap_top, gap_bottom)) = gap_extent(board) {
        let gap_y = MARGIN + (gap_top as i32 + y_offset) as f32 * CELL_Y;
        let gap_h = (gap_bottom - gap_top + 1) as f32 * CELL_Y;
        let inner_w = board.cols() as f32 * CELL_X;
        let _ = writeln!(
            out,
            r##"<rect x="{MARGIN}" y="{gap_y:.1}" width="{inner_w}" height="{gap_h:.1}" fill="#e2e8f0" stroke="#94a3b8" stroke-width="0.5"/>"##
        );
    }

    // 画 external gap (主区到 power rail 之间的 2 行间隔, 同款灰带)
    for (gap_top, gap_bottom) in board.external_gaps() {
        let gap_y = MARGIN + (gap_top + y_offset) as f32 * CELL_Y;
        let gap_h = (gap_bottom - gap_top + 1) as f32 * CELL_Y;
        let inner_w = board.cols() as f32 * CELL_X;
        let _ = writeln!(
            out,
            r##"<rect x="{MARGIN}" y="{gap_y:.1}" width="{inner_w}" height="{gap_h:.1}" fill="#e2e8f0" stroke="#94a3b8" stroke-width="0.5"/>"##
        );
    }

    // 画电源轨: 红/蓝色色带 + 5 组 5 孔断开的样式
    if let Some(pr) = board.power_rails() {
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            let rail_y = MARGIN + (rail.y + y_offset) as f32 * CELL_Y;
            let (color, label) = match rail.polarity {
                Polarity::Positive => ("#dc2626", "+"),
                Polarity::Negative => ("#2563eb", "−"),
            };
            // 每个 group 画一条细长色带
            for g in &rail.groups {
                let gx = MARGIN + *g.start() as f32 * CELL_X;
                let gw = (*g.end() - *g.start() + 1) as f32 * CELL_X;
                let _ = writeln!(
                    out,
                    r##"<rect x="{gx:.1}" y="{rail_y:.1}" width="{gw:.1}" height="{ch}" fill="{color}" fill-opacity="0.10" stroke="{color}" stroke-width="0.6" stroke-dasharray="3,2"/>"##,
                    gx = gx,
                    gw = gw,
                    rail_y = rail_y,
                    ch = CELL_Y,
                    color = color
                );
            }
            // 行首标极性
            let _ = writeln!(
                out,
                r##"<text x="{}" y="{:.1}" font-size="11" font-weight="bold" fill="{color}" text-anchor="end" dominant-baseline="central">{label}</text>"##,
                MARGIN - 3.0,
                rail_y + CELL_Y / 2.0,
                color = color,
                label = label
            );
        }
    }

    // 1) 元件包围框 + ref 文字
    for (i, slot) in layout.placements().iter().enumerate() {
        let Some(placement) = *slot else { continue };
        let component = &circuit.components()[i];
        if component.footprint().is_none() {
            continue;
        }
        match placement {
            Placement::OnBoard { .. } => {
                let Some(bbox) = component_bbox(circuit, board, component.id(), placement) else {
                    continue;
                };

                let bx = MARGIN + bbox.min_x as f32 * CELL_X;
                let by = MARGIN + (bbox.min_y + y_offset) as f32 * CELL_Y;
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
            Placement::Bridged { pin_holes } => {
                draw_bridged(
                    &mut out,
                    circuit,
                    board,
                    component.id(),
                    &pin_holes,
                    y_offset,
                );
            }
        }
    }

    // 2) 接线 (同行 → Z 形通道, 跨行 → 直线)
    let plans = plan_wires(board, layout.wires(), y_offset);
    for plan in &plans {
        draw_wire(&mut out, plan, circuit);
    }

    // 3) 所有孔
    for hole in board.holes() {
        let (cx, cy) = hole_px_from_pos(Position {
            x: hole.position.x,
            y: hole.position.y + y_offset,
        });
        let blocked_color;
        let (fill, stroke): (&str, &str) = match occ.occupant_at(hole.id) {
            Some(Occupant::Pin(_)) => ("#2563eb", "#1e3a8a"),
            Some(Occupant::Wire(_)) => ("#16a34a", "#14532d"),
            // 被元件本体占据 — 用同元件的 hue 但 fill-opacity 为 1.0
            // (bbox 的 fill-opacity=0.18 让颜色淡化, 这里反过来更鲜),
            // 让“本体覆盖区”一眼能跟空孔 / 单纯 pin 区分开。
            Some(Occupant::Blocked(cid)) => {
                blocked_color = component_color(cid.raw());
                (blocked_color.as_str(), "#374151")
            }
            None => {
                // 电源轨空孔: 略深一点的色, 跟色带背景区分
                match hole.region {
                    Region::PowerRail => ("#f3f4f6", "#9ca3af"),
                    Region::MainRail => ("#ffffff", "#cbd5e1"),
                }
            }
        };
        let _ = writeln!(
            out,
            r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{RADIUS}" fill="{fill}" stroke="{stroke}" stroke-width="1"/>"#
        );
    }

    let _ = writeln!(out, "</svg>");
    out
}

/// 板上所有 y 值的范围 (含 power rail 的负 y / >= main_rows 的 y)。
/// 返回 (min, max), 闭区间。
fn y_extent(board: &Breadboard) -> (i32, i32) {
    let mut lo = 0;
    let mut hi = board.main_rows() as i32 - 1;
    if let Some(pr) = board.power_rails() {
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            lo = lo.min(rail.y);
            hi = hi.max(rail.y);
        }
    }
    (lo, hi)
}

// ---------- 孔坐标 ----------

fn hole_px_from_pos(pos: Position) -> (f32, f32) {
    (
        MARGIN + pos.x as f32 * CELL_X + CELL_X / 2.0,
        MARGIN + pos.y as f32 * CELL_Y + CELL_Y / 2.0,
    )
}

/// 算中央通道 (连续 blocked row) 的 (top, bottom) 范围, 没有则返回 None。
/// Breadboard 允许任意的 blocked_rows 集合 (不强制一段),
/// 这里选最长连续段; 多段连不上的情况下只返回第一段的范围。
fn gap_extent(board: &Breadboard) -> Option<(usize, usize)> {
    let blocked = board.blocked_rows();
    if blocked.is_empty() {
        return None;
    }
    // 找最长连续段
    let mut best = (blocked[0], blocked[0]);
    let mut cur_start = blocked[0];
    let mut cur_end = blocked[0];
    for &r in blocked.iter().skip(1) {
        if r == cur_end + 1 {
            cur_end = r;
        } else {
            if cur_end - cur_start > best.1 - best.0 {
                best = (cur_start, cur_end);
            }
            cur_start = r;
            cur_end = r;
        }
    }
    if cur_end - cur_start > best.1 - best.0 {
        best = (cur_start, cur_end);
    }
    Some(best)
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
///
/// `y_offset` 把面包板坐标系的 y (可能为负, 含 power rail) 偏到画布像素 y。
fn plan_wires(board: &Breadboard, wires: &[Wire], y_offset: i32) -> Vec<WirePlan> {
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
            let (x1, y1) = hole_px_from_pos(Position {
                x: p1.x,
                y: p1.y + y_offset,
            });
            let (x2, y2) = hole_px_from_pos(Position {
                x: p2.x,
                y: p2.y + y_offset,
            });
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

/// 给元件分配一个稳定的色相 (按 id 散开)。乘子 47 与 360 互素,
/// 保证相邻 id 的色相在 [0, 360) 上均匀散布; net 用 73 跟元件错开避免撞色。
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

/// 算出该 placement 下, 元件在板上的包围盒。调 [`Placement::apply`] 后
/// 读 `placed.bbox` 字段 (跟 `Occupancy::from_layout` 走同一条路径)。
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
    placed.bbox
}

/// 画一个 bridged 元件: 两根 leads (从 body 到两个 pin) + body (中点小矩形, 同 OnBoard 风格)。
///
/// `y_offset` 是把面包板 y 偏到画布像素 y 的偏移量。
fn draw_bridged(
    out: &mut String,
    circuit: &Circuit,
    board: &Breadboard,
    cid: ComponentId,
    pin_holes: &[(HoleId, crate::circuit::PinId); 2],
    y_offset: i32,
) {
    let comp = &circuit.components()[cid.raw()];
    let color = component_color(cid.raw());

    let (h1, _) = pin_holes[0];
    let (h2, _) = pin_holes[1];
    let p1 = board.hole(h1).position;
    let p2 = board.hole(h2).position;
    let (x1, y1) = hole_px_from_pos(Position {
        x: p1.x,
        y: p1.y + y_offset,
    });
    let (x2, y2) = hole_px_from_pos(Position {
        x: p2.x,
        y: p2.y + y_offset,
    });

    // body: 在两点中点画小矩形, 朝向跟 leads 一致
    // (轴线和 legs 同向, 看起来像真电阻的 body)
    let mx = (x1 + x2) / 2.0;
    let my = (y1 + y2) / 2.0;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let length = (dx * dx + dy * dy).sqrt();
    // body 长度 = leads 长度的 40% (min 24, max 44), 宽度 14
    let body_len = (length * 0.4).clamp(24.0, 44.0);
    let body_w = 14.0_f32;
    // 角度 (度): leads 方向 = body 长边方向
    let angle = dy.atan2(dx).to_degrees();

    // leads: 两根细线从 pin 孔到 body 端点
    let _ = writeln!(
        out,
        r##"<line x1="{x1:.1}" y1="{y1:.1}" x2="{mx:.1}" y2="{my:.1}" stroke="{color}" stroke-width="2" stroke-linecap="round" opacity="0.85"/>"##,
        x1 = x1,
        y1 = y1,
        mx = mx,
        my = my,
        color = color
    );
    let _ = writeln!(
        out,
        r##"<line x1="{x2:.1}" y1="{y2:.1}" x2="{mx:.1}" y2="{my:.1}" stroke="{color}" stroke-width="2" stroke-linecap="round" opacity="0.85"/>"##,
        x2 = x2,
        y2 = y2,
        mx = mx,
        my = my,
        color = color
    );

    // body: 旋转矩形, 跟 OnBoard 类似 (圆角 + 半透明 + 边线),
    // 但 rx=3 / fill-opacity=0.30 跟 OnBoard (rx=4 / 0.18) 都略不同,
    // 让桥接元件比板上的更醒目。
    let _ = writeln!(
        out,
        r##"<rect x="{bx:.1}" y="{by:.1}" width="{bw:.1}" height="{bh:.1}" rx="3" fill="{color}" fill-opacity="0.30" stroke="{color}" stroke-width="1.5" transform="rotate({angle:.1} {mx:.1} {my:.1})"/>"##,
        bx = mx - body_len / 2.0,
        by = my - body_w / 2.0,
        bw = body_len,
        bh = body_w,
        color = color,
        angle = angle,
        mx = mx,
        my = my,
    );
    // ref 标签: 跟着 body 旋转 (保证文字始终朝上有点不现实, 这里直接水平画在 body 中心)
    let _ = writeln!(
        out,
        r##"<text x="{mx:.1}" y="{my:.1}" font-size="10" font-weight="bold" fill="{color}" text-anchor="middle" dominant-baseline="central" stroke="#fafafa" stroke-width="2" paint-order="stroke">{ref_}</text>"##,
        mx = mx,
        my = my,
        color = color,
        ref_ = html_escape(comp.ref_())
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId,
    };
    use crate::layout::Layout;

    fn board() -> Breadboard {
        Breadboard::standard()
    }

    /// 验证 bridged 元件被画成: 2 根 leads + 1 个 body 矩形 + ref 标签
    #[test]
    fn bridged_renders_with_leads_and_body() {
        let fp = Footprint {
            id: FootprintId(0),
            name: "R_BR".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 5, y: 0 },
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
        let pins = vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                physical_pin_index: 0,
                net: None,
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                physical_pin_index: 1,
                net: None,
            },
        ];
        let circuit = Circuit {
            components: vec![r1],
            pins,
            nets: vec![Net {
                id: NetId(0),
                name: "n".into(),
                pins: vec![],
            }],
            footprints: vec![fp],
        };

        let b = board();
        let h_main = b.at(5, 0).unwrap();
        let h_rail = b.at(0, -4).unwrap();
        let placement = Placement::Bridged {
            pin_holes: [(h_main, PinId(0)), (h_rail, PinId(1))],
        };
        let mut layout = Layout::new(&circuit);
        layout.place(ComponentId(0), placement);
        let svg = to_svg(&circuit, &b, &layout);

        // 验证 SVG 里有 body 相关的 SVG 元素 (rect 旋转, 文字, leads 线)
        assert!(svg.contains("R1"), "SVG 应该有 ref 标签 R1");
        assert!(svg.contains("<line"), "SVG 应该有 leads (line 元素)");
        assert!(
            svg.contains("rotate("),
            "SVG 里 body 矩形应该旋转 (rotate transform)"
        );
    }
}
