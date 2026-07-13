use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

mod compute;
mod sch;

/// 给 tests/sch_smoke.rs 用的入口
#[doc(hidden)]
pub fn test_render_sch(path: &str) -> Result<String, String> {
    sch::render(path)
}

/// 全局状态:记住用户当前选中的 .kicad_pcb 路径 + 面包板配置
#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) pcb_path: Mutex<Option<String>>,
    pub(crate) breadboard_cfg: Mutex<Option<(String, knead_net::layout::Breadboard)>>,
    pub(crate) compute_running: AtomicBool,
    pub(crate) next_run_id: AtomicU64,
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
fn list_folder(path: String) -> Result<Vec<FolderEntry>, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("不是目录: {}", path));
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
fn render_sch(path: String) -> Result<String, String> {
    sch::render(&path)
}

/// 把选中的 .pcb 路径存到全局 state, 供 Step 3 布局用
#[tauri::command]
fn set_pcb_path(state: tauri::State<AppState>, path: String) -> Result<(), String> {
    let mut guard = state.pcb_path.lock().map_err(|e| e.to_string())?;
    *guard = Some(path);
    Ok(())
}

/// 清除之前选择的 PCB，避免重新选择无 PCB 的目录后沿用旧路径。
#[tauri::command]
fn clear_pcb_path(state: tauri::State<AppState>) -> Result<(), String> {
    let mut guard = state.pcb_path.lock().map_err(|e| e.to_string())?;
    *guard = None;
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
) -> Result<BreadboardInfo, String> {
    let p = preset_from_str(&preset)?;
    let board = make_breadboard(p, cols)?;
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
            clear_pcb_path,
            get_pcb_path,
            set_breadboard,
            get_breadboard_info,
            compute::start_compute
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
