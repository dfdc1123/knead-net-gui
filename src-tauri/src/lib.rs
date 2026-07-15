use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

mod compute;
mod sch;

#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub(crate) enum UiLocale {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

impl UiLocale {
    pub(crate) fn text(self, zh_cn: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::ZhCn => zh_cn,
            Self::En => en,
        }
    }
}

/// 给 tests/sch_smoke.rs 用的入口
#[doc(hidden)]
pub fn test_render_sch(path: &str) -> Result<String, String> {
    sch::render(path)
}

/// 给集成测试验证 Step 4 的元件/net 语义标记。
#[doc(hidden)]
pub fn test_render_sch_with_pcb(path: &str, pcb_path: &str) -> Result<String, String> {
    sch::render_with_pcb(path, Some(pcb_path))
}

/// 全局状态：当前工程输入、面包板配置和计算任务状态。
#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) pcb_path: Mutex<Option<String>>,
    pub(crate) schematic_metadata: Mutex<sch::ComponentMetadataMap>,
    pub(crate) breadboard_cfg: Mutex<Option<(String, knead_net::layout::Breadboard)>>,
    pub(crate) compute_running: AtomicBool,
    pub(crate) next_run_id: AtomicU64,
    pub(crate) compute_cancellation: Mutex<Option<knead_net::CancellationToken>>,
}

// ─────────────── Step 1: 选目录 + 渲染 .sch ───────────────

#[derive(Serialize)]
struct FolderEntry {
    name: String,
    path: String,
    ext: String,
    bytes: u64,
}

/// 列出指定目录下所有文件 (Step 1 选了目录后调用)
#[tauri::command]
fn list_folder(path: String, locale: UiLocale) -> Result<Vec<FolderEntry>, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!(
            "{}: {path}",
            locale.text("不是目录", "Not a directory")
        ));
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(FolderEntry {
            name,
            path: p.to_string_lossy().to_string(),
            ext,
            bytes,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 渲染 .kicad_sch → SVG 字符串 (Step 1 调用)
#[tauri::command]
fn render_sch(
    state: tauri::State<AppState>,
    path: String,
    pcb_path: Option<String>,
    locale: UiLocale,
) -> Result<String, String> {
    let (svg, metadata) = sch::render_with_pcb_and_metadata(&path, pcb_path.as_deref())
        .map_err(|error| localize_schematic_error(error, locale))?;
    *state.schematic_metadata.lock().map_err(|e| e.to_string())? = metadata;
    Ok(svg)
}

fn localize_schematic_error(error: sch::RenderError, locale: UiLocale) -> String {
    match error {
        sch::RenderError::Read(detail) => format!(
            "{}: {detail}",
            locale.text("读取原理图失败", "Failed to read schematic")
        ),
        sch::RenderError::Parse(detail) => format!(
            "{}: {detail}",
            locale.text("解析 S-Expression 失败", "Failed to parse S-expression")
        ),
        sch::RenderError::MissingLibrarySymbols => locale
            .text(
                "原理图缺少 lib_symbols 节点",
                "Schematic is missing lib_symbols",
            )
            .to_string(),
    }
}

/// 把选中的 .pcb 路径存到全局 state, 供 Step 3 布局用
#[tauri::command]
fn set_pcb_path(state: tauri::State<AppState>, path: String) -> Result<(), String> {
    let mut guard = state.pcb_path.lock().map_err(|e| e.to_string())?;
    *guard = Some(path);
    Ok(())
}

/// 清除之前选择的工程输入，避免项目切换时混用 PCB 和原理图元数据。
#[tauri::command]
fn clear_project_source(state: tauri::State<AppState>) -> Result<(), String> {
    let mut guard = state.pcb_path.lock().map_err(|e| e.to_string())?;
    *guard = None;
    state
        .schematic_metadata
        .lock()
        .map_err(|e| e.to_string())?
        .clear();
    Ok(())
}

/// 读出当前选中的 .pcb 路径 (Step 3 用)
#[tauri::command]
fn get_pcb_path(state: tauri::State<AppState>) -> Result<Option<String>, String> {
    state
        .pcb_path
        .lock()
        .map(|g| g.clone())
        .map_err(|e| e.to_string())
}

// ─────────────── Step 2: 选择面包板 ───────────────

#[derive(serde::Serialize, Clone)]
struct BreadboardInfo {
    preset: String,
    cols: usize,
    holes: usize,
    has_power_rails: bool,
}

fn preset_from_str(s: &str) -> Result<knead_net::layout::Preset, String> {
    use knead_net::layout::Preset;
    match s {
        "hole170" => Ok(Preset::Hole170),
        "hole400" => Ok(Preset::Hole400),
        "hole800" => Ok(Preset::Hole800),
        other => Err(format!("未知预设: {other}")),
    }
}

fn make_breadboard(
    preset: knead_net::layout::Preset,
    cols: usize,
) -> Result<knead_net::layout::Breadboard, String> {
    if !(3..=120).contains(&cols) {
        return Err("面包板列数必须在 3 到 120 之间".into());
    }
    if preset == knead_net::layout::Preset::Hole800 && cols < 4 {
        return Err("800 孔预设需要至少 4 列".into());
    }
    Ok(preset.make(cols))
}

#[tauri::command]
fn set_breadboard(
    state: tauri::State<AppState>,
    preset: String,
    cols: usize,
    locale: UiLocale,
) -> Result<BreadboardInfo, String> {
    let p = preset_from_str(&preset)
        .map_err(|_| format!("{}: {preset}", locale.text("未知预设", "Unknown preset")))?;
    let board = make_breadboard(p, cols).map_err(|_| {
        if !(3..=120).contains(&cols) {
            locale.text(
                "面包板列数必须在 3 到 120 之间",
                "Breadboard columns must be between 3 and 120",
            )
        } else {
            locale.text(
                "800 孔预设需要至少 4 列",
                "The 800-hole preset requires at least 4 columns",
            )
        }
        .to_string()
    })?;
    let info = BreadboardInfo {
        preset: preset.clone(),
        cols: board.cols(),
        holes: board.len(),
        has_power_rails: board.power_rails().is_some(),
    };
    *state.breadboard_cfg.lock().map_err(|e| e.to_string())? = Some((preset, board));
    Ok(info)
}

#[tauri::command]
fn get_breadboard_info(state: tauri::State<AppState>) -> Option<BreadboardInfo> {
    state.breadboard_cfg.lock().ok().and_then(|g| {
        g.as_ref().map(|(preset, b)| BreadboardInfo {
            preset: preset.clone(),
            cols: b.cols(),
            holes: b.len(),
            has_power_rails: b.power_rails().is_some(),
        })
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_folder,
            render_sch,
            set_pcb_path,
            clear_project_source,
            get_pcb_path,
            set_breadboard,
            get_breadboard_info,
            compute::start_compute,
            compute::cancel_compute
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use knead_net::layout::Preset;

    #[test]
    fn hole800_with_too_few_columns_returns_error_without_panicking() {
        for cols in 0..4 {
            let result = std::panic::catch_unwind(|| make_breadboard(Preset::Hole800, cols));
            assert!(result.is_ok(), "cols={cols} must not panic");
            assert!(result.unwrap().is_err(), "cols={cols} must return an error");
        }
    }

    #[test]
    fn column_limits_are_enforced_by_the_backend() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole800] {
            assert!(make_breadboard(preset, 0).is_err());
            assert!(make_breadboard(preset, 2).is_err());
            assert!(make_breadboard(preset, 121).is_err());
        }
    }

    #[test]
    fn valid_breadboard_dimensions_are_still_accepted() {
        assert_eq!(make_breadboard(Preset::Hole170, 17).unwrap().len(), 170);
        assert_eq!(make_breadboard(Preset::Hole400, 30).unwrap().len(), 400);
        assert_eq!(make_breadboard(Preset::Hole800, 4).unwrap().cols(), 4);
    }
}
