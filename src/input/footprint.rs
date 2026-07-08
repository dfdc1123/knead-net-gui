//! KiCad `.kicad_mod` 封装文件解析器。

use std::path::{Path, PathBuf};

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

    // 找 (at X Y) 这一项 (实际只看 pad list 的直接子项; 若 KiCad 把它套在更深的嵌套里, 会找不到)
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

// ── 在 KiCad 库里查找 footprint 文件 ─────────────────────────────

/// 把 "LIB:NAME" 形式的 footprint ref 拆成 `(LIB, NAME)`。
///
/// 例如 `"LED_THT:LED_D5.0mm" → ("LED_THT", "LED_D5.0mm")`。
/// 没有冒号时 lib 和 name 都用整个字符串 (KiCad 正常 .net 不会这样,
/// 但解析器层做个容错, 调用方按需 panic / 跳过)。
///
/// 用 `rsplit_once` 而不是 `split_once` 是为了和 `netlist::strip_library_prefix`
/// 的语义对齐 — 永远取最后一段当 name, 前面无论多复杂都当 lib prefix。
pub fn split_footprint_ref(footprint_ref: &str) -> (&str, &str) {
    match footprint_ref.rsplit_once(':') {
        Some((l, n)) => (l, n),
        None => (footprint_ref, footprint_ref),
    }
}

/// 给定一个 footprint ref, 在一组 KiCad 库根目录 + 一个 flat fallback 目录里找 `.kicad_mod` 文件。
///
/// **查找顺序** (返回第一个命中的):
/// 1. 对每个 `kicad_lib_path`, 尝试 `<kicad_lib_path>/<LIB>.pretty/<NAME>.kicad_mod`
/// 2. `<fallback_dir>/<NAME>.kicad_mod` (单个 flat 目录, 没有 `.pretty` 嵌套)
///
/// 都没有就返回 `None`。
///
/// 多个 `--kicad-lib` 路径按调用方传入的顺序搜索, 第一个命中即返回。
pub fn find_footprint_file(
    footprint_ref: &str,
    kicad_lib_paths: &[&Path],
    fallback_dir: &Path,
) -> Option<PathBuf> {
    let (lib, name) = split_footprint_ref(footprint_ref);
    for kicad_path in kicad_lib_paths {
        let p = kicad_path
            .join(format!("{lib}.pretty"))
            .join(format!("{name}.kicad_mod"));
        if p.is_file() {
            return Some(p);
        }
    }
    let p = fallback_dir.join(format!("{name}.kicad_mod"));
    if p.is_file() {
        return Some(p);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 在临时目录里建一组 (lib, footprint) → 文件的映射, 返回 (root, 单个文件路径) 给测试用。
    fn touch_pretty(root: &Path, lib: &str, name: &str) -> PathBuf {
        let p = root
            .join(format!("{lib}.pretty"))
            .join(format!("{name}.kicad_mod"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "(footprint \"test\")").unwrap();
        p
    }

    #[test]
    fn split_standard_led_ref() {
        assert_eq!(
            split_footprint_ref("LED_THT:LED_D5.0mm"),
            ("LED_THT", "LED_D5.0mm")
        );
    }

    #[test]
    fn split_uses_last_colon_segment_as_name() {
        // 跟 netlist::strip_library_prefix 对齐: rsplit_once, 永远取最后一段
        assert_eq!(
            split_footprint_ref("weird:lib:prefix:FP_NAME"),
            ("weird:lib:prefix", "FP_NAME")
        );
    }

    #[test]
    fn split_no_colon_returns_full_string_for_both() {
        let (l, n) = split_footprint_ref("nocolon");
        assert_eq!(l, "nocolon");
        assert_eq!(n, "nocolon");
    }

    #[test]
    fn find_in_kicad_lib_first_pretty() {
        let tmp = tempdir_in_target();
        let p = touch_pretty(&tmp, "LED_THT", "LED_D5.0mm");
        let found = find_footprint_file(
            "LED_THT:LED_D5.0mm",
            &[tmp.as_path()],
            Path::new("/nonexistent"),
        );
        assert_eq!(found, Some(p));
    }

    #[test]
    fn find_falls_back_to_flat_dir() {
        let tmp = tempdir_in_target();
        let p = tmp.join("LED_D5.0mm.kicad_mod");
        fs::write(&p, "(footprint \"test\")").unwrap();
        let found = find_footprint_file("LED_THT:LED_D5.0mm", &[], tmp.as_path());
        assert_eq!(found, Some(p));
    }

    #[test]
    fn find_kicad_lib_takes_precedence_over_fallback() {
        // kicad 库里有, fallback 也有 — 应该用 kicad 库里的版本
        let kicad = tempdir_in_target();
        let fallback = tempdir_in_target();
        let kicad_p = touch_pretty(&kicad, "LED_THT", "LED_D5.0mm");
        let fb_p = fallback.join("LED_D5.0mm.kicad_mod");
        fs::write(&fb_p, "(footprint \"fallback\")").unwrap();

        let found =
            find_footprint_file("LED_THT:LED_D5.0mm", &[kicad.as_path()], fallback.as_path());
        assert_eq!(found, Some(kicad_p));
    }

    #[test]
    fn find_returns_none_when_nowhere() {
        let tmp = tempdir_in_target();
        let found = find_footprint_file("MISSING:FP", &[tmp.as_path()], tmp.as_path());
        assert_eq!(found, None);
    }

    #[test]
    fn find_searches_multiple_kicad_lib_paths_in_order() {
        let lib_a = tempdir_in_target();
        let lib_b = tempdir_in_target();
        // lib_a 里没有 LED_THT.pretty, lib_b 里有 — 应该走到 lib_b
        let p = touch_pretty(&lib_b, "LED_THT", "LED_D5.0mm");
        let found = find_footprint_file(
            "LED_THT:LED_D5.0mm",
            &[lib_a.as_path(), lib_b.as_path()],
            Path::new("/nonexistent"),
        );
        assert_eq!(found, Some(p));
    }

    /// 在 target/ 下建一个临时目录, 测试用完不主动清理 (cargo test 之间复用同一棵 target/ 没事)。
    /// 用 std::env::temp_dir() 在 linux 上一般就是 /tmp, 每次跑 test 名字带 nanoid 风格不重复。
    fn tempdir_in_target() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("knead-net-test-{pid}-{nanos}-{n}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
