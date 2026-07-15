use std::collections::HashMap;
use std::fs;
use std::sync::atomic::Ordering;
use std::time::Instant;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    Breadboard, CancellationToken, Circuit, HoleId, Layout, LayoutProgress, LayoutSnapshot,
    PathFinderRouter, ProgressOptions, Region, SAConfig,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::sch::ComponentMetadataMap;
use crate::{AppState, UiLocale};

const PROGRESS_EVENT: &str = "compute-progress";

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ComputeProfile {
    Quick,
    Standard,
    Full,
}

#[derive(Debug, Deserialize)]
pub struct ComputeRequest {
    profile: ComputeProfile,
    locale: UiLocale,
}

impl ComputeProfile {
    fn label(self, locale: UiLocale) -> &'static str {
        match self {
            Self::Quick => locale.text("快速", "Quick"),
            Self::Standard => locale.text("标准", "Standard"),
            Self::Full => locale.text("完整", "Full"),
        }
    }

    fn config(self) -> SAConfig {
        let (n_seeds, max_iters) = match self {
            Self::Quick => (8, 5_000),
            Self::Standard => (32, 200_000),
            Self::Full => (100, 1_000_000),
        };
        SAConfig {
            max_iters,
            n_seeds,
            use_spectral: true,
            // 正式计算使用较慢的降温配置；默认的 0.95 会过早冻结。
            t0: 40.0,
            cool_rate: 0.99999,
            ..SAConfig::default()
        }
    }
}

#[derive(Clone, Serialize)]
struct ComputeEvent {
    run_id: u64,
    phase: &'static str,
    progress: f64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<LayoutFrame>,
}

#[derive(Clone, Serialize)]
struct LayoutFrame {
    parts: Vec<LayoutPart>,
    wires: Vec<LayoutWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<f64>,
}

#[derive(Clone, Serialize)]
struct LayoutPart {
    id: String,
    reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    datasheet: Option<String>,
    footprint: String,
    package: &'static str,
    device: &'static str,
    pins: Vec<LayoutPin>,
}

#[derive(Clone, Serialize)]
struct LayoutPin {
    hole: BreadboardHole,
    number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_shape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    net_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    net_name: Option<String>,
}

#[derive(Clone, Serialize)]
struct LayoutWire {
    id: String,
    from: BreadboardHole,
    to: BreadboardHole,
    color: &'static str,
    kind: &'static str,
    net_id: String,
    net_name: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct BreadboardHole {
    region: &'static str,
    col: i32,
    row: usize,
}

struct RunningGuard<'a>(&'a AppState);

struct ProgressContext<'a> {
    circuit: &'a Circuit,
    board: &'a Breadboard,
    schematic_metadata: &'a ComponentMetadataMap,
    locale: UiLocale,
    started: &'a Instant,
}

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut cancellation) = self.0.compute_cancellation.lock() {
            *cancellation = None;
        }
        self.0.compute_running.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub fn cancel_compute(state: State<'_, AppState>) -> Result<bool, String> {
    let cancellation = state
        .compute_cancellation
        .lock()
        .map_err(|e| e.to_string())?;
    let Some(cancellation) = cancellation.as_ref() else {
        return Ok(false);
    };
    cancellation.cancel();
    Ok(true)
}

#[tauri::command]
pub async fn start_compute(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ComputeRequest,
) -> Result<(), String> {
    let locale = request.locale;
    if state
        .compute_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(locale
            .text("已有布局任务正在运行", "A layout task is already running")
            .into());
    }
    let _running = RunningGuard(&state);

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
    let schematic_metadata = state
        .schematic_metadata
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let (_, board) = state
        .breadboard_cfg
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| {
            locale
                .text(
                    "请先在 Step 2 选择面包板",
                    "Choose a breadboard in Step 2 first",
                )
                .to_string()
        })?;
    let run_id = state.next_run_id.fetch_add(1, Ordering::Relaxed) + 1;
    let cancellation = CancellationToken::new();
    *state
        .compute_cancellation
        .lock()
        .map_err(|e| e.to_string())? = Some(cancellation.clone());

    let result = tauri::async_runtime::spawn_blocking(move || {
        run_compute(
            app.clone(),
            run_id,
            &pcb_path,
            schematic_metadata,
            board,
            request,
            cancellation,
        )
        .inspect_err(|error| {
            let _ = app.emit(
                PROGRESS_EVENT,
                ComputeEvent {
                    run_id,
                    phase: "error",
                    progress: 0.0,
                    message: error.clone(),
                    frame: None,
                },
            );
        })
    })
    .await
    .map_err(|e| {
        format!(
            "{}: {e}",
            locale.text("布局任务异常退出", "Layout task exited unexpectedly")
        )
    })?;

    result
}

fn run_compute(
    app: AppHandle,
    run_id: u64,
    pcb_path: &str,
    schematic_metadata: ComponentMetadataMap,
    board: Breadboard,
    request: ComputeRequest,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let locale = request.locale;
    let started = Instant::now();
    let text = fs::read_to_string(pcb_path).map_err(|e| {
        format!(
            "{}: {e}",
            locale.text("读取 PCB 失败", "Failed to read PCB")
        )
    })?;
    let mut circuit = parse_pcb(&text).map_err(|e| {
        format!(
            "{}: {}",
            locale.text("解析 PCB 失败", "Failed to parse PCB"),
            e.message
        )
    })?;
    let board = knead_net::prepare_for_layout(&mut circuit, board).board;
    let mut layout = Layout::new(&circuit);

    let config = request.profile.config();
    let options = ProgressOptions {
        display_seed: 0,
        sample_every: (config.max_iters / 120).max(1),
    };

    // Rayon worker 只构造纯数据并送入 channel；窗口事件由专用转发线程发布。
    let (sender, receiver) = std::sync::mpsc::channel::<ComputeEvent>();
    let event_app = app.clone();
    let forwarder = std::thread::spawn(move || {
        for event in receiver {
            if event_app.emit(PROGRESS_EVENT, event).is_err() {
                break;
            }
        }
    });

    sender
        .send(ComputeEvent {
            run_id,
            phase: "spectral",
            progress: 0.0,
            message: match locale {
                UiLocale::ZhCn => format!(
                    "{}模式 · {} seeds × {} 次迭代",
                    request.profile.label(locale),
                    config.n_seeds,
                    config.max_iters
                ),
                UiLocale::En => format!(
                    "{} profile · {} seeds × {} iterations",
                    request.profile.label(locale),
                    config.n_seeds,
                    config.max_iters
                ),
            },
            frame: None,
        })
        .map_err(|_| {
            locale
                .text(
                    "进度转发线程已退出",
                    "Progress forwarding thread has exited",
                )
                .to_string()
        })?;

    let callback_sender = sender.clone();
    let progress_context = ProgressContext {
        circuit: &circuit,
        board: &board,
        schematic_metadata: &schematic_metadata,
        locale,
        started: &started,
    };
    layout
        .place_sa_with_progress_and_cancellation(
            &board,
            &config,
            options,
            &cancellation,
            |progress| {
                let event = progress_event(
                    run_id,
                    progress,
                    &progress_context,
                    cancellation.is_cancelled(),
                );
                let _ = callback_sender.send(event);
            },
        )
        .map_err(|errors| {
            format_layout_errors(locale.text("布局失败", "Layout failed"), &errors)
        })?;

    let cancelled = cancellation.is_cancelled();
    sender
        .send(ComputeEvent {
            run_id,
            phase: "routing",
            progress: 90.0,
            message: if cancelled {
                locale
                    .text(
                        "SA 已中断，正在为当前最佳布局生成跳线",
                        "SA stopped; routing the current best layout",
                    )
                    .into()
            } else {
                locale
                    .text(
                        "全局最佳布局已选出，正在生成跳线",
                        "Global best layout selected; generating wires",
                    )
                    .into()
            },
            frame: Some(snapshot_frame(
                &LayoutSnapshot {
                    placements: layout.placements().to_vec(),
                    wires: Vec::new(),
                },
                &circuit,
                &board,
                &schematic_metadata,
                None,
                None,
            )),
        })
        .map_err(|_| {
            locale
                .text(
                    "进度转发线程已退出",
                    "Progress forwarding thread has exited",
                )
                .to_string()
        })?;

    let route_sender = sender.clone();
    layout
        .route_with_progress(&board, &PathFinderRouter::default(), |progress| {
            let _ = route_sender.send(progress_event(
                run_id,
                progress,
                &progress_context,
                cancelled,
            ));
        })
        .map_err(|errors| {
            format_layout_errors(locale.text("布线失败", "Routing failed"), &errors)
        })?;

    drop(route_sender);
    drop(callback_sender);
    drop(sender);
    forwarder.join().map_err(|_| {
        locale
            .text(
                "进度转发线程异常退出",
                "Progress forwarding thread exited unexpectedly",
            )
            .to_string()
    })?;
    Ok(())
}

fn progress_event(
    run_id: u64,
    progress: LayoutProgress,
    context: &ProgressContext<'_>,
    cancelled: bool,
) -> ComputeEvent {
    let ProgressContext {
        circuit,
        board,
        schematic_metadata,
        locale,
        started,
    } = context;
    let locale = *locale;
    match progress {
        LayoutProgress::SpectralInitial { seed, snapshot } => ComputeEvent {
            run_id,
            phase: "spectral",
            progress: 5.0,
            message: match locale {
                UiLocale::ZhCn => format!("Spectral 初始布局 · 观察 seed {seed}"),
                UiLocale::En => format!("Spectral initial layout · observing seed {seed}"),
            },
            frame: Some(snapshot_frame(
                &snapshot,
                circuit,
                board,
                schematic_metadata,
                Some(0),
                None,
            )),
        },
        LayoutProgress::Annealing {
            seed,
            iteration,
            best_cost,
            snapshot,
            ..
        } => ComputeEvent {
            run_id,
            phase: "annealing",
            // 观察 seed 的帧不代表所有并行 seed 的总进度。
            progress: 10.0,
            message: match locale {
                UiLocale::ZhCn => format!("SA 优化中 · 固定观察 seed {seed}"),
                UiLocale::En => format!("SA optimization · observing seed {seed}"),
            },
            frame: Some(snapshot_frame(
                &snapshot,
                circuit,
                board,
                schematic_metadata,
                Some(iteration),
                Some(best_cost),
            )),
        },
        LayoutProgress::SeedsProgress { completed, total } => ComputeEvent {
            run_id,
            phase: "annealing",
            progress: 10.0 + 75.0 * completed as f64 / total.max(1) as f64,
            message: match locale {
                UiLocale::ZhCn => format!("SA 优化中 · 已完成 {completed}/{total} seeds"),
                UiLocale::En => format!("SA optimization · {completed}/{total} seeds complete"),
            },
            frame: None,
        },
        LayoutProgress::PlacementComplete {
            seed,
            cost,
            cancelled,
            snapshot,
        } => ComputeEvent {
            run_id,
            phase: "annealing",
            progress: 88.0,
            message: if cancelled {
                match locale {
                    UiLocale::ZhCn => format!("SA 已中断 · 当前最佳 seed {seed}"),
                    UiLocale::En => format!("SA stopped · current best seed {seed}"),
                }
            } else {
                match locale {
                    UiLocale::ZhCn => format!("全部种子完成 · 最佳 seed {seed}"),
                    UiLocale::En => format!("All seeds complete · best seed {seed}"),
                }
            },
            frame: Some(snapshot_frame(
                &snapshot,
                circuit,
                board,
                schematic_metadata,
                None,
                Some(cost),
            )),
        },
        LayoutProgress::RoutingComplete { snapshot } => ComputeEvent {
            run_id,
            phase: "done",
            progress: 100.0,
            message: match locale {
                UiLocale::ZhCn => format!(
                    "{} · 用时 {:.2}s",
                    if cancelled {
                        "中断后的布局与布线完成"
                    } else {
                        "布局与布线完成"
                    },
                    started.elapsed().as_secs_f64()
                ),
                UiLocale::En => format!(
                    "{} · {:.2}s",
                    if cancelled {
                        "Layout and routing complete after interruption"
                    } else {
                        "Layout and routing complete"
                    },
                    started.elapsed().as_secs_f64()
                ),
            },
            frame: Some(snapshot_frame(
                &snapshot,
                circuit,
                board,
                schematic_metadata,
                None,
                None,
            )),
        },
    }
}

fn snapshot_frame(
    snapshot: &LayoutSnapshot,
    circuit: &Circuit,
    board: &Breadboard,
    schematic_metadata: &ComponentMetadataMap,
    iteration: Option<usize>,
    cost: Option<f64>,
) -> LayoutFrame {
    let mut pin_holes = HashMap::new();
    let mut parts = Vec::new();

    for (index, placement) in snapshot.placements.iter().enumerate() {
        let Some(placement) = placement else { continue };
        let component = &circuit.components()[index];
        let Some(footprint_id) = component.footprint() else {
            continue;
        };
        let footprint = &circuit.footprints()[footprint_id.raw()];
        let Ok(placed) = placement.apply(component, footprint, board, circuit.pins()) else {
            continue;
        };
        let package = classify_package(component.kind(), footprint.name(), component.pins().len());
        let device = classify_device(component.kind());
        let component_metadata = schematic_metadata.get(component.ref_());
        let pins: Vec<_> = placed
            .pin_holes
            .into_iter()
            .map(|pin_hole| {
                let pin = &circuit.pins()[pin_hole.pin.raw()];
                let pin_metadata =
                    component_metadata.and_then(|metadata| metadata.pins.get(pin.num()));
                let hole = display_hole(board, pin_hole.hole);
                pin_holes.insert(pin_hole.pin, hole);
                LayoutPin {
                    hole,
                    number: pin.num().to_string(),
                    name: pin_metadata
                        .and_then(|metadata| metadata.name.clone())
                        .or_else(|| pin.pinfunction().map(str::to_string)),
                    pin_type: pin_metadata.and_then(|metadata| metadata.electrical_type.clone()),
                    pin_shape: pin_metadata.and_then(|metadata| metadata.shape.clone()),
                    unit: pin_metadata.and_then(|metadata| metadata.unit),
                    net_id: pin
                        .net()
                        .map(|net| circuit.nets()[net.raw()].name().to_string()),
                    net_name: pin
                        .net()
                        .map(|net| circuit.nets()[net.raw()].name().to_string()),
                }
            })
            .collect();
        parts.push(LayoutPart {
            id: format!("component-{index}"),
            reference: component.ref_().to_string(),
            value: component
                .value()
                .map(str::to_string)
                .or_else(|| component_metadata.and_then(|metadata| metadata.value.clone())),
            description: component_metadata.and_then(|metadata| metadata.description.clone()),
            datasheet: component_metadata.and_then(|metadata| metadata.datasheet.clone()),
            footprint: component_metadata
                .and_then(|metadata| metadata.footprint.clone())
                .unwrap_or_else(|| footprint.name().to_string()),
            package,
            device,
            pins,
        });
    }

    let wires = if snapshot.wires.is_empty() {
        air_wires(circuit, &pin_holes)
    } else {
        snapshot
            .wires
            .iter()
            .map(|wire| LayoutWire {
                id: format!("wire-{}", wire.id.raw()),
                from: display_hole(board, wire.from),
                to: display_hole(board, wire.to),
                color: net_color(wire.net.raw()),
                kind: "routed",
                net_id: circuit.nets()[wire.net.raw()].name().to_string(),
                net_name: circuit.nets()[wire.net.raw()].name().to_string(),
            })
            .collect()
    };

    LayoutFrame {
        parts,
        wires,
        iteration,
        cost,
    }
}

fn classify_package(component_kind: &str, footprint_name: &str, pin_count: usize) -> &'static str {
    let kind = component_kind.to_ascii_lowercase();
    let footprint = footprint_name.to_ascii_lowercase();
    if kind.contains("package_dip") || footprint.starts_with("dip-") {
        "dip"
    } else if pin_count == 2 {
        "axial"
    } else {
        "generic"
    }
}

fn classify_device(component_kind: &str) -> &'static str {
    let kind = component_kind.to_ascii_lowercase();
    if kind.contains("led") {
        "led"
    } else if kind.contains("diode") {
        "diode"
    } else {
        "generic"
    }
}

#[cfg(test)]
mod layout_metadata_tests {
    use super::{classify_device, classify_package, parse_pcb};

    #[test]
    fn classifies_only_the_kicad_metadata_needed_for_physical_markers() {
        assert_eq!(classify_package("Diode_THT", "D_DO-41", 2), "axial");
        assert_eq!(classify_device("Diode_THT"), "diode");
        assert_eq!(classify_package("Package_DIP", "DIP-8_W7.62mm", 8), "dip");
        assert_eq!(classify_device("Package_DIP"), "generic");
        assert_eq!(classify_device("Package_TO_SOT_THT"), "generic");
    }

    #[test]
    fn example_files_expose_the_pin_definitions_used_by_the_assembly_view() {
        let h_bridge = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/inputs/h-bridge.kicad_pcb"
        ))
        .unwrap();
        let circuit = parse_pcb(&h_bridge).unwrap();

        let diode = circuit
            .components()
            .iter()
            .find(|component| component.ref_() == "D1")
            .unwrap();
        let diode_footprint = &circuit.footprints()[diode.footprint().unwrap().raw()];
        let diode_package =
            classify_package(diode.kind(), diode_footprint.name(), diode.pins().len());
        let diode_device = classify_device(diode.kind());
        let diode_pins: Vec<_> = diode
            .pins()
            .iter()
            .map(|pin_id| {
                let pin = &circuit.pins()[pin_id.raw()];
                (pin.num(), pin.pinfunction().map(str::to_string))
            })
            .collect();
        assert_eq!(diode_package, "axial");
        assert_eq!(diode_device, "diode");
        assert!(diode_pins.contains(&("1", Some("K_1".into()))));
        assert!(diode_pins.contains(&("2", Some("A_2".into()))));

        let transistor = circuit
            .components()
            .iter()
            .find(|component| component.ref_() == "Q1")
            .unwrap();
        let transistor_names: Vec<_> = transistor
            .pins()
            .iter()
            .filter_map(|pin_id| {
                let pin = &circuit.pins()[pin_id.raw()];
                pin.pinfunction().map(str::to_string)
            })
            .collect();
        assert_eq!(transistor_names, ["C_1", "B_2", "E_3"]);

        let lm741 = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/inputs/lm741.kicad_pcb"
        ))
        .unwrap();
        let circuit = parse_pcb(&lm741).unwrap();
        let op_amp = circuit
            .components()
            .iter()
            .find(|component| component.ref_() == "U1")
            .unwrap();
        let footprint = &circuit.footprints()[op_amp.footprint().unwrap().raw()];
        assert_eq!(
            classify_package(op_amp.kind(), footprint.name(), op_amp.pins().len()),
            "dip"
        );
        assert!(op_amp.pins().iter().any(|pin_id| {
            let pin = &circuit.pins()[pin_id.raw()];
            pin.num() == "1" && pin.pinfunction() == Some("NULL_1")
        }));
    }
}

fn air_wires(
    circuit: &Circuit,
    pin_holes: &HashMap<knead_net::PinId, BreadboardHole>,
) -> Vec<LayoutWire> {
    let mut wires = Vec::new();
    for net in circuit.nets() {
        let holes: Vec<_> = net
            .pins()
            .iter()
            .filter_map(|pin| pin_holes.get(pin).copied())
            .collect();
        let Some((&first, rest)) = holes.split_first() else {
            continue;
        };
        for (index, &to) in rest.iter().enumerate() {
            wires.push(LayoutWire {
                id: format!("air-{}-{index}", net.id().raw()),
                from: first,
                to,
                color: net_color(net.id().raw()),
                kind: "air",
                net_id: net.name().to_string(),
                net_name: net.name().to_string(),
            });
        }
    }
    wires
}

fn display_hole(board: &Breadboard, hole_id: HoleId) -> BreadboardHole {
    let hole = board.hole(hole_id);
    if hole.region == Region::PowerRail {
        let rails = board
            .power_rails()
            .expect("power rail hole must have rails");
        for (region, strip) in [("rail-top", &rails.top), ("rail-bottom", &rails.bottom)] {
            if let Some(row) = strip.rows.iter().position(|rail| rail.y == hole.position.y) {
                return BreadboardHole {
                    region,
                    col: hole.position.x,
                    row,
                };
            }
        }
        unreachable!("power rail hole y must belong to a strip");
    }

    let blocked = board.blocked_rows();
    let visible_row = hole.position.y as usize
        - blocked
            .iter()
            .filter(|&&row| row < hole.position.y as usize)
            .count();
    let visible_rows = board.main_rows() - blocked.len();
    let top_rows = visible_rows / 2;
    if visible_row < top_rows {
        BreadboardHole {
            region: "main-top",
            col: hole.position.x,
            row: visible_row,
        }
    } else {
        BreadboardHole {
            region: "main-bottom",
            col: hole.position.x,
            row: visible_row - top_rows,
        }
    }
}

fn net_color(index: usize) -> &'static str {
    const COLORS: [&str; 8] = [
        "#c83434", "#2f6fbd", "#24845b", "#9b59b6", "#e67e22", "#007c91", "#8d6e63", "#ad8b00",
    ];
    COLORS[index % COLORS.len()]
}

fn format_layout_errors(context: &str, errors: &[knead_net::LayoutError]) -> String {
    let details = errors
        .iter()
        .take(4)
        .map(|error| format!("{error:?}"))
        .collect::<Vec<_>>()
        .join("; ");
    format!("{context}: {details}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_profiles_map_to_distinct_backend_configs() {
        let quick = ComputeProfile::Quick.config();
        let standard = ComputeProfile::Standard.config();
        let full = ComputeProfile::Full.config();

        assert_eq!((quick.n_seeds, quick.max_iters), (8, 5_000));
        assert_eq!((standard.n_seeds, standard.max_iters), (32, 200_000));
        assert_eq!((full.n_seeds, full.max_iters), (100, 1_000_000));
        for config in [quick, standard, full] {
            assert_eq!(config.t0, 40.0);
            assert_eq!(config.cool_rate, 0.99999);
            assert!(config.use_spectral);
        }
    }
}
