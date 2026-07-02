//! KiCad `.kicad_mod` 封装文件解析器。

use super::sexp::{ParseError, Sexp, parse};
use crate::circuit::{Footprint, FootprintId, PhysicalPin, Position};

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

/// 一个 `.kicad_mod` 文件解析出来的"草稿" —— 还没分配 FootprintId
pub struct FootprintDraft {
    pub name: String,
    pub pins: Vec<PhysicalPin>,
}

/// 解析单个 .kicad_mod 文件
pub fn parse_one(text: &str) -> Result<FootprintDraft, ParseError> {
    let sexp = parse(text)?;

    let top = match sexp {
        Sexp::List(items) => items,
        _ => {
            return Err(ParseError {
                message: "expected list at top of .kicad_mod".into(),
            });
        }
    };

    if top.is_empty() {
        return Err(ParseError {
            message: "empty footprint file".into(),
        });
    }
    if !matches!(&top[0], Sexp::Atom(s) if s == "footprint") {
        return Err(ParseError {
            message: "expected (footprint ...) at top".into(),
        });
    }

    // 第二个元素是 footprint 的名字 (例如 "TO-92L_Inline")
    let name = match top.get(1) {
        Some(Sexp::Atom(s)) => s.clone(),
        _ => {
            return Err(ParseError {
                message: "missing footprint name".into(),
            });
        }
    };

    // 在剩下的元素里找所有 (pad "NUM" ... (at X Y) ...) 形式
    // 但首先排除 SMD: 面包板只能用直插 (through_hole) 元件
    for item in &top[2..] {
        if is_smd_attr(item) {
            panic!(
                "footprint '{}' 是 SMD, 面包板只能用直插 (through_hole) 元件",
                name
            );
        }
    }

    let mut pins = Vec::new();
    for item in &top[2..] {
        if let Some(pin) = extract_pad(item) {
            pins.push(pin);
        }
    }

    Ok(FootprintDraft { name, pins })
}

/// 判断一个 sexp 是不是 (attr smd)
fn is_smd_attr(sexp: &Sexp) -> bool {
    if let Sexp::List(items) = sexp
        && matches!(items.first(), Some(Sexp::Atom(s)) if s == "attr")
        && let Some(Sexp::Atom(s)) = items.get(1)
    {
        return s == "smd";
    }
    false
}

/// 解析多个 .kicad_mod 文件, 按顺序给每个 footprint 分配 FootprintId(0), (1), ...
pub fn parse_many<I, S>(texts: I) -> Result<Vec<Footprint>, ParseError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    texts
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let draft = parse_one(t.as_ref())?;
            Ok(Footprint {
                id: FootprintId(i),
                name: draft.name,
                pins: draft.pins,
            })
        })
        .collect()
}

/// 尝试从一个 sexp 里提取 (pad "NUM" ... (at X Y) ...)
fn extract_pad(sexp: &Sexp) -> Option<PhysicalPin> {
    let items = match sexp {
        Sexp::List(items) => items,
        _ => return None,
    };
    if !matches!(items.first(), Some(Sexp::Atom(s)) if s == "pad") {
        return None;
    }

    // pad 编号: 第二个元素
    let number = match items.get(1) {
        Some(Sexp::Atom(s)) => s.clone(),
        _ => return None,
    };

    // 找 (at X Y) 这一项 (可能在嵌套里面)
    let (x_mm, y_mm) = find_at(items)?;

    Some(PhysicalPin {
        name: number,
        offset: Position {
            x: mm_to_holes(x_mm),
            y: mm_to_holes(y_mm),
        },
    })
}

/// 在一个列表里找 (at X Y), 返回 (x, y)
fn find_at(items: &[Sexp]) -> Option<(f64, f64)> {
    for item in items {
        if let Sexp::List(sub) = item
            && let Some(Sexp::Atom(head)) = sub.first()
            && head == "at"
            && sub.len() >= 3
            && let (Some(x), Some(y)) = (parse_f64(&sub[1]), parse_f64(&sub[2]))
        {
            return Some((x, y));
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
