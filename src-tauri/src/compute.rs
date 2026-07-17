use std::collections::HashMap;
use std::fs;
use std::sync::atomic::Ordering;
use std::time::Instant;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    Breadboard, BridgeInitial, BridgePolicy, CancellationToken, Circuit, HoleId, InitializerFamily,
    Layout, LayoutProgress, LayoutSnapshot, PathFinderRouter, Preset, ProgressOptions, Region,
    SAConfig,
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
        let (n_seeds, max_iters, t_end) = match self {
            Self::Quick => (8, 5_000, 0.1),
            Self::Standard => (32, 20_000, 0.001),
            Self::Full => (128, 40_000, 0.001),
        };
        SAConfig {
            max_iters,
            n_seeds,
            use_spectral: true,
            bridge_policy: BridgePolicy::Explore {
                initial: BridgeInitial::BestOfBoth,
            },
            t_start: 40.0,
            t_end,
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
    board_cols: usize,
    board_count: usize,
    total_cols: usize,
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
    properties: Vec<LayoutProperty>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exclude_from_sim: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    in_bom: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    on_board: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    in_pos_files: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dnp: Option<bool>,
}

#[derive(Clone, Serialize)]
struct LayoutProperty {
    name: String,
    value: String,
    hidden: bool,
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

#[derive(Debug, Clone, Copy)]
struct BoardAllocation {
    preset: Preset,
    board_cols: usize,
    board_count: usize,
}

struct RunningGuard<'a>(&'a AppState);

struct ProgressContext<'a> {
    circuit: &'a Circuit,
    board: &'a Breadboard,
    allocation: BoardAllocation,
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
    let board_config = state
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
        run_compute(ComputeJob {
            app: app.clone(),
            run_id,
            pcb_path,
            schematic_metadata,
            preset: board_config.preset,
            upper_half_only: board_config.upper_half_only,
            top_positive_net: board_config.top_positive_net,
            top_negative_net: board_config.top_negative_net,
            bottom_positive_net: board_config.bottom_positive_net,
            bottom_negative_net: board_config.bottom_negative_net,
            request,
            cancellation,
        })
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

struct ComputeJob {
    app: AppHandle,
    run_id: u64,
    pcb_path: String,
    schematic_metadata: ComponentMetadataMap,
    preset: String,
    upper_half_only: bool,
    top_positive_net: Option<String>,
    top_negative_net: Option<String>,
    bottom_positive_net: Option<String>,
    bottom_negative_net: Option<String>,
    request: ComputeRequest,
    cancellation: CancellationToken,
}

fn run_compute(job: ComputeJob) -> Result<(), String> {
    let ComputeJob {
        app,
        run_id,
        pcb_path,
        schematic_metadata,
        preset,
        upper_half_only,
        top_positive_net,
        top_negative_net,
        bottom_positive_net,
        bottom_negative_net,
        request,
        cancellation,
    } = job;
    let locale = request.locale;
    let started = Instant::now();
    let text = fs::read_to_string(&pcb_path).map_err(|e| {
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
    for (rail_zh, rail_en, selected) in [
        ("上方正极", "top positive", top_positive_net.as_deref()),
        ("上方负极", "top negative", top_negative_net.as_deref()),
        (
            "下方正极",
            "bottom positive",
            bottom_positive_net.as_deref(),
        ),
        (
            "下方负极",
            "bottom negative",
            bottom_negative_net.as_deref(),
        ),
    ] {
        if let Some(name) = selected {
            if !circuit.nets().iter().any(|net| net.name() == name) {
                return Err(format!(
                    "{}: {name}",
                    match locale {
                        UiLocale::ZhCn => format!("所选{rail_zh}电源轨网络已不存在"),
                        UiLocale::En => {
                            format!("The selected {rail_en} power-rail net no longer exists")
                        }
                    }
                ));
            }
        }
    }
    let preset = crate::preset_from_str(&preset)
        .map_err(|_| locale.text("未知面包板预设", "Unknown breadboard preset"))?;
    let board_cols = preset.default_cols();
    let prepared_single_board = knead_net::prepare_for_layout_with_individual_power_nets(
        &mut circuit,
        crate::make_breadboards(preset, 1, upper_half_only)?,
        top_positive_net.as_deref(),
        top_negative_net.as_deref(),
        bottom_positive_net.as_deref(),
        bottom_negative_net.as_deref(),
    )
    .board;
    let power_rail_bindings = prepared_single_board.power_rail_bindings().copied();

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

    let callback_sender = sender.clone();
    let mut selected = None;
    for board_count in 1..=crate::MAX_BOARD_COUNT {
        sender
            .send(ComputeEvent {
                run_id,
                phase: "spectral",
                progress: 0.0,
                message: match locale {
                    UiLocale::ZhCn => format!(
                        "正在尝试 {board_count} 块面包板 · {}模式 · {} seeds × {} 次迭代",
                        request.profile.label(locale),
                        config.n_seeds,
                        config.max_iters
                    ),
                    UiLocale::En => format!(
                        "Trying {board_count} breadboard{} · {} profile · {} seeds × {} iterations",
                        if board_count == 1 { "" } else { "s" },
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

        let raw_board = crate::make_breadboards(preset, board_count, upper_half_only)?;
        let board = match power_rail_bindings {
            Some(bindings) => raw_board.with_power_rail_bindings(bindings),
            None => raw_board,
        };
        let mut layout = Layout::new(&circuit);
        let progress_context = ProgressContext {
            circuit: &circuit,
            board: &board,
            allocation: BoardAllocation {
                preset,
                board_cols,
                board_count,
            },
            schematic_metadata: &schematic_metadata,
            locale,
            started: &started,
        };
        match layout.place_sa_with_progress_and_cancellation(
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
        ) {
            Ok(()) => {
                selected = Some((board, layout, board_count));
                break;
            }
            Err(errors)
                if is_initial_capacity_error(&errors) && board_count < crate::MAX_BOARD_COUNT =>
            {
                continue;
            }
            Err(errors) if is_initial_capacity_error(&errors) => {
                return Err(locale
                    .text(
                        "使用 4 块相同面包板仍无法完成初始布局",
                        "Initial placement still does not fit on four identical breadboards",
                    )
                    .to_string());
            }
            Err(errors) => {
                return Err(format_layout_errors(
                    locale.text("布局失败", "Layout failed"),
                    &errors,
                ));
            }
        }
    }
    let (board, mut layout, board_count) =
        selected.expect("the final capacity failure returns before leaving the retry loop");

    let progress_context = ProgressContext {
        circuit: &circuit,
        board: &board,
        allocation: BoardAllocation {
            preset,
            board_cols,
            board_count,
        },
        schematic_metadata: &schematic_metadata,
        locale,
        started: &started,
    };

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
                progress_context.allocation,
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

fn is_initial_capacity_error(errors: &[knead_net::LayoutError]) -> bool {
    !errors.is_empty()
        && errors.iter().all(|error| {
            matches!(
                error,
                knead_net::LayoutError::NoLegalInitialPlacement { .. }
            )
        })
}

fn board_count_for_max_x(
    max_x: Option<i32>,
    board_cols: usize,
    attempted_board_count: usize,
) -> usize {
    let required = max_x.map_or(1, |x| {
        (x.max(0) as usize / board_cols.max(1)).saturating_add(1)
    });
    required.clamp(1, attempted_board_count.max(1))
}

fn visible_board_count(
    snapshot: &LayoutSnapshot,
    circuit: &Circuit,
    board: &Breadboard,
    board_cols: usize,
    attempted_board_count: usize,
) -> usize {
    let placement_max = snapshot
        .placements
        .iter()
        .enumerate()
        .filter_map(|(index, placement)| {
            let placement = placement.as_ref()?;
            let component = &circuit.components()[index];
            let footprint = &circuit.footprints()[component.footprint()?.raw()];
            placement
                .apply(component, footprint, board, circuit.pins())
                .ok()?
                .bbox
                .map(|bbox| bbox.max_x)
        })
        .max();
    let wire_max = snapshot
        .wires
        .iter()
        .flat_map(|wire| [wire.from, wire.to])
        .map(|hole| board.hole(hole).position.x)
        .max();
    board_count_for_max_x(
        placement_max.into_iter().chain(wire_max).max(),
        board_cols,
        attempted_board_count,
    )
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
        allocation,
        schematic_metadata,
        locale,
        started,
    } = context;
    let locale = *locale;
    match progress {
        LayoutProgress::InitialPlacement {
            seed,
            initializer,
            cost,
            snapshot,
        } => ComputeEvent {
            run_id,
            phase: "spectral",
            progress: 5.0,
            message: match locale {
                UiLocale::ZhCn => format!(
                    "{} 初始布局 · seed {seed} · cost {cost:.1}",
                    initializer_label(initializer, locale)
                ),
                UiLocale::En => format!(
                    "{} initial layout · seed {seed} · cost {cost:.1}",
                    initializer_label(initializer, locale)
                ),
            },
            frame: Some(snapshot_frame(
                &snapshot,
                circuit,
                board,
                schematic_metadata,
                *allocation,
                Some(0),
                Some(cost),
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
                *allocation,
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
            ..
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
                *allocation,
                None,
                Some(cost),
            )),
        },
        LayoutProgress::RoutingComplete { snapshot } => {
            let visible_board_count = visible_board_count(
                &snapshot,
                circuit,
                board,
                allocation.board_cols,
                allocation.board_count,
            );
            let visible_allocation = BoardAllocation {
                board_count: visible_board_count,
                ..*allocation
            };
            ComputeEvent {
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
                    visible_allocation,
                    None,
                    None,
                )),
            }
        }
    }
}

fn initializer_label(initializer: InitializerFamily, locale: UiLocale) -> &'static str {
    match initializer {
        InitializerFamily::Greedy => locale.text("贪心", "Greedy"),
        InitializerFamily::Spectral => locale.text("频谱", "Spectral"),
        InitializerFamily::ForceDirected => locale.text("力导向", "Force-directed"),
        InitializerFamily::RandomizedGreedy => locale.text("随机化贪心", "Randomized greedy"),
    }
}

fn snapshot_frame(
    snapshot: &LayoutSnapshot,
    circuit: &Circuit,
    board: &Breadboard,
    schematic_metadata: &ComponentMetadataMap,
    allocation: BoardAllocation,
    iteration: Option<usize>,
    cost: Option<f64>,
) -> LayoutFrame {
    let BoardAllocation {
        preset,
        board_cols,
        board_count,
    } = allocation;
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
            properties: component_metadata
                .map(|metadata| {
                    metadata
                        .properties
                        .iter()
                        .map(|property| LayoutProperty {
                            name: property.name.clone(),
                            value: property.value.clone(),
                            hidden: property.hidden,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            exclude_from_sim: component_metadata.and_then(|metadata| metadata.exclude_from_sim),
            in_bom: component_metadata.and_then(|metadata| metadata.in_bom),
            on_board: component_metadata.and_then(|metadata| metadata.on_board),
            in_pos_files: component_metadata.and_then(|metadata| metadata.in_pos_files),
            dnp: component_metadata.and_then(|metadata| metadata.dnp),
        });
    }

    let mut wires = if snapshot.wires.is_empty() {
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
    let visible_tie_col = last_visible_power_rail_col(preset, board_cols, board_count);
    wires.extend(board.rail_ties().iter().map(|tie| {
        let polarity = board
            .power_rail_of(tie.from)
            .expect("preset RailTie endpoint must be on a power rail")
            .polarity;
        let tie_rail = board.effective_rail_id_of(tie.from);
        let bound_net = board
            .bound_power_rail_anchors()
            .into_iter()
            .find_map(|(anchor, net)| {
                (board.effective_rail_id_of(anchor) == tie_rail).then_some(net)
            });
        let bound_name = bound_net
            .filter(|net| net.raw() < circuit.nets().len())
            .map(|net| circuit.nets()[net.raw()].name().to_string());
        let polarity_name = match polarity {
            knead_net::Polarity::Negative => "negative",
            knead_net::Polarity::Positive => "positive",
        };
        let mut from = display_hole(board, tie.from);
        let mut to = display_hole(board, tie.to);
        if let Some(col) = visible_tie_col {
            from.col = col;
            to.col = col;
        }
        LayoutWire {
            id: format!("rail-tie:{}", tie.key),
            from,
            to,
            color: match polarity {
                knead_net::Polarity::Negative => "#2f6fbd",
                knead_net::Polarity::Positive => "#c83434",
            },
            kind: "rail-tie",
            net_id: bound_name
                .clone()
                .unwrap_or_else(|| format!("power-rail-{polarity_name}")),
            net_name: bound_name.unwrap_or_else(|| format!("{polarity_name} power-rail tie")),
        }
    }));

    LayoutFrame {
        board_cols,
        board_count,
        total_cols: board_cols * board_count,
        parts,
        wires,
        iteration,
        cost,
    }
}

fn last_visible_power_rail_col(
    preset: Preset,
    board_cols: usize,
    board_count: usize,
) -> Option<i32> {
    if preset == Preset::Hole170 || board_count == 0 {
        return None;
    }
    let margin = if preset == Preset::Hole800 { 2 } else { 0 };
    let mut last = None;
    let mut start = margin;
    while start < board_cols.saturating_sub(margin) {
        let end = (start + 4).min(board_cols - margin - 1);
        last = Some(end);
        start += 6;
    }
    last.map(|local| ((board_count - 1) * board_cols + local) as i32)
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
    let top_rows = blocked.first().copied().unwrap_or(visible_rows / 2);
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
        assert_eq!((standard.n_seeds, standard.max_iters), (32, 20_000));
        assert_eq!((full.n_seeds, full.max_iters), (128, 40_000));
        for config in [quick, standard, full] {
            assert_eq!(config.t_start, 40.0);
            assert!(config.use_spectral);
            assert_eq!(
                config.bridge_policy,
                BridgePolicy::Explore {
                    initial: BridgeInitial::BestOfBoth
                }
            );
        }
        assert_eq!(quick.t_end, 0.1);
        assert_eq!(standard.t_end, 0.001);
        assert_eq!(full.t_end, 0.001);
    }

    #[test]
    fn only_initial_capacity_errors_request_another_board() {
        use knead_net::LayoutError;

        let text =
            std::fs::read_to_string("../examples/folders/SNx4HC00/SNx4HC00.kicad_pcb").unwrap();
        let circuit = parse_pcb(&text).unwrap();
        let first = circuit.components()[0].id();
        let second = circuit.components()[1].id();

        assert!(is_initial_capacity_error(&[
            LayoutError::NoLegalInitialPlacement { component: first },
            LayoutError::NoLegalInitialPlacement { component: second },
        ]));
        assert!(!is_initial_capacity_error(&[LayoutError::NoFootprint {
            component: first,
        }]));
        assert!(!is_initial_capacity_error(&[]));
    }

    #[test]
    fn trailing_empty_boards_are_trimmed_from_the_visible_result() {
        assert_eq!(board_count_for_max_x(None, 30, 2), 1);
        assert_eq!(board_count_for_max_x(Some(29), 30, 2), 1);
        assert_eq!(board_count_for_max_x(Some(30), 30, 2), 2);
        assert_eq!(board_count_for_max_x(Some(62), 30, 4), 3);
        assert_eq!(board_count_for_max_x(Some(999), 30, 4), 4);
    }

    #[test]
    fn snapshot_frame_exposes_preset_rail_ties() {
        let circuit = Circuit::empty();
        let board = Breadboard::standard();
        let metadata = ComponentMetadataMap::new();
        let frame = snapshot_frame(
            &LayoutSnapshot {
                placements: Vec::new(),
                wires: Vec::new(),
            },
            &circuit,
            &board,
            &metadata,
            BoardAllocation {
                preset: Preset::Hole400,
                board_cols: 30,
                board_count: 1,
            },
            None,
            None,
        );

        let rail_ties: Vec<_> = frame
            .wires
            .iter()
            .filter(|wire| wire.kind == "rail-tie")
            .collect();
        assert_eq!(rail_ties.len(), 2);
        assert!(rail_ties
            .iter()
            .any(|wire| wire.id == "rail-tie:preset:negative:top-bottom"));
        assert!(rail_ties
            .iter()
            .any(|wire| wire.id == "rail-tie:preset:positive:top-bottom"));
    }

    #[test]
    fn trimmed_800_frame_moves_visual_rail_ties_to_the_last_visible_board() {
        let circuit = Circuit::empty();
        let board = Preset::Hole800.make_repeated(2);
        let metadata = ComponentMetadataMap::new();
        let frame = snapshot_frame(
            &LayoutSnapshot {
                placements: Vec::new(),
                wires: Vec::new(),
            },
            &circuit,
            &board,
            &metadata,
            BoardAllocation {
                preset: Preset::Hole800,
                board_cols: 63,
                board_count: 1,
            },
            None,
            None,
        );

        assert_eq!(frame.board_count, 1);
        assert_eq!(frame.total_cols, 63);
        let rail_ties: Vec<_> = frame
            .wires
            .iter()
            .filter(|wire| wire.kind == "rail-tie")
            .collect();
        assert_eq!(rail_ties.len(), 2);
        assert!(rail_ties
            .iter()
            .all(|wire| wire.from.col == 60 && wire.to.col == 60));
    }

    #[test]
    fn initializer_progress_exposes_the_post_bridge_cost() {
        let circuit = Circuit::empty();
        let board = Breadboard::standard();
        let metadata = ComponentMetadataMap::new();
        let started = Instant::now();
        let event = progress_event(
            7,
            LayoutProgress::InitialPlacement {
                seed: 42,
                initializer: knead_net::layout::InitializerFamily::ForceDirected,
                cost: 123.5,
                snapshot: LayoutSnapshot {
                    placements: Vec::new(),
                    wires: Vec::new(),
                },
            },
            &ProgressContext {
                circuit: &circuit,
                board: &board,
                allocation: BoardAllocation {
                    preset: Preset::Hole400,
                    board_cols: 30,
                    board_count: 1,
                },
                schematic_metadata: &metadata,
                locale: UiLocale::ZhCn,
                started: &started,
            },
            false,
        );

        assert_eq!(event.phase, "spectral");
        assert!(event.message.contains("cost 123.5"));
        assert!(event.message.contains("力导向"));
        assert_eq!(event.frame.expect("initializer frame").cost, Some(123.5));
    }
}
