use std::collections::HashMap;
use std::fs;
use std::sync::atomic::Ordering;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    Breadboard, Circuit, HoleId, Layout, LayoutProgress, LayoutSnapshot, PathFinderRouter,
    ProgressOptions, Region, SAConfig,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::AppState;

const PROGRESS_EVENT: &str = "compute-progress";

#[derive(Debug, Deserialize)]
pub struct ComputeRequest {
    n_seeds: usize,
    max_iters: usize,
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
    kind: &'static str,
    pins: Vec<LayoutPin>,
}

#[derive(Clone, Serialize)]
struct LayoutPin {
    hole: BreadboardHole,
    number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Clone, Serialize)]
struct LayoutWire {
    id: String,
    from: BreadboardHole,
    to: BreadboardHole,
    color: &'static str,
    kind: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct BreadboardHole {
    region: &'static str,
    col: i32,
    row: usize,
}

struct RunningGuard<'a>(&'a AppState);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.compute_running.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn start_compute(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ComputeRequest,
) -> Result<(), String> {
    validate_request(&request)?;
    if state
        .compute_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("已有布局任务正在运行".into());
    }
    let _running = RunningGuard(&state);

    let pcb_path = state
        .pcb_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "请先在 Step 1 选择一个 .kicad_pcb 文件".to_string())?;
    let (_, board) = state
        .breadboard_cfg
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "请先在 Step 2 选择面包板".to_string())?;
    let run_id = state.next_run_id.fetch_add(1, Ordering::Relaxed) + 1;

    let result = tauri::async_runtime::spawn_blocking(move || {
        run_compute(app.clone(), run_id, &pcb_path, board, request).inspect_err(|error| {
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
    .map_err(|e| format!("布局任务异常退出: {e}"))?;

    result
}

fn validate_request(request: &ComputeRequest) -> Result<(), String> {
    if !(1..=256).contains(&request.n_seeds) {
        return Err("种子数量必须在 1 到 256 之间".into());
    }
    if !(1..=2_000_000).contains(&request.max_iters) {
        return Err("每个种子的迭代次数必须在 1 到 2,000,000 之间".into());
    }
    Ok(())
}

fn run_compute(
    app: AppHandle,
    run_id: u64,
    pcb_path: &str,
    board: Breadboard,
    request: ComputeRequest,
) -> Result<(), String> {
    let text = fs::read_to_string(pcb_path).map_err(|e| format!("读取 PCB 失败: {e}"))?;
    let mut circuit = parse_pcb(&text).map_err(|e| format!("解析 PCB 失败: {}", e.message))?;
    let board = knead_net::prepare_for_layout(&mut circuit, board).board;
    let mut layout = Layout::new(&circuit);

    let config = SAConfig {
        max_iters: request.max_iters,
        n_seeds: request.n_seeds,
        use_spectral: true,
        ..SAConfig::default()
    };
    let options = ProgressOptions {
        display_seed: 0,
        sample_every: (request.max_iters / 120).max(1),
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
    layout
        .place_sa_with_progress(&board, &config, options, |progress| {
            let event = progress_event(run_id, progress, &circuit, &board);
            let _ = callback_sender.send(event);
        })
        .map_err(|errors| format_layout_errors("布局失败", &errors))?;

    sender
        .send(ComputeEvent {
            run_id,
            phase: "routing",
            progress: 90.0,
            message: "全局最佳布局已选出，正在生成跳线".into(),
            frame: Some(snapshot_frame(
                &LayoutSnapshot {
                    placements: layout.placements().to_vec(),
                    wires: Vec::new(),
                },
                &circuit,
                &board,
                None,
                None,
            )),
        })
        .map_err(|_| "进度转发线程已退出".to_string())?;

    let route_sender = sender.clone();
    layout
        .route_with_progress(&board, &PathFinderRouter::default(), |progress| {
            let _ = route_sender.send(progress_event(run_id, progress, &circuit, &board));
        })
        .map_err(|errors| format_layout_errors("布线失败", &errors))?;

    drop(route_sender);
    drop(callback_sender);
    drop(sender);
    forwarder
        .join()
        .map_err(|_| "进度转发线程异常退出".to_string())?;
    Ok(())
}

fn progress_event(
    run_id: u64,
    progress: LayoutProgress,
    circuit: &Circuit,
    board: &Breadboard,
) -> ComputeEvent {
    match progress {
        LayoutProgress::SpectralInitial { seed, snapshot } => ComputeEvent {
            run_id,
            phase: "spectral",
            progress: 5.0,
            message: format!("Spectral 初始布局 · 观察 seed {seed}"),
            frame: Some(snapshot_frame(&snapshot, circuit, board, Some(0), None)),
        },
        LayoutProgress::Annealing {
            seed,
            iteration,
            total_iterations,
            best_cost,
            snapshot,
            ..
        } => ComputeEvent {
            run_id,
            phase: "annealing",
            progress: 10.0 + 75.0 * iteration as f64 / total_iterations.max(1) as f64,
            message: format!("SA 优化中 · 固定观察 seed {seed}"),
            frame: Some(snapshot_frame(
                &snapshot,
                circuit,
                board,
                Some(iteration),
                Some(best_cost),
            )),
        },
        LayoutProgress::PlacementComplete {
            seed,
            cost,
            snapshot,
        } => ComputeEvent {
            run_id,
            phase: "annealing",
            progress: 88.0,
            message: format!("全部种子完成 · 最佳 seed {seed}"),
            frame: Some(snapshot_frame(&snapshot, circuit, board, None, Some(cost))),
        },
        LayoutProgress::RoutingComplete { snapshot } => ComputeEvent {
            run_id,
            phase: "done",
            progress: 100.0,
            message: "布局与布线完成".into(),
            frame: Some(snapshot_frame(&snapshot, circuit, board, None, None)),
        },
    }
}

fn snapshot_frame(
    snapshot: &LayoutSnapshot,
    circuit: &Circuit,
    board: &Breadboard,
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
        let pins: Vec<_> = placed
            .pin_holes
            .into_iter()
            .map(|pin_hole| {
                let pin = &circuit.pins()[pin_hole.pin.raw()];
                let hole = display_hole(board, pin_hole.hole);
                pin_holes.insert(pin_hole.pin, hole);
                LayoutPin {
                    hole,
                    number: pin.num().to_string(),
                    name: pin.pinfunction().map(str::to_string),
                }
            })
            .collect();
        let kind = if pins.len() == 2 {
            "axial"
        } else if pins.len() >= 6 {
            "ic"
        } else {
            "generic"
        };
        parts.push(LayoutPart {
            id: format!("component-{index}"),
            reference: component.ref_().to_string(),
            value: component.value().map(str::to_string),
            kind,
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
