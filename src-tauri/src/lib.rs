// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use knead_net::input::pcb;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

mod sch;

/// 给 tests/sch_smoke.rs 用的入口
#[doc(hidden)]
pub fn test_render_sch(path: &str) -> Result<String, String> {
    sch::render(path)
}

/// 全局状态:记住用户当前选中的 .kicad_pcb 路径 + 面包板配置
#[derive(Default)]
struct AppState {
    pcb_path: Mutex<Option<String>>,
    breadboard_cfg: Mutex<Option<(String, knead_net::layout::Breadboard)>>,
}

/// 默认的问候命令 (Tauri 模板保留)
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[derive(Serialize)]
struct ExampleFile {
    name: String,
    path: String,
    bytes: u64,
}

/// 列出 examples/inputs/ 下所有可加载的 PCB 文件
#[tauri::command]
fn list_examples() -> Result<Vec<ExampleFile>, String> {
    let dir = PathBuf::from("../examples/inputs");
    if !dir.exists() {
        return Err(format!("examples/inputs 不存在: {}", dir.display()));
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("kicad_pcb") {
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            out.push(ExampleFile {
                name: path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string(),
                path: path.to_string_lossy().to_string(),
                bytes: meta.len(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 解析一个 PCB 文件, 返回最基础的领域统计
#[derive(Serialize)]
struct CircuitStats {
    components: usize,
    nets: usize,
    pins: usize,
}

#[tauri::command]
fn parse_circuit(path: String) -> Result<CircuitStats, String> {
    let circuit = pcb::parse_pcb(&path).map_err(|e| format!("解析失败: {e:?}"))?;
    Ok(CircuitStats {
        components: circuit.components().len(),
        nets: circuit.nets().len(),
        pins: circuit.pins().len(),
    })
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

#[tauri::command]
fn set_breadboard(
    state: tauri::State<AppState>,
    preset: String,
    cols: usize,
) -> Result<BreadboardInfo, String> {
    let p = preset_from_str(&preset)?;
    let board = p.make(cols);
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
            greet,
            list_examples,
            parse_circuit,
            list_folder,
            render_sch,
            set_pcb_path,
            get_pcb_path,
            set_breadboard,
            get_breadboard_info
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
