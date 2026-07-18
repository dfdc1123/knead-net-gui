use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

mod compute;
mod sch;

pub(crate) const MAX_BOARD_COUNT: usize = 4;

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
    pub(crate) breadboard_cfg: Mutex<Option<BreadboardConfig>>,
    pub(crate) compute_running: AtomicBool,
    pub(crate) next_run_id: AtomicU64,
    pub(crate) compute_cancellation: Mutex<Option<knead_net::CancellationToken>>,
}

#[derive(Clone)]
pub(crate) struct BreadboardConfig {
    pub(crate) preset: String,
    pub(crate) board: knead_net::layout::Breadboard,
    pub(crate) use_upper_half: bool,
    pub(crate) use_lower_half: bool,
    pub(crate) top_positive_net: Option<String>,
    pub(crate) top_negative_net: Option<String>,
    pub(crate) bottom_positive_net: Option<String>,
    pub(crate) bottom_negative_net: Option<String>,
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
    use_upper_half: bool,
    use_lower_half: bool,
}

#[derive(serde::Serialize)]
struct PowerNetOptions {
    net_names: Vec<String>,
    positive_net: Option<String>,
    negative_net: Option<String>,
}

#[derive(serde::Deserialize)]
struct PowerNetSelection {
    top_positive_net: Option<String>,
    top_negative_net: Option<String>,
    bottom_positive_net: Option<String>,
    bottom_negative_net: Option<String>,
}

fn power_net_options_for(
    circuit: &knead_net::Circuit,
    board: &knead_net::layout::Breadboard,
) -> PowerNetOptions {
    let net_names: Vec<String> = circuit
        .nets()
        .iter()
        .map(|net| net.name().to_string())
        .collect();
    let positive_net = board
        .positive_names()
        .iter()
        .find(|candidate| net_names.contains(candidate))
        .cloned();
    let negative_net = board
        .negative_names()
        .iter()
        .find(|candidate| net_names.contains(candidate))
        .cloned();

    PowerNetOptions {
        net_names,
        positive_net,
        negative_net,
    }
}

fn preset_from_str(s: &str) -> Result<knead_net::layout::Preset, String> {
    use knead_net::layout::Preset;
    match s {
        "hole170" => Ok(Preset::Hole170),
        "hole400" => Ok(Preset::Hole400),
        "hole830" => Ok(Preset::Hole830),
        other => Err(format!("未知预设: {other}")),
    }
}

pub(crate) fn make_breadboards(
    preset: knead_net::layout::Preset,
    board_count: usize,
    use_upper_half: bool,
    use_lower_half: bool,
) -> Result<knead_net::layout::Breadboard, String> {
    if !(1..=MAX_BOARD_COUNT).contains(&board_count) {
        return Err(format!("面包板数量必须在 1 到 {MAX_BOARD_COUNT} 之间"));
    }
    if !use_upper_half && !use_lower_half {
        return Err("至少要启用面包板的一半".to_string());
    }
    Ok(if use_upper_half && use_lower_half {
        preset.make_repeated(board_count)
    } else if use_upper_half {
        preset.make_repeated_upper_half(board_count)
    } else {
        preset.make_repeated_lower_half(board_count)
    })
}

fn active_hole_count(board: &knead_net::layout::Breadboard) -> usize {
    board.len()
}

#[tauri::command]
fn set_breadboard(
    state: tauri::State<AppState>,
    preset: String,
    use_upper_half: bool,
    use_lower_half: bool,
    power_nets: PowerNetSelection,
    locale: UiLocale,
) -> Result<BreadboardInfo, String> {
    let p = preset_from_str(&preset)
        .map_err(|_| format!("{}: {preset}", locale.text("未知预设", "Unknown preset")))?;
    let board = make_breadboards(p, 1, use_upper_half, use_lower_half).map_err(|error| {
        format!(
            "{}: {error}",
            locale.text("无法创建面包板", "Failed to create breadboard")
        )
    })?;
    let info = BreadboardInfo {
        preset: preset.clone(),
        cols: board.cols(),
        holes: active_hole_count(&board),
        has_power_rails: board.power_rails().is_some(),
        use_upper_half,
        use_lower_half,
    };
    let has_power_rails = board.power_rails().is_some();
    let PowerNetSelection {
        top_positive_net,
        top_negative_net,
        bottom_positive_net,
        bottom_negative_net,
    } = power_nets;
    *state.breadboard_cfg.lock().map_err(|e| e.to_string())? = Some(BreadboardConfig {
        preset,
        board,
        use_upper_half,
        use_lower_half,
        top_positive_net: use_upper_half
            .then_some(top_positive_net)
            .flatten()
            .filter(|_| has_power_rails),
        top_negative_net: use_upper_half
            .then_some(top_negative_net)
            .flatten()
            .filter(|_| has_power_rails),
        bottom_positive_net: use_lower_half
            .then_some(bottom_positive_net)
            .flatten()
            .filter(|_| has_power_rails),
        bottom_negative_net: use_lower_half
            .then_some(bottom_negative_net)
            .flatten()
            .filter(|_| has_power_rails),
    });
    Ok(info)
}

#[tauri::command]
fn get_power_net_options(
    state: tauri::State<AppState>,
    preset: String,
    locale: UiLocale,
) -> Result<PowerNetOptions, String> {
    let pcb_path = state
        .pcb_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| {
            locale
                .text(
                    "请先在 Step 1 选择一个 .kicad_pcb 文件",
                    "Select a .kicad_pcb file in Step 1 first",
                )
                .to_string()
        })?;
    let text = fs::read_to_string(&pcb_path).map_err(|error| {
        format!(
            "{}: {error}",
            locale.text("读取 PCB 失败", "Failed to read PCB")
        )
    })?;
    let circuit = knead_net::input::pcb::parse_pcb(&text).map_err(|error| {
        format!(
            "{}: {}",
            locale.text("解析 PCB 失败", "Failed to parse PCB"),
            error.message
        )
    })?;
    let preset = preset_from_str(&preset)
        .map_err(|_| locale.text("未知面包板预设", "Unknown breadboard preset"))?;
    let board = make_breadboards(preset, 1, true, true)?;
    Ok(power_net_options_for(&circuit, &board))
}

#[tauri::command]
fn get_breadboard_info(state: tauri::State<AppState>) -> Option<BreadboardInfo> {
    state.breadboard_cfg.lock().ok().and_then(|g| {
        g.as_ref().map(|config| BreadboardInfo {
            preset: config.preset.clone(),
            cols: config.board.cols(),
            holes: active_hole_count(&config.board),
            has_power_rails: config.board.power_rails().is_some(),
            use_upper_half: config.use_upper_half,
            use_lower_half: config.use_lower_half,
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
            get_power_net_options,
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
    fn valid_breadboard_dimensions_are_still_accepted() {
        assert_eq!(
            make_breadboards(Preset::Hole170, 1, true, true)
                .unwrap()
                .len(),
            170
        );
        assert_eq!(
            make_breadboards(Preset::Hole400, 1, true, true)
                .unwrap()
                .len(),
            400
        );
        assert_eq!(
            make_breadboards(Preset::Hole830, 1, true, true)
                .unwrap()
                .cols(),
            63
        );
    }

    #[test]
    fn automatic_board_counts_scale_each_preset_through_four_boards() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole830] {
            for board_count in 1..=MAX_BOARD_COUNT {
                let board = make_breadboards(preset, board_count, true, true).unwrap();
                assert_eq!(
                    board.cols(),
                    preset.default_cols() * board_count
                        + preset.inter_board_gap_cols() * board_count.saturating_sub(1)
                );
            }
        }
        assert_eq!(
            make_breadboards(Preset::Hole830, 4, true, true)
                .unwrap()
                .cols(),
            261
        );
    }

    #[test]
    fn automatic_board_count_rejects_zero_and_more_than_four() {
        assert!(make_breadboards(Preset::Hole400, 0, true, true).is_err());
        assert!(make_breadboards(Preset::Hole400, MAX_BOARD_COUNT + 1, true, true).is_err());
    }

    #[test]
    fn repeated_830_boards_restart_power_rail_margins_around_the_inter_board_gap() {
        let board = make_breadboards(Preset::Hole830, 2, true, true).unwrap();

        for col in [60, 68, 126] {
            assert!(board.at(col, -4).is_some(), "rail hole missing at {col}");
        }
        for col in [61, 62, 63, 64, 65, 66, 67, 127, 128] {
            assert!(board.at(col, -4).is_none(), "rail margin missing at {col}");
        }
        for col in [63, 64, 65] {
            assert!(
                board.at(col, 0).is_none(),
                "inter-board main gap missing at {col}"
            );
        }
        assert!(board.at(62, 0).is_some());
        assert!(board.at(66, 0).is_some());
    }

    #[test]
    fn repeated_400_boards_restart_the_local_power_rail_cadence() {
        let board = make_breadboards(Preset::Hole400, 2, true, true).unwrap();

        assert!(board.at(28, -4).is_some());
        assert!(board.at(29, -4).is_none());
        for col in 30..33 {
            assert!(board.at(col, -4).is_none());
            assert!(board.at(col, 0).is_none());
        }
        assert!(board.at(33, -4).is_some());
        assert!(board.at(61, -4).is_some());
        assert!(board.at(62, -4).is_none());
    }

    #[test]
    fn upper_half_multi_boards_have_no_lower_main_holes_or_rail_ties() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole830] {
            let board = make_breadboards(preset, MAX_BOARD_COUNT, true, false).unwrap();
            assert!(board.at(0, 4).is_some());
            assert!(board.at((board.cols() - 1) as i32, 4).is_some());
            assert!(board.at(0, 7).is_none());
            assert!(board.at((board.cols() - 1) as i32, 7).is_none());
            assert!(board.at(0, 14).is_none());
            assert!(board.rail_ties().is_empty());
        }
    }

    #[test]
    fn lower_half_multi_boards_have_no_upper_main_holes_or_power_rails() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole830] {
            let board = make_breadboards(preset, MAX_BOARD_COUNT, false, true).unwrap();
            assert!(board.at(0, 7).is_some());
            assert!(board.at((board.cols() - 1) as i32, 11).is_some());
            assert!(board.at(0, 4).is_none());
            assert!(board.at((board.cols() - 1) as i32, 4).is_none());
            assert!(board.at(0, -4).is_none());
        }
    }

    #[test]
    fn selecting_no_board_half_is_rejected() {
        assert!(make_breadboards(Preset::Hole400, 1, false, false).is_err());
    }

    #[test]
    fn power_net_options_keep_the_existing_alias_priority() {
        let text = std::fs::read_to_string("../examples/SNx4HC00/SNx4HC00.kicad_pcb").unwrap();
        let circuit = knead_net::input::pcb::parse_pcb(&text).unwrap();
        let options = power_net_options_for(&circuit, &Preset::Hole400.make(30));

        assert_eq!(options.positive_net.as_deref(), Some("+5V"));
        assert_eq!(options.negative_net.as_deref(), Some("GND"));
        assert!(options.net_names.iter().any(|name| name == "+5V"));
    }

    #[test]
    fn board_without_rails_has_no_default_power_net_selection() {
        let text = std::fs::read_to_string("../examples/SNx4HC00/SNx4HC00.kicad_pcb").unwrap();
        let circuit = knead_net::input::pcb::parse_pcb(&text).unwrap();
        let options = power_net_options_for(&circuit, &Preset::Hole170.make(17));

        assert_eq!(options.positive_net, None);
        assert_eq!(options.negative_net, None);
    }
}
