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
use std::fmt;
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
const COLOR_BUS: &str = "#0000c2";
const COLOR_LABEL: &str = "#0000c2";
const COLOR_SHEET: &str = "#840084";
const COLOR_REFERENCE: &str = "#008484";
const COLOR_VALUE: &str = "#0000c2";
const COMPONENT_HIT_PADDING: f64 = 6.0;
const COMPONENT_HIT_MIN_SIZE: f64 = 24.0;

// ─────────────────────────── 数据结构 ───────────────────────────

#[derive(Debug, Clone)]
enum Graphic {
    Polyline {
        pts: Vec<(f64, f64)>,
        stroke: f64,
        fill: Fill,
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
    Outline,
    Background,
}

#[derive(Debug, Clone)]
struct Pin {
    at: (f64, f64, f64), // x, y, direction(度)
    length: f64,
    name: String,
    number: String,
    name_text: PinText,
    number_text: PinText,
    name_offset: f64,
    electrical_type: String,
    shape: String,
}

#[derive(Debug, Clone, Copy)]
struct PinText {
    visible: bool,
    font_size: f64,
}

impl Default for PinText {
    fn default() -> Self {
        Self {
            visible: true,
            font_size: 1.27,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PinTextSettings {
    names: PinText,
    numbers: PinText,
    name_offset: f64,
}

impl Default for PinTextSettings {
    fn default() -> Self {
        Self {
            names: PinText::default(),
            numbers: PinText::default(),
            // KiCad's file-format default when the pin_names section is absent.
            name_offset: 0.508,
        }
    }
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
    show_name: bool,
    font_size: f64,
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
    exclude_from_sim: Option<bool>,
    in_bom: Option<bool>,
    on_board: Option<bool>,
    in_pos_files: Option<bool>,
    dnp: Option<bool>,
}

#[derive(Debug, Clone)]
struct StrokePath {
    pts: Vec<(f64, f64)>,
    stroke: f64,
}

#[derive(Debug, Clone, Copy)]
struct BusEntry {
    at: (f64, f64),
    size: (f64, f64),
    stroke: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchematicTextKind {
    Text,
    LocalLabel,
    GlobalLabel,
    HierarchicalLabel,
}

#[derive(Debug, Clone)]
struct SchematicText {
    kind: SchematicTextKind,
    text: String,
    at: (f64, f64, f64),
    shape: Option<String>,
    font_size: f64,
    hide: bool,
}

#[derive(Debug, Clone)]
struct SheetPin {
    name: String,
    electrical_type: String,
    at: (f64, f64, f64),
    font_size: f64,
    hide: bool,
}

#[derive(Debug, Clone)]
struct Sheet {
    at: (f64, f64),
    size: (f64, f64),
    stroke: f64,
    properties: Vec<Property>,
    pins: Vec<SheetPin>,
}

/// 从 `.kicad_sch` 提取的、供装配视图使用的逻辑引脚信息。
#[derive(Debug, Clone, Default)]
pub(crate) struct ComponentMetadata {
    pub(crate) value: Option<String>,
    pub(crate) footprint: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) datasheet: Option<String>,
    pub(crate) pins: HashMap<String, PinMetadata>,
    pub(crate) properties: Vec<ComponentProperty>,
    pub(crate) exclude_from_sim: Option<bool>,
    pub(crate) in_bom: Option<bool>,
    pub(crate) on_board: Option<bool>,
    pub(crate) in_pos_files: Option<bool>,
    pub(crate) dnp: Option<bool>,
}

#[derive(Debug, Clone)]
pub(crate) struct ComponentProperty {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) hidden: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PinMetadata {
    pub(crate) name: Option<String>,
    pub(crate) electrical_type: Option<String>,
    pub(crate) shape: Option<String>,
    pub(crate) unit: Option<u32>,
}

pub(crate) type ComponentMetadataMap = HashMap<String, ComponentMetadata>;

#[derive(Debug)]
pub(crate) enum RenderError {
    Read(String),
    Parse(String),
    MissingLibrarySymbols,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(detail) => write!(formatter, "Failed to read schematic: {detail}"),
            Self::Parse(detail) => write!(formatter, "Failed to parse S-expression: {detail}"),
            Self::MissingLibrarySymbols => formatter.write_str("Missing lib_symbols node"),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct LibraryMetadata {
    description: Option<String>,
    datasheet: Option<String>,
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
        .and_then(|fill| {
            child(fill, "type")
                .and_then(|kind| list_items(kind).nth(1))
                .or_else(|| list_items(fill).nth(1))
        })
        .and_then(as_symbol)
        .map(|s| match s {
            "background" => Fill::Background,
            "outline" => Fill::Outline,
            _ => Fill::None,
        })
        .unwrap_or(Fill::None)
}

fn token_bool(v: &Value, name: &str) -> Option<bool> {
    for item in list_items(v) {
        if item.as_symbol() == Some(name) {
            return Some(true);
        }
        let Some(cons) = item.as_cons() else {
            continue;
        };
        if cons.car().as_symbol() != Some(name) {
            continue;
        }
        return match list_items(item).nth(1).and_then(as_symbol) {
            Some("yes") => Some(true),
            Some("no") => Some(false),
            _ => Some(true),
        };
    }
    None
}

fn is_hidden(v: &Value) -> bool {
    token_bool(v, "hide") == Some(true)
}

fn text_font_size(v: &Value) -> Option<f64> {
    child(v, "effects")
        .and_then(|effects| child(effects, "font"))
        .and_then(|font| child(font, "size"))
        .and_then(|size| list_items(size).nth(1))
        .and_then(as_f64)
}

fn extract_pin_text_settings(symbol: &Value) -> PinTextSettings {
    let mut settings = PinTextSettings::default();
    if let Some(pin_names) = child(symbol, "pin_names") {
        settings.names.visible = !is_hidden(pin_names);
        settings.name_offset = child(pin_names, "offset")
            .and_then(parse_number_child)
            .unwrap_or(settings.name_offset);
    }
    if let Some(pin_numbers) = child(symbol, "pin_numbers") {
        settings.numbers.visible = !is_hidden(pin_numbers);
    }
    settings
}

fn parse_property(property: &Value) -> Property {
    let mut iter = list_items(property);
    let _ = iter.next();
    let key = iter.next().and_then(as_str).unwrap_or_default();
    let value = iter.next().and_then(as_str).unwrap_or_default();
    Property {
        key,
        value,
        at: child(property, "at")
            .and_then(parse_at)
            .unwrap_or((0.0, 0.0, 0.0)),
        hide: is_hidden(property),
        show_name: token_bool(property, "show_name") == Some(true),
        font_size: text_font_size(property).unwrap_or(1.27),
    }
}

// ─────────────────────────── 提取 ───────────────────────────

fn extract_lib_symbols(root: &Value) -> Result<LibMap, RenderError> {
    let mut libs = LibMap::new();
    let lib_symbols_node = child(root, "lib_symbols").ok_or(RenderError::MissingLibrarySymbols)?;

    for sym_node in children(lib_symbols_node, "symbol") {
        // 顶层 symbol: (symbol "NAME" ...)
        let mut iter = list_items(sym_node);
        let _ = iter.next();
        let name = iter.next().and_then(as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }

        let mut unit_map: HashMap<u32, HashMap<u32, SubSymbol>> = HashMap::new();
        let pin_text_settings = extract_pin_text_settings(sym_node);

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
            let (graphics, pins) = extract_body(&body, pin_text_settings);

            unit_map
                .entry(unit)
                .or_default()
                .insert(style, SubSymbol { graphics, pins });
        }

        libs.insert(name, unit_map);
    }

    Ok(libs)
}

fn extract_body(v: &Value, pin_text_settings: PinTextSettings) -> (Vec<Graphic>, Vec<Pin>) {
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
                    fill: parse_fill(item),
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
                let radius = child(item, "radius")
                    .and_then(parse_number_child)
                    .unwrap_or(0.0);
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
                let name_node = child(item, "name");
                let name = name_node
                    .and_then(|value| value.list_iter().into_iter().flatten().nth(1))
                    .and_then(as_str)
                    .unwrap_or_default();
                let number_node = child(item, "number");
                let number = number_node
                    .and_then(|value| value.list_iter().into_iter().flatten().nth(1))
                    .and_then(as_str)
                    .unwrap_or_default();
                let mut name_text = pin_text_settings.names;
                if let Some(font_size) = name_node.and_then(text_font_size) {
                    name_text.font_size = font_size;
                }
                let mut number_text = pin_text_settings.numbers;
                if let Some(font_size) = number_node.and_then(text_font_size) {
                    number_text.font_size = font_size;
                }
                let electrical_type = list_items(item)
                    .nth(1)
                    .and_then(as_symbol)
                    .unwrap_or_default()
                    .to_string();
                let shape = list_items(item)
                    .nth(2)
                    .and_then(as_symbol)
                    .unwrap_or_default()
                    .to_string();
                pins.push(Pin {
                    at,
                    length,
                    name,
                    number,
                    name_text,
                    number_text,
                    name_offset: pin_text_settings.name_offset,
                    electrical_type,
                    shape,
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

fn extract_stroke_paths(root: &Value, name: &str) -> Vec<StrokePath> {
    children(root, name)
        .iter()
        .map(|path| StrokePath {
            pts: children(path, "pts")
                .iter()
                .flat_map(|pts| children(pts, "xy"))
                .filter_map(parse_xy)
                .collect(),
            stroke: parse_stroke(path),
        })
        .collect()
}

fn extract_bus_entries(root: &Value) -> Vec<BusEntry> {
    children(root, "bus_entry")
        .iter()
        .filter_map(|entry| {
            Some(BusEntry {
                at: child(entry, "at").and_then(parse_xy)?,
                size: child(entry, "size").and_then(parse_xy)?,
                stroke: parse_stroke(entry),
            })
        })
        .collect()
}

fn extract_no_connects(root: &Value) -> Vec<(f64, f64)> {
    children(root, "no_connect")
        .iter()
        .filter_map(|marker| child(marker, "at").and_then(parse_xy))
        .collect()
}

fn extract_schematic_texts(root: &Value) -> Vec<SchematicText> {
    let mut texts = Vec::new();
    for (token, kind) in [
        ("text", SchematicTextKind::Text),
        ("label", SchematicTextKind::LocalLabel),
        ("global_label", SchematicTextKind::GlobalLabel),
        ("hierarchical_label", SchematicTextKind::HierarchicalLabel),
    ] {
        for node in children(root, token) {
            let Some(text) = list_items(node).nth(1).and_then(as_str) else {
                continue;
            };
            let Some(at) = child(node, "at").and_then(parse_at) else {
                continue;
            };
            let shape = child(node, "shape")
                .and_then(|shape| list_items(shape).nth(1))
                .and_then(as_symbol)
                .map(str::to_string);
            texts.push(SchematicText {
                kind,
                text,
                at,
                shape,
                font_size: text_font_size(node).unwrap_or(1.27),
                hide: child(node, "effects").is_some_and(is_hidden),
            });
        }
    }
    texts
}

fn extract_sheets(root: &Value) -> Vec<Sheet> {
    children(root, "sheet")
        .iter()
        .filter_map(|sheet| {
            let at = child(sheet, "at").and_then(parse_xy)?;
            let size = child(sheet, "size").and_then(parse_xy)?;
            let properties = children(sheet, "property")
                .into_iter()
                .map(parse_property)
                .collect();
            let pins = children(sheet, "pin")
                .into_iter()
                .filter_map(|pin| {
                    let name = list_items(pin).nth(1).and_then(as_str)?;
                    let electrical_type = list_items(pin)
                        .nth(2)
                        .and_then(as_symbol)
                        .unwrap_or_default()
                        .to_string();
                    let at = child(pin, "at").and_then(parse_at)?;
                    Some(SheetPin {
                        name,
                        electrical_type,
                        at,
                        font_size: text_font_size(pin).unwrap_or(1.27),
                        hide: child(pin, "effects").is_some_and(is_hidden),
                    })
                })
                .collect();
            Some(Sheet {
                at,
                size,
                stroke: parse_stroke(sheet),
                properties,
                pins,
            })
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
                .map(|property| parse_property(property))
                .collect();

            Some(Inst {
                lib_id,
                at,
                mirror_x,
                mirror_y,
                unit,
                body_style,
                properties,
                exclude_from_sim: token_bool(sym, "exclude_from_sim"),
                in_bom: token_bool(sym, "in_bom"),
                on_board: token_bool(sym, "on_board"),
                in_pos_files: token_bool(sym, "in_pos_files"),
                dnp: token_bool(sym, "dnp"),
            })
        })
        .collect()
}

fn property_value_in_node<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
    children(node, "property").into_iter().find_map(|property| {
        let mut iter = list_items(property);
        let _ = iter.next();
        let property_key = iter.next()?.as_str()?;
        if property_key != key {
            return None;
        }
        iter.next()?.as_str()
    })
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_string)
}

fn library_metadata(root: &Value) -> HashMap<String, LibraryMetadata> {
    let mut properties = HashMap::new();
    let Some(lib_symbols) = child(root, "lib_symbols") else {
        return properties;
    };
    for symbol in children(lib_symbols, "symbol") {
        let mut iter = list_items(symbol);
        let _ = iter.next();
        let Some(name) = iter.next().and_then(as_str) else {
            continue;
        };
        properties.insert(
            name,
            LibraryMetadata {
                description: non_empty(property_value_in_node(symbol, "Description")),
                datasheet: non_empty(property_value_in_node(symbol, "Datasheet")),
            },
        );
    }
    properties
}

fn instance_pins_for_metadata<'a>(inst: &Inst, libs: &'a LibMap) -> Vec<&'a Pin> {
    let mut seen = HashSet::new();
    selected_sub_symbols(inst, libs)
        .into_iter()
        .flat_map(|sub| sub.pins.iter())
        .filter(|pin| seen.insert(pin.number.clone()))
        .collect()
}

/// Return exactly the KiCad fragments selected by an instance.
///
/// Unit 0 is shared by every unit and body style 0 is shared by every alternate
/// representation. Other numbered units/styles are alternatives and must never
/// leak into the rendered instance.
fn selected_sub_symbols<'a>(inst: &Inst, libs: &'a LibMap) -> Vec<&'a SubSymbol> {
    let Some(units) = libs.get(&inst.lib_id) else {
        return Vec::new();
    };
    let mut sub_symbols = Vec::new();
    let mut seen = HashSet::new();
    for key in [
        (inst.unit, inst.body_style),
        (inst.unit, 0),
        (0, inst.body_style),
        (0, 0),
    ] {
        if !seen.insert(key) {
            continue;
        }
        if let Some(fragment) = units.get(&key.0).and_then(|styles| styles.get(&key.1)) {
            sub_symbols.push(fragment);
        }
    }
    sub_symbols
}

/// 聚合同一参考标号下的全部多单元符号定义。
fn extract_component_metadata(root: &Value, libs: &LibMap) -> ComponentMetadataMap {
    let library_metadata = library_metadata(root);
    let mut metadata = ComponentMetadataMap::new();

    for inst in extract_instances(root) {
        let Some(reference) = property_value(&inst, "Reference") else {
            continue;
        };
        let library = library_metadata.get(&inst.lib_id);
        let entry = metadata.entry(reference.to_string()).or_default();
        if entry.value.is_none() {
            entry.value = non_empty(property_value(&inst, "Value"));
        }
        if entry.footprint.is_none() {
            entry.footprint = non_empty(property_value(&inst, "Footprint"));
        }
        if entry.description.is_none() {
            entry.description = non_empty(property_value(&inst, "Description"))
                .or_else(|| library.and_then(|metadata| metadata.description.clone()));
        }
        if entry.datasheet.is_none() {
            entry.datasheet = non_empty(property_value(&inst, "Datasheet"))
                .or_else(|| library.and_then(|metadata| metadata.datasheet.clone()));
        }
        if entry.properties.is_empty() {
            entry.properties = inst
                .properties
                .iter()
                .filter(|property| !property.value.is_empty())
                .map(|property| ComponentProperty {
                    name: property.key.clone(),
                    value: property.value.clone(),
                    hidden: property.hide,
                })
                .collect();
        }
        entry.exclude_from_sim = entry.exclude_from_sim.or(inst.exclude_from_sim);
        entry.in_bom = entry.in_bom.or(inst.in_bom);
        entry.on_board = entry.on_board.or(inst.on_board);
        entry.in_pos_files = entry.in_pos_files.or(inst.in_pos_files);
        entry.dnp = entry.dnp.or(inst.dnp);

        for pin in instance_pins_for_metadata(&inst, libs) {
            if pin.number.is_empty() {
                continue;
            }
            entry
                .pins
                .entry(pin.number.clone())
                .or_insert_with(|| PinMetadata {
                    name: non_empty(Some(pin.name.as_str())),
                    electrical_type: (!pin.electrical_type.is_empty())
                        .then(|| pin.electrical_type.clone()),
                    shape: (!pin.shape.is_empty()).then(|| pin.shape.clone()),
                    unit: Some(inst.unit),
                });
        }
    }

    metadata
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
) -> (f64, f64, f64, f64) {
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
        for fragment in selected_sub_symbols(inst, libs) {
            for graphic in &fragment.graphics {
                bbs.push(bbox_of_graphic(
                    graphic,
                    inst.at,
                    inst.mirror_x,
                    inst.mirror_y,
                ));
            }
            for pin in &fragment.pins {
                let rad = pin.at.2.to_radians();
                let tip = (
                    pin.at.0 + pin.length * rad.cos(),
                    pin.at.1 + pin.length * rad.sin(),
                );
                let start = transform(pin.at.0, pin.at.1, inst.at, inst.mirror_x, inst.mirror_y);
                let end = transform(tip.0, tip.1, inst.at, inst.mirror_x, inst.mirror_y);
                bbs.push((
                    start.0.min(end.0),
                    start.1.min(end.1),
                    start.0.max(end.0),
                    start.1.max(end.1),
                ));
            }
        }
    }

    if bbs.is_empty() {
        return (0.0, 0.0, 100.0, 100.0);
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = bbs.remove(0);
    for (a, b, c, d) in bbs {
        minx = minx.min(a);
        miny = miny.min(b);
        maxx = maxx.max(c);
        maxy = maxy.max(d);
    }
    (minx, miny, maxx, maxy)
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

fn fill_color(fill: Fill) -> &'static str {
    match fill {
        Fill::None => "none",
        Fill::Outline => COLOR_SYMBOL,
        Fill::Background => COLOR_SYMBOL_FILL,
    }
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

/// Convert KiCad's `~{text}` overbar markup into safe SVG tspans.
///
/// Every literal segment is XML-escaped before insertion. Balanced braces are
/// tracked so nested markup or text containing braces cannot terminate a span
/// early; malformed markup remains visible as literal source text.
fn render_kicad_text_markup(text: &str) -> String {
    let mut rendered = String::new();
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find("~{") {
        let start = cursor + relative_start;
        rendered.push_str(&escape_xml_text(&text[cursor..start]));

        let inner_start = start + 2;
        let mut depth = 1usize;
        let mut closing = None;
        for (relative, ch) in text[inner_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        closing = Some(inner_start + relative);
                        break;
                    }
                }
                _ => {}
            }
        }

        let Some(closing) = closing else {
            rendered.push_str(&escape_xml_text(&text[start..]));
            return rendered;
        };
        rendered.push_str(r#"<tspan text-decoration="overline">"#);
        rendered.push_str(&render_kicad_text_markup(&text[inner_start..closing]));
        rendered.push_str("</tspan>");
        cursor = closing + 1;
    }
    rendered.push_str(&escape_xml_text(&text[cursor..]));
    rendered
}

fn property_value<'a>(inst: &'a Inst, key: &str) -> Option<&'a str> {
    inst.properties
        .iter()
        .find(|property| property.key == key)
        .map(|property| property.value.as_str())
}

fn instance_pins<'a>(inst: &Inst, libs: &'a LibMap) -> Vec<&'a Pin> {
    let mut seen = HashSet::new();
    selected_sub_symbols(inst, libs)
        .into_iter()
        .flat_map(|sub| sub.pins.iter())
        .filter(|pin| seen.insert((pin.number.clone(), pin.at.0.to_bits(), pin.at.1.to_bits())))
        .collect()
}

fn point_on_segment(point: (f64, f64), start: (f64, f64), end: (f64, f64)) -> bool {
    let length_sq = (end.0 - start.0).powi(2) + (end.1 - start.1).powi(2);
    if length_sq <= 1e-10 {
        return (point.0 - start.0).powi(2) + (point.1 - start.1).powi(2) <= 1e-10;
    }
    let cross = (point.0 - start.0) * (end.1 - start.1) - (point.1 - start.1) * (end.0 - start.0);
    if cross.abs() > 1e-5 {
        return false;
    }
    let dot = (point.0 - start.0) * (end.0 - start.0) + (point.1 - start.1) * (end.1 - start.1);
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
    schematic_texts: &[SchematicText],
    pcb_path: Option<&str>,
) -> Vec<Option<String>> {
    let mut pin_nets = HashMap::new();
    if let Some(circuit) = pcb_path
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| parse_pcb(&text).ok())
    {
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
    for label in schematic_texts.iter().filter(|text| {
        matches!(
            text.kind,
            SchematicTextKind::LocalLabel
                | SchematicTextKind::GlobalLabel
                | SchematicTextKind::HierarchicalLabel
        )
    }) {
        let point = (label.at.0, label.at.1);
        for (index, wire) in wires.iter().enumerate() {
            if point_on_wire(point, wire) {
                let group = root(&mut parents, index);
                component_nets
                    .entry(group)
                    .or_insert_with(|| label.text.clone());
            }
        }
    }
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
        Graphic::Polyline { pts, stroke, fill } => {
            let svg_pts: Vec<(f64, f64)> = pts
                .iter()
                .map(|&(x, y)| {
                    let (gx, gy) = transform(x, y, inst.at, inst.mirror_x, inst.mirror_y);
                    to_svg(gx, gy, ox, oy)
                })
                .collect();
            svg.push_str(&format!(
                r##"<polyline points="{}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                fmt_pts(&svg_pts),
                fill_color(*fill),
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
            svg.push_str(&format!(
                r##"<polygon points="{}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                fmt_pts(&svg_pts),
                fill_color(*fill),
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
            svg.push_str(&format!(
                r##"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                sx,
                sy,
                radius * SCALE,
                fill_color(*fill),
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
            svg.push_str(&format!(
                r##"<path d="M {:.2},{:.2} A {:.2},{:.2} 0 {},{} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
                sx, sy, r * SCALE, r * SCALE, large_arc, sweep, ex, ey, fill_color(*fill),
                COLOR_SYMBOL, stroke_w(*stroke)
            ));
        }
    }
}

fn render_pin_labels(svg: &mut String, pin: &Pin, start: (f64, f64), end: (f64, f64)) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = dx.hypot(dy);
    if length <= f64::EPSILON {
        return;
    }
    let direction = (dx / length, dy / length);

    if pin.name_text.visible && !pin.name.is_empty() {
        let font_size = pin.name_text.font_size * SCALE;
        let spacing = pin.name_offset * SCALE + font_size * 0.15;
        let x = end.0 + direction.0 * spacing;
        let y = end.1 + direction.1 * spacing;
        let vertical = direction.1.abs() > direction.0.abs();
        let text_anchor = if !vertical {
            if direction.0 >= 0.0 {
                "start"
            } else {
                "end"
            }
        } else {
            "start"
        };
        let rotation = if vertical {
            format!(
                r#" transform="rotate({:.2} {:.2} {:.2})""#,
                direction.1.atan2(direction.0).to_degrees(),
                x,
                y
            )
        } else {
            String::new()
        };
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.2}" fill="{}" text-anchor="{}" dominant-baseline="central"{} class="sch-pin-name">{}</text>"##,
            x,
            y,
            font_size,
            COLOR_PIN,
            text_anchor,
            rotation,
            render_kicad_text_markup(&pin.name)
        ));
    }

    if pin.number_text.visible && !pin.number.is_empty() {
        let font_size = pin.number_text.font_size * SCALE;
        let normal = (direction.1, -direction.0);
        let x = (start.0 + end.0) / 2.0 + normal.0 * font_size * 0.65;
        let y = (start.1 + end.1) / 2.0 + normal.1 * font_size * 0.65;
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.2}" fill="{}" text-anchor="middle" dominant-baseline="central" class="sch-pin-number">{}</text>"##,
            x,
            y,
            font_size,
            COLOR_PIN,
            render_kicad_text_markup(&pin.number)
        ));
    }
}

fn render_pin(svg: &mut String, pin: &Pin, start: (f64, f64), end: (f64, f64)) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = dx.hypot(dy);
    if length <= f64::EPSILON {
        return;
    }
    let direction = (dx / length, dy / length);
    let normal = (-direction.1, direction.0);
    let size = (pin.name_text.font_size.max(pin.number_text.font_size) * SCALE * 0.5).max(3.0);
    let point = |along: f64, across: f64| {
        (
            end.0 + direction.0 * along + normal.0 * across,
            end.1 + direction.1 * along + normal.1 * across,
        )
    };

    svg.push_str(&format!(
        r#"<g class="sch-pin" data-pin-shape="{}">"#,
        escape_xml_text(&pin.shape)
    ));
    svg.push_str(&format!(
        r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
        start.0, start.1, end.0, end.1, COLOR_PIN
    ));
    if pin.electrical_type == "no_connect" {
        render_cross(svg, "sch-pin-no-connect", start, size, COLOR_PIN);
    }

    let render_clock = |svg: &mut String| {
        let apex = point(0.0, 0.0);
        let upper = point(-size, size);
        let lower = point(-size, -size);
        svg.push_str(&format!(
            r##"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="1"/>"##,
            upper.0, upper.1, apex.0, apex.1, lower.0, lower.1, COLOR_PIN
        ));
    };
    let render_bubble = |svg: &mut String| {
        let center = point(-size, 0.0);
        svg.push_str(&format!(
            r##"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="1"/>"##,
            center.0, center.1, size, COLOR_BACKGROUND, COLOR_PIN
        ));
    };
    let render_low = |svg: &mut String| {
        let outside = point(-size * 1.5, size);
        let root = point(0.0, 0.0);
        svg.push_str(&format!(
            r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
            outside.0, outside.1, root.0, root.1, COLOR_PIN
        ));
    };

    match pin.shape.as_str() {
        "inverted" => render_bubble(svg),
        "clock" => render_clock(svg),
        "inverted_clock" => {
            render_clock(svg);
            render_bubble(svg);
        }
        "input_low" | "output_low" => render_low(svg),
        "clock_low" => {
            render_clock(svg);
            render_low(svg);
        }
        "edge_clock_high" => {
            let apex = point(-size, 0.0);
            let upper = point(0.0, size);
            let lower = point(0.0, -size);
            svg.push_str(&format!(
                r##"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="1"/>"##,
                upper.0, upper.1, apex.0, apex.1, lower.0, lower.1, COLOR_PIN
            ));
        }
        "non_logic" => {
            let center = point(-size, 0.0);
            for sign in [-1.0, 1.0] {
                let first = (
                    center.0 + direction.0 * size + normal.0 * size * sign,
                    center.1 + direction.1 * size + normal.1 * size * sign,
                );
                let second = (
                    center.0 - direction.0 * size - normal.0 * size * sign,
                    center.1 - direction.1 * size - normal.1 * size * sign,
                );
                svg.push_str(&format!(
                    r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
                    first.0, first.1, second.0, second.1, COLOR_PIN
                ));
            }
        }
        _ => {}
    }
    svg.push_str("</g>");
    render_pin_labels(svg, pin, start, end);
}

fn render_instance(svg: &mut String, inst: &Inst, libs: &LibMap, ox: f64, oy: f64) {
    if !libs.contains_key(&inst.lib_id) {
        return;
    }

    let body_graphics: Vec<&Graphic> = selected_sub_symbols(inst, libs)
        .into_iter()
        .flat_map(|fragment| fragment.graphics.iter())
        .collect();

    // 引脚: 合并实例单元与 KiCad unit 0 中的共享引脚，再按 (number, at) 去重。
    let pins = instance_pins(inst, libs);

    // SVG 分组本身没有可点击面积。先绘制覆盖本体的透明矩形，让电阻内部、
    // 三极管线条之间等空白区域也能选中元器件。不要把伸出的引脚纳入矩形，
    // 否则矩形会挡住引脚附近的导线命中区域。
    if property_value(inst, "Reference").is_some() {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        let mut include_bounds = |(min_x, min_y, max_x, max_y)| {
            bounds = Some(match bounds {
                Some((left, top, right, bottom)) => (
                    left.min(min_x),
                    top.min(min_y),
                    right.max(max_x),
                    bottom.max(max_y),
                ),
                None => (min_x, min_y, max_x, max_y),
            });
        };

        for graphic in &body_graphics {
            include_bounds(bbox_of_graphic(
                graphic,
                inst.at,
                inst.mirror_x,
                inst.mirror_y,
            ));
        }

        // 极少数符号没有本体图形；这时退回到引脚范围，避免它完全无法点击。
        if body_graphics.is_empty() {
            for pin in &pins {
                let rad = pin.at.2 * std::f64::consts::PI / 180.0;
                let tip = (
                    pin.at.0 + pin.length * rad.cos(),
                    pin.at.1 + pin.length * rad.sin(),
                );
                let start = transform(pin.at.0, pin.at.1, inst.at, inst.mirror_x, inst.mirror_y);
                let end = transform(tip.0, tip.1, inst.at, inst.mirror_x, inst.mirror_y);
                include_bounds((
                    start.0.min(end.0),
                    start.1.min(end.1),
                    start.0.max(end.0),
                    start.1.max(end.1),
                ));
            }
        }

        if let Some((min_x, min_y, max_x, max_y)) = bounds {
            let (left, top) = to_svg(min_x, min_y, ox, oy);
            let (right, bottom) = to_svg(max_x, max_y, ox, oy);
            let center_x = (left + right) / 2.0;
            let center_y = (top + bottom) / 2.0;
            let width = (right - left).abs() + COMPONENT_HIT_PADDING * 2.0;
            let height = (bottom - top).abs() + COMPONENT_HIT_PADDING * 2.0;
            let width = width.max(COMPONENT_HIT_MIN_SIZE);
            let height = height.max(COMPONENT_HIT_MIN_SIZE);
            svg.push_str(&format!(
                r##"<rect class="sch-component-hit" x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="transparent" pointer-events="all"/>"##,
                center_x - width / 2.0,
                center_y - height / 2.0,
                width,
                height
            ));
        }
    }

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
        render_pin(&mut *svg, p, (sx1, sy1), (sx2, sy2));
    }

    // 文本标注
    for prop in &inst.properties {
        if prop.hide || prop.value.is_empty() {
            continue;
        }
        let (sx, sy) = to_svg(prop.at.0, prop.at.1, ox, oy);
        let displayed = if prop.show_name {
            format!("{}: {}", prop.key, prop.value)
        } else {
            prop.value.clone()
        };
        let value = render_kicad_text_markup(&displayed);
        let color = if prop.key == "Reference" {
            COLOR_REFERENCE
        } else {
            COLOR_VALUE
        };
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.2}" fill="{}" transform="rotate({:.2} {:.2} {:.2})">{}</text>"##,
            sx,
            sy,
            prop.font_size * SCALE,
            color,
            -prop.at.2,
            sx,
            sy,
            value
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

fn render_cross(svg: &mut String, class_name: &str, center: (f64, f64), radius: f64, color: &str) {
    svg.push_str(&format!(r#"<g class="{}">"#, class_name));
    for sign in [-1.0, 1.0] {
        svg.push_str(&format!(
            r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1.5"/>"##,
            center.0 - radius,
            center.1 - radius * sign,
            center.0 + radius,
            center.1 + radius * sign,
            color
        ));
    }
    svg.push_str("</g>");
}

fn render_stroke_path(
    svg: &mut String,
    path: &StrokePath,
    class_name: &str,
    color: &str,
    ox: f64,
    oy: f64,
) {
    for segment in path.pts.windows(2) {
        let start = to_svg(segment[0].0, segment[0].1, ox, oy);
        let end = to_svg(segment[1].0, segment[1].1, ox, oy);
        svg.push_str(&format!(
            r##"<line class="{}" x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="{:.2}"/>"##,
            class_name,
            start.0,
            start.1,
            end.0,
            end.1,
            color,
            stroke_w(path.stroke)
        ));
    }
}

fn render_schematic_text(svg: &mut String, text: &SchematicText, ox: f64, oy: f64) {
    if text.hide {
        return;
    }
    let (x, y) = to_svg(text.at.0, text.at.1, ox, oy);
    let class_name = match text.kind {
        SchematicTextKind::Text => "sch-text",
        SchematicTextKind::LocalLabel => "sch-local-label",
        SchematicTextKind::GlobalLabel => "sch-global-label",
        SchematicTextKind::HierarchicalLabel => "sch-hierarchical-label",
    };
    let shape = text.shape.as_deref().unwrap_or("");
    let angle = -text.at.2;
    svg.push_str(&format!(
        r#"<g class="{}" data-shape="{}" transform="rotate({:.2} {:.2} {:.2})">"#,
        class_name,
        escape_xml_text(shape),
        angle,
        x,
        y
    ));
    if text.kind != SchematicTextKind::Text && text.kind != SchematicTextKind::LocalLabel {
        let direction = match shape {
            "output" => -1.0,
            _ => 1.0,
        };
        let points = [
            (x, y),
            (x + direction * 8.0, y - 6.0),
            (x + direction * 8.0, y + 6.0),
        ];
        svg.push_str(&format!(
            r##"<polygon points="{}" fill="none" stroke="{}" stroke-width="1"/>"##,
            fmt_pts(&points),
            COLOR_LABEL
        ));
    }
    svg.push_str(&format!(
        r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.2}" fill="{}" dominant-baseline="central">{}</text>"##,
        x + 10.0,
        y,
        text.font_size * SCALE,
        COLOR_LABEL,
        render_kicad_text_markup(&text.text)
    ));
    svg.push_str("</g>");
}

fn render_sheet(svg: &mut String, sheet: &Sheet, ox: f64, oy: f64) {
    let start = to_svg(sheet.at.0, sheet.at.1, ox, oy);
    let end = to_svg(sheet.at.0 + sheet.size.0, sheet.at.1 + sheet.size.1, ox, oy);
    svg.push_str(r#"<g class="sch-sheet">"#);
    svg.push_str(&format!(
        r##"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="{:.2}"/>"##,
        start.0.min(end.0),
        start.1.min(end.1),
        (end.0 - start.0).abs(),
        (end.1 - start.1).abs(),
        COLOR_SYMBOL_FILL,
        COLOR_SHEET,
        stroke_w(sheet.stroke)
    ));
    for property in &sheet.properties {
        if property.hide || property.value.is_empty() {
            continue;
        }
        let (x, y) = to_svg(property.at.0, property.at.1, ox, oy);
        let displayed = if property.show_name {
            format!("{}: {}", property.key, property.value)
        } else {
            property.value.clone()
        };
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.2}" fill="{}">{}</text>"##,
            x,
            y,
            property.font_size * SCALE,
            COLOR_SHEET,
            render_kicad_text_markup(&displayed)
        ));
    }
    for pin in &sheet.pins {
        if pin.hide {
            continue;
        }
        let start = to_svg(pin.at.0, pin.at.1, ox, oy);
        let rad = -pin.at.2.to_radians();
        let end = (start.0 + 12.0 * rad.cos(), start.1 + 12.0 * rad.sin());
        svg.push_str(&format!(
            r#"<g class="sch-sheet-pin" data-pin-type="{}">"#,
            escape_xml_text(&pin.electrical_type)
        ));
        svg.push_str(&format!(
            r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1"/>"##,
            start.0, start.1, end.0, end.1, COLOR_SHEET
        ));
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.2}" fill="{}" dominant-baseline="central">{}</text>"##,
            end.0,
            end.1,
            pin.font_size * SCALE,
            COLOR_SHEET,
            render_kicad_text_markup(&pin.name)
        ));
        svg.push_str("</g>");
    }
    svg.push_str("</g>");
}

// ─────────────────────────── 入口 ───────────────────────────

pub fn render(path: &str) -> Result<String, String> {
    render_with_pcb(path, None)
}

pub fn render_with_pcb(path: &str, pcb_path: Option<&str>) -> Result<String, String> {
    render_with_pcb_and_metadata(path, pcb_path)
        .map(|(svg, _)| svg)
        .map_err(|error| error.to_string())
}

pub(crate) fn render_with_pcb_and_metadata(
    path: &str,
    pcb_path: Option<&str>,
) -> Result<(String, ComponentMetadataMap), RenderError> {
    let text = fs::read_to_string(path).map_err(|error| RenderError::Read(error.to_string()))?;
    let root = lexpr::from_str(&text).map_err(|error| RenderError::Parse(error.to_string()))?;

    let libs = extract_lib_symbols(&root)?;
    let component_metadata = extract_component_metadata(&root, &libs);
    let wires = extract_wires(&root);
    let buses = extract_stroke_paths(&root, "bus");
    let bus_entries = extract_bus_entries(&root);
    let no_connects = extract_no_connects(&root);
    let schematic_texts = extract_schematic_texts(&root);
    let sheets = extract_sheets(&root);
    let junctions = extract_junctions(&root);
    let instances = extract_instances(&root);
    let wire_nets = wire_net_names(&wires, &instances, &libs, &schematic_texts, pcb_path);

    let (mut min_x, mut min_y, mut max_x, mut max_y) =
        compute_bbox(&wires, &junctions, &instances, &libs);
    let mut include_point = |(x, y): (f64, f64)| {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    };
    for bus in &buses {
        for &point in &bus.pts {
            include_point(point);
        }
    }
    for entry in &bus_entries {
        include_point(entry.at);
        include_point((entry.at.0 + entry.size.0, entry.at.1 + entry.size.1));
    }
    for &point in &no_connects {
        include_point(point);
    }
    for text in &schematic_texts {
        include_point((text.at.0, text.at.1));
    }
    for sheet in &sheets {
        include_point(sheet.at);
        include_point((sheet.at.0 + sheet.size.0, sheet.at.1 + sheet.size.1));
        for pin in &sheet.pins {
            include_point((pin.at.0, pin.at.1));
        }
    }
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

    for bus in &buses {
        render_stroke_path(&mut svg, bus, "sch-bus-line", COLOR_BUS, ox, oy);
    }
    for entry in &bus_entries {
        render_stroke_path(
            &mut svg,
            &StrokePath {
                pts: vec![
                    entry.at,
                    (entry.at.0 + entry.size.0, entry.at.1 + entry.size.1),
                ],
                stroke: entry.stroke,
            },
            "sch-bus-entry",
            COLOR_BUS,
            ox,
            oy,
        );
    }

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

    for &(x, y) in &no_connects {
        render_cross(
            &mut svg,
            "sch-no-connect",
            to_svg(x, y, ox, oy),
            4.0,
            COLOR_PIN,
        );
    }

    for text in &schematic_texts {
        render_schematic_text(&mut svg, text, ox, oy);
    }

    for sheet in &sheets {
        render_sheet(&mut svg, sheet, ox, oy);
    }

    // 符号实例
    for inst in &instances {
        if let Some(reference) = property_value(inst, "Reference") {
            let mut attributes = format!(
                r#" class="sch-component" data-component="{}""#,
                escape_xml_text(reference)
            );
            for (name, value) in [
                ("data-dnp", inst.dnp),
                ("data-in-bom", inst.in_bom),
                ("data-on-board", inst.on_board),
                ("data-in-pos-files", inst.in_pos_files),
                ("data-exclude-from-sim", inst.exclude_from_sim),
            ] {
                if let Some(value) = value {
                    attributes.push_str(&format!(
                        r#" {}="{}""#,
                        name,
                        if value { "yes" } else { "no" }
                    ));
                }
            }
            svg.push_str(&format!("<g{attributes}>"));
        } else {
            svg.push_str("<g>");
        }
        render_instance(&mut svg, inst, &libs, ox, oy);
        svg.push_str("</g>");
    }

    svg.push_str("</svg>");
    Ok((svg, component_metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn example_schematic(relative_path: &str) -> Value {
        let path = format!(
            "{}/../examples/folders/{relative_path}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(path).unwrap();
        lexpr::from_str(&text).unwrap()
    }

    fn render_fixture(name: &str, text: &str) -> String {
        let path: PathBuf = std::env::temp_dir().join(format!(
            "knead-net-{name}-{}-{}.kicad_sch",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, text).unwrap();
        let rendered = render(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        rendered.unwrap()
    }

    #[test]
    fn missing_pin_names_section_uses_kicad_default_offset() {
        let root = lexpr::from_str(
            r#"(kicad_sch
                (lib_symbols
                    (symbol "Test:Part"
                        (symbol "Part_1_1"
                            (pin input line
                                (at 0 0 0)
                                (length 2.54)
                                (name "IN" (effects (font (size 1.27 1.27))))
                                (number "1" (effects (font (size 1.27 1.27)))))))))"#,
        )
        .unwrap();
        let libs = extract_lib_symbols(&root).unwrap();
        let pin = &libs["Test:Part"][&1][&1].pins[0];

        assert!((pin.name_offset - 0.508).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_nested_fill_types_and_pin_visibility_flags() {
        let root = lexpr::from_str(
            r#"(kicad_sch
                (lib_symbols
                    (symbol "Test:Part"
                        (pin_numbers hide)
                        (pin_names (offset 0.75) (hide no))
                        (symbol "Part_0_1"
                            (polyline (pts (xy 0 0) (xy 1 0) (xy 1 1))
                                (stroke (width 0.1)) (fill (type outline)))
                            (rectangle (start 0 0) (end 1 1)
                                (stroke (width 0.1)) (fill (type background)))
                            (pin input line (at 0 0 0) (length 1)
                                (name "IN") (number "1"))))))"#,
        )
        .unwrap();
        let libs = extract_lib_symbols(&root).unwrap();
        let fragment = &libs["Test:Part"][&0][&1];

        assert!(matches!(
            fragment.graphics[0],
            Graphic::Polyline {
                fill: Fill::Outline,
                ..
            }
        ));
        assert!(matches!(
            fragment.graphics[1],
            Graphic::Rectangle {
                fill: Fill::Background,
                ..
            }
        ));
        assert!(fragment.pins[0].name_text.visible);
        assert!(!fragment.pins[0].number_text.visible);
        assert!((fragment.pins[0].name_offset - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_length_wire_segment_only_contains_its_own_point() {
        assert!(point_on_segment((1.0, 2.0), (1.0, 2.0), (1.0, 2.0)));
        assert!(!point_on_segment((8.0, 9.0), (1.0, 2.0), (1.0, 2.0)));
    }

    #[test]
    fn renders_no_connect_markers_from_existing_ne555_example() {
        let svg = render(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/folders/NE555+CD4017/NE555+CD4017.kicad_sch"
        ))
        .unwrap();

        assert_eq!(svg.matches(r#"class="sch-no-connect""#).count(), 2);
    }

    #[test]
    fn renders_kicad_overbar_markup_without_raw_delimiters() {
        let root = example_schematic("NE555+CD4017/NE555+CD4017.kicad_sch");
        let libs = extract_lib_symbols(&root).unwrap();
        let instances = extract_instances(&root);
        let ne555 = instances
            .iter()
            .find(|inst| property_value(inst, "Reference") == Some("U1"))
            .unwrap();
        let mut svg = String::new();

        render_instance(&mut svg, ne555, &libs, 0.0, 0.0);

        assert!(!svg.contains("~{RST}"));
        assert!(svg.contains(r#"<tspan text-decoration="overline">RST</tspan>"#));
    }

    #[test]
    fn overbar_markup_is_partial_malformed_safe_and_xml_escaped() {
        assert_eq!(
            render_kicad_text_markup("~{FO}O"),
            r#"<tspan text-decoration="overline">FO</tspan>O"#
        );
        assert_eq!(render_kicad_text_markup("~{RST"), "~{RST");
        assert_eq!(render_kicad_text_markup("A ~ B"), "A ~ B");
        assert_eq!(
            render_kicad_text_markup("~{<script>&}"),
            r#"<tspan text-decoration="overline">&lt;script&gt;&amp;</tspan>"#
        );
    }

    #[test]
    fn renders_library_no_connect_pin_at_its_connection_point() {
        let svg = render(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/folders/lm741/lm741.kicad_sch"
        ))
        .unwrap();

        assert!(svg.contains(r#"class="sch-pin-no-connect""#));
    }

    #[test]
    fn renders_every_kicad_pin_graphic_shape_from_pin_data() {
        let fixture = r#"(kicad_sch
            (lib_symbols
                (symbol "Test:Shapes"
                    (pin_names (hide yes))
                    (pin_numbers (hide yes))
                    (symbol "Shapes_1_1"
                        (pin input line (at 0 0 0) (length 2.54) (name "") (number "1"))
                        (pin input inverted (at 0 2.54 0) (length 2.54) (name "") (number "2"))
                        (pin input clock (at 0 5.08 0) (length 2.54) (name "") (number "3"))
                        (pin input inverted_clock (at 0 7.62 0) (length 2.54) (name "") (number "4"))
                        (pin input input_low (at 0 10.16 0) (length 2.54) (name "") (number "5"))
                        (pin input clock_low (at 0 12.7 0) (length 2.54) (name "") (number "6"))
                        (pin output output_low (at 0 15.24 0) (length 2.54) (name "") (number "7"))
                        (pin input edge_clock_high (at 0 17.78 0) (length 2.54) (name "") (number "8"))
                        (pin input non_logic (at 0 20.32 0) (length 2.54) (name "") (number "9")))))
            (symbol
                (lib_id "Test:Shapes")
                (at 20 20 0)
                (unit 1)
                (body_style 1)
                (property "Reference" "U1" (at 20 15 0))
                (property "Value" "Shapes" (at 20 17 0))))"#;
        let svg = render_fixture("pin-shapes", fixture);

        for shape in [
            "line",
            "inverted",
            "clock",
            "inverted_clock",
            "input_low",
            "clock_low",
            "output_low",
            "edge_clock_high",
            "non_logic",
        ] {
            assert!(
                svg.contains(&format!(r#"data-pin-shape="{shape}""#)),
                "missing rendered shape {shape}"
            );
        }
    }

    #[test]
    fn renders_bus_labels_hierarchy_text_and_visible_custom_properties() {
        let fixture = r#"(kicad_sch
            (lib_symbols
                (symbol "Test:Part"
                    (symbol "Part_1_1"
                        (rectangle (start -2.54 2.54) (end 2.54 -2.54)
                            (stroke (width 0.254) (type default)) (fill (type background))))))
            (bus (pts (xy 10 10) (xy 40 10))
                (stroke (width 0) (type default)))
            (bus_entry (at 15 10) (size 2.54 2.54)
                (stroke (width 0) (type default)))
            (label "D0" (at 17.54 12.54 0) (effects (font (size 1.27 1.27))))
            (global_label "ENABLE" (shape input) (at 10 20 0)
                (effects (font (size 1.27 1.27))))
            (hierarchical_label "RESULT" (shape output) (at 40 20 180)
                (effects (font (size 1.27 1.27))))
            (text "~{RESET}" (at 25 25 0) (effects (font (size 1.27 1.27))))
            (sheet
                (at 15 30)
                (size 20 12)
                (stroke (width 0.254) (type default))
                (fill (color 0 0 0 0))
                (property "Sheetname" "Controller" (at 15 29 0)
                    (effects (font (size 1.27 1.27))))
                (property "Sheetfile" "controller.kicad_sch" (at 15 43 0)
                    (effects (font (size 1.27 1.27))))
                (pin "START" input (at 15 35 0)
                    (effects (font (size 1.27 1.27)))))
            (symbol
                (lib_id "Test:Part")
                (at 50 30 0)
                (unit 1)
                (body_style 1)
                (exclude_from_sim yes)
                (in_bom no)
                (on_board no)
                (in_pos_files no)
                (dnp yes)
                (property "Reference" "U1" (at 50 25 0))
                (property "Value" "Part" (at 50 27 0))
                (property "Manufacturer" "Acme" (at 50 35 0)
                    (effects (font (size 1.27 1.27))))))"#;
        let svg = render_fixture("schematic-features", fixture);

        for class in [
            "sch-bus-line",
            "sch-bus-entry",
            "sch-local-label",
            "sch-global-label",
            "sch-hierarchical-label",
            "sch-sheet",
            "sch-sheet-pin",
            "sch-text",
        ] {
            assert!(
                svg.contains(&format!(r#"class="{class}""#)),
                "missing {class}"
            );
        }
        assert!(svg.contains(">Acme</text>"));
        assert!(svg.contains(r#"data-dnp="yes""#));
        assert!(svg.contains(r#"data-in-bom="no""#));
        assert!(svg.contains(r#"data-on-board="no""#));
        assert!(svg.contains(r#"data-exclude-from-sim="yes""#));

        let root = lexpr::from_str(fixture).unwrap();
        let libs = extract_lib_symbols(&root).unwrap();
        let metadata = extract_component_metadata(&root, &libs);
        let part = &metadata["U1"];
        assert_eq!(part.dnp, Some(true));
        assert_eq!(part.in_bom, Some(false));
        assert_eq!(part.on_board, Some(false));
        assert_eq!(part.in_pos_files, Some(false));
        assert_eq!(part.exclude_from_sim, Some(true));
        assert!(part
            .properties
            .iter()
            .any(|property| property.name == "Manufacturer" && property.value == "Acme"));
    }

    #[test]
    fn schematic_labels_name_wires_without_a_pcb_file() {
        let fixture = r#"(kicad_sch
            (lib_symbols)
            (wire (pts (xy 10 10) (xy 20 10)) (stroke (width 0) (type default)))
            (wire (pts (xy 30 10) (xy 40 10)) (stroke (width 0) (type default)))
            (label "SIG" (at 10 10 0) (effects (font (size 1.27 1.27))))
            (label "SIG" (at 30 10 0) (effects (font (size 1.27 1.27)))))"#;
        let svg = render_fixture("labeled-wires", fixture);

        assert_eq!(
            svg.matches(r#"class="sch-net-line" data-net="SIG""#)
                .count(),
            2
        );
    }

    #[test]
    fn extracts_bjt_circle_radius_from_kicad_radius_node() {
        let root = example_schematic("h-bridge/h-bridge.kicad_sch");
        let libs = extract_lib_symbols(&root).unwrap();
        let circle = libs["Simulation_SPICE:NPN"][&0][&1]
            .graphics
            .iter()
            .find_map(|graphic| match graphic {
                Graphic::Circle { radius, .. } => Some(*radius),
                _ => None,
            })
            .expect("NPN symbol should contain the circle stored by KiCad");

        assert!((circle - 2.8194).abs() < f64::EPSILON);
    }

    #[test]
    fn ne555_instance_includes_shared_power_pins_from_unit_zero() {
        let root = example_schematic("NE555+CD4017/NE555+CD4017.kicad_sch");
        let libs = extract_lib_symbols(&root).unwrap();
        let instances = extract_instances(&root);
        let ne555 = instances
            .iter()
            .find(|inst| property_value(inst, "Reference") == Some("U1"))
            .unwrap();
        let pins = instance_pins(ne555, &libs);
        let numbers: HashSet<&str> = pins.iter().map(|pin| pin.number.as_str()).collect();

        assert_eq!(
            numbers,
            HashSet::from(["1", "2", "3", "4", "5", "6", "7", "8"])
        );

        let ground = pins.iter().find(|pin| pin.number == "1").unwrap();
        let connection = transform(
            ground.at.0,
            ground.at.1,
            ne555.at,
            ne555.mirror_x,
            ne555.mirror_y,
        );
        assert!((connection.0 - 129.54).abs() < 1e-9);
        assert!((connection.1 - 102.87).abs() < 1e-9);
        assert!(
            extract_wires(&root)
                .iter()
                .any(|wire| point_on_wire(connection, wire)),
            "the KiCad GND pin connection point should touch the wire below U1"
        );

        let metadata = extract_component_metadata(&root, &libs);
        let ne555_metadata = &metadata["U1"];
        assert_eq!(ne555_metadata.pins.len(), 8);
        assert_eq!(ne555_metadata.pins["1"].name.as_deref(), Some("GND"));
        assert_eq!(ne555_metadata.pins["8"].name.as_deref(), Some("VCC"));
    }

    #[test]
    fn renders_pin_labels_using_kicad_text_and_visibility() {
        let root = example_schematic("NE555+CD4017/NE555+CD4017.kicad_sch");
        let libs = extract_lib_symbols(&root).unwrap();
        let instances = extract_instances(&root);
        let ne555 = instances
            .iter()
            .find(|inst| property_value(inst, "Reference") == Some("U1"))
            .unwrap();
        let mut svg = String::new();

        render_instance(&mut svg, ne555, &libs, 0.0, 0.0);

        assert!(svg.contains(r#"class="sch-pin-name">GND</text>"#));
        assert!(svg.contains(r#"class="sch-pin-name">TRIG</text>"#));
        assert!(svg.contains(r#"class="sch-pin-number">1</text>"#));
        assert!(svg.contains(r#"class="sch-pin-number">8</text>"#));

        let root = example_schematic("h-bridge/h-bridge.kicad_sch");
        let libs = extract_lib_symbols(&root).unwrap();
        let instances = extract_instances(&root);
        let bjt = instances
            .iter()
            .find(|inst| property_value(inst, "Reference") == Some("Q1"))
            .unwrap();
        let mut svg = String::new();

        render_instance(&mut svg, bjt, &libs, 0.0, 0.0);

        assert!(svg.contains(r#"class="sch-pin-name">B</text>"#));
        assert!(!svg.contains(r#"class="sch-pin-number""#));
    }

    #[test]
    fn extracts_meaningful_multi_unit_pin_metadata_from_sn4hc00() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/folders/SNx4HC00/SNx4HC00.kicad_sch"
        ))
        .unwrap();
        let root = lexpr::from_str(&text).unwrap();
        let libs = extract_lib_symbols(&root).unwrap();
        let metadata = extract_component_metadata(&root, &libs);
        let u1 = metadata.get("U1").unwrap();

        assert_eq!(u1.value.as_deref(), Some("74HC00"));
        assert_eq!(u1.description.as_deref(), Some("quad 2-input NAND gate"));
        assert_eq!(
            u1.pins.get("1").unwrap().electrical_type.as_deref(),
            Some("input")
        );
        assert_eq!(
            u1.pins.get("3").unwrap().electrical_type.as_deref(),
            Some("output")
        );
        assert_eq!(u1.pins.get("3").unwrap().shape.as_deref(), Some("inverted"));
        assert_eq!(u1.pins.get("7").unwrap().name.as_deref(), Some("GND"));
        assert_eq!(u1.pins.get("14").unwrap().name.as_deref(), Some("VCC"));
        assert_eq!(u1.pins.get("7").unwrap().unit, Some(5));
        assert_eq!(
            u1.pins.get("14").unwrap().electrical_type.as_deref(),
            Some("power_in")
        );
    }

    #[test]
    fn preserves_raw_pin_fields_and_discards_only_empty_properties() {
        let root = lexpr::from_str(
            r#"(kicad_sch
                (lib_symbols
                    (symbol "Test:Part"
                        (property "Description" "")
                        (property "Datasheet" "")
                        (symbol "Part_0_1"
                            (pin input line
                                (at 0 0 0)
                                (length 2.54)
                                (name "~")
                                (number "1")))))
                (symbol
                    (lib_id "Test:Part")
                    (body_style 1)
                    (property "Reference" "U1")
                    (property "Value" "")
                    (property "Footprint" "")))"#,
        )
        .unwrap();
        let libs = extract_lib_symbols(&root).unwrap();
        let metadata = extract_component_metadata(&root, &libs);
        let part = metadata.get("U1").unwrap();
        let pin = part.pins.get("1").unwrap();

        assert_eq!(part.value, None);
        assert_eq!(part.footprint, None);
        assert_eq!(part.description, None);
        assert_eq!(part.datasheet, None);
        assert_eq!(pin.name.as_deref(), Some("~"));
        assert_eq!(pin.electrical_type.as_deref(), Some("input"));
        assert_eq!(pin.shape.as_deref(), Some("line"));
        assert_eq!(pin.unit, Some(0));
    }

    #[test]
    fn pin_metadata_uses_selected_and_shared_but_not_alternate_body_styles() {
        let root = lexpr::from_str(
            r#"(kicad_sch
                (lib_symbols
                    (symbol "Test:Styled"
                        (symbol "Styled_1_0"
                            (pin passive line (at 0 0 0) (length 2.54) (name "shared-1") (number "1"))
                            (pin passive line (at 0 0 0) (length 2.54) (name "shared-2") (number "2")))
                        (symbol "Styled_1_1"
                            (pin passive line (at 0 0 0) (length 2.54) (name "style-1-2") (number "2"))
                            (pin passive line (at 0 0 0) (length 2.54) (name "style-1-3") (number "3")))
                        (symbol "Styled_1_2"
                            (pin passive line (at 0 0 0) (length 2.54) (name "selected-1") (number "1")))))
                (symbol
                    (lib_id "Test:Styled")
                    (unit 1)
                    (body_style 2)
                    (property "Reference" "U1")))"#,
        )
        .unwrap();
        let libs = extract_lib_symbols(&root).unwrap();
        let metadata = extract_component_metadata(&root, &libs);
        let pins = &metadata.get("U1").unwrap().pins;

        assert_eq!(pins.get("1").unwrap().name.as_deref(), Some("selected-1"));
        assert_eq!(pins.get("2").unwrap().name.as_deref(), Some("shared-2"));
        assert!(!pins.contains_key("3"));
    }

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
            exclude_from_sim: None,
            in_bom: None,
            on_board: None,
            in_pos_files: None,
            dnp: None,
            properties: vec![
                Property {
                    key: "Reference".into(),
                    value: malicious.into(),
                    at: (0.0, 0.0, 0.0),
                    hide: false,
                    show_name: false,
                    font_size: 1.27,
                },
                Property {
                    key: "Value".into(),
                    value: "R1 > R2 & R3".into(),
                    at: (0.0, 1.0, 0.0),
                    hide: false,
                    show_name: false,
                    font_size: 1.27,
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
        assert!(svg.contains(r##"fill="#008484""##));
        assert!(svg.contains(r##"fill="#0000c2""##));
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
            exclude_from_sim: None,
            in_bom: None,
            on_board: None,
            in_pos_files: None,
            dnp: None,
        };
        let pin = Pin {
            at: (0.0, 0.0, 0.0),
            length: 2.54,
            name: "IN".into(),
            number: "1".into(),
            name_text: PinText::default(),
            number_text: PinText::default(),
            name_offset: 0.0,
            electrical_type: "input".into(),
            shape: "line".into(),
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
    fn referenced_component_hit_area_covers_body_but_not_extended_pins() {
        let inst = Inst {
            lib_id: "Device:R".into(),
            at: (0.0, 0.0, 0.0),
            mirror_x: false,
            mirror_y: false,
            unit: 1,
            body_style: 1,
            exclude_from_sim: None,
            in_bom: None,
            on_board: None,
            in_pos_files: None,
            dnp: None,
            properties: vec![Property {
                key: "Reference".into(),
                value: "R1".into(),
                at: (0.0, 0.0, 0.0),
                hide: false,
                show_name: false,
                font_size: 1.27,
            }],
        };
        let libs = HashMap::from([(
            "Device:R".into(),
            HashMap::from([(
                1,
                HashMap::from([(
                    1,
                    SubSymbol {
                        graphics: vec![Graphic::Rectangle {
                            start: (-1.0, -1.0),
                            end: (1.0, 1.0),
                            stroke: 0.0,
                            fill: Fill::None,
                        }],
                        pins: vec![Pin {
                            at: (-2.0, 0.0, 0.0),
                            length: 1.0,
                            name: "1".into(),
                            number: "1".into(),
                            name_text: PinText::default(),
                            number_text: PinText::default(),
                            name_offset: 0.0,
                            electrical_type: "passive".into(),
                            shape: "line".into(),
                        }],
                    },
                )]),
            )]),
        )]);
        let mut svg = String::new();

        render_instance(&mut svg, &inst, &libs, 0.0, 0.0);

        let hit_area = svg.find(r#"class="sch-component-hit""#).unwrap();
        let visible_graphic = svg.find("<polygon").unwrap();
        assert!(hit_area < visible_graphic);
        assert!(svg.contains(r#"fill="transparent" pointer-events="all""#));
        assert!(svg.contains(r#"width="32.00" height="32.00""#));
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
