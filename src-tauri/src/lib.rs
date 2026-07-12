// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use knead_net::input::pcb;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

/// 默认的问候命令 (Tauri 模板保留, 给前端拿来测连通性)
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// 列出 examples/inputs/ 下所有可加载的 PCB 文件
/// 这是第一个把老 knead-net crate 接进来的 command —— 证明 workspace 集成通了
#[derive(Serialize)]
struct ExampleFile {
    name: String,
    path: String,
    bytes: u64,
}

#[tauri::command]
fn list_examples() -> Result<Vec<ExampleFile>, String> {
    // 走 CARGO_MANIFEST_DIR 之外的 examples/, 所以用 cwd 上溯一层
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

/// 解析一个 PCB 文件, 返回最基础的领域统计 (Component 数, Net 数)
/// 调用老 knead-net 的 parse_pcb, 证明从 Rust 调用老 lib 通了
#[derive(Serialize)]
struct CircuitStats {
    components: usize,
    nets: usize,
    pins: usize,
    source: String,
}

#[tauri::command]
fn parse_circuit(path: String) -> Result<CircuitStats, String> {
    let circuit = pcb::parse_pcb(&path).map_err(|e| format!("解析失败: {e:?}"))?;
    Ok(CircuitStats {
        components: circuit.components().len(),
        nets: circuit.nets().len(),
        pins: circuit.pins().len(),
        source: format!("{:?}", circuit),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            list_examples,
            parse_circuit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
