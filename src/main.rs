use std::fs;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    Layout, Occupant, PathFinderRouter, Placement, PowerRailMatch, Preset, Router, SAConfig,
    prepare_for_layout, spectral_debug_positions,
};

// profile helpers live in sa.rs (pub(super) gated). We re-import via a small
// shim in `lib.rs` if needed; for now keep them private and reach via a debug
// helper exposed from the layout module.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let render_empty_boards = args.iter().any(|a| a == "--render-empty-boards");

    let inputs_dir = "examples/inputs";
    let outputs_dir = "output";
    fs::create_dir_all(outputs_dir).expect("创建 output 目录失败");

    // ── 仅渲染空板 (调试/验证用, 不读 .kicad_pcb, 不跑 SA) ──
    if render_empty_boards {
        render_empty_boards_to(outputs_dir);
        return;
    }

    // ── 读 .kicad_pcb 文件 (一步到位: 封装几何 + 网络连接都在里面) ──
    let pcb_path = format!("{inputs_dir}/h-bridge.kicad_pcb");
    let pcb_text = fs::read_to_string(&pcb_path).unwrap();
    let mut circuit = parse_pcb(&pcb_text).unwrap();
    eprintln!(
        "从 {pcb_path} 加载: {} 元件, {} net",
        circuit.components().len(),
        circuit.nets().len()
    );

    // ============================================================
    // 选板 — main.rs 里唯一一块“换板型”的地方。
    // 换 Preset 或改 BOARD_COLS 都能调。
    //   - Preset::Hole170: 无电源轨迷你板
    //   - Preset::Hole400: 5×5 电源轨标准板 (本次默认)
    //   - Preset::Hole800: 10×5 电源轨宽板, 左右各空 2
    // 电源轨存在与否、名字列表、宽度适配全都由 Preset::make 内部处理。
    // ============================================================
    const BOARD_PRESET: Preset = Preset::Hole800;
    const BOARD_COLS: usize = 40;
    let board = BOARD_PRESET.make(BOARD_COLS);
    let main_holes = board.cols() * 10; // 12 rows − 2 blocked = 10 main rows
    let rail_holes = board.len() - main_holes;
    eprintln!(
        "板子: {} preset × {} cols → {} cols × {} main rows + {} rail holes = {} total",
        BOARD_PRESET.name(),
        BOARD_COLS,
        board.cols(),
        main_holes / board.cols(),
        rail_holes,
        board.len()
    );

    let preparation = prepare_for_layout(&mut circuit, board);
    let board = preparation.board;
    for component_id in preparation.bridgeable_components {
        let component = &circuit.components()[component_id.raw()];
        eprintln!(
            "  bridgeable: {} (kind={})",
            component.ref_(),
            component.kind()
        );
    }
    match preparation.power_rails {
        PowerRailMatch::Bound(binding) => {
            let pos = &circuit.nets()[binding.positive.expect("bound positive rail").raw()];
            let neg = &circuit.nets()[binding.negative.expect("bound negative rail").raw()];
            eprintln!(
                "Power rail binding: - -> {} ({:?}), + -> {} ({:?})",
                neg.name(),
                neg.id(),
                pos.name(),
                pos.id()
            );
        }
        PowerRailMatch::PositiveOnly(id) => {
            let pos = &circuit.nets()[id.raw()];
            eprintln!(
                "Power rail: only positive {} matched, negative not found ({:?})",
                pos.name(),
                board.negative_names()
            );
        }
        PowerRailMatch::NegativeOnly(id) => {
            let neg = &circuit.nets()[id.raw()];
            eprintln!(
                "Power rail: only negative {} matched, positive not found ({:?})",
                neg.name(),
                board.positive_names()
            );
        }
        PowerRailMatch::IndividuallyBound(bindings) => {
            let labels: Vec<_> = bindings
                .iter()
                .map(|(side, polarity, id)| {
                    format!(
                        "{side:?} {polarity:?} -> {}",
                        circuit.nets()[id.raw()].name()
                    )
                })
                .collect();
            println!("Power rails: {}", labels.join(", "));
        }
        PowerRailMatch::Unmatched => {
            eprintln!(
                "Power rail: no match (positive={:?}, negative={:?})",
                board.positive_names(),
                board.negative_names()
            );
        }
        PowerRailMatch::NotPresent => {
            eprintln!("Power rail: 板子没电源轨 (preset={})", BOARD_PRESET.name());
        }
    }
    let mut layout = Layout::new(&circuit);

    // ============================================================
    // 预处理: 在 SA 和频谱调试之前算一次
    // ============================================================
    let preprocess = knead_net::layout::preprocess::preprocess_for_breadboard(&circuit, &board);
    if !preprocess.r90_only.is_empty() {
        let names: Vec<&str> = preprocess
            .r90_only
            .iter()
            .map(|&cid| circuit.components()[cid.raw()].ref_())
            .collect();
        eprintln!(
            "R90 预处理: {} 个元件 → {:?}",
            preprocess.r90_only.len(),
            names
        );
    }
    if !preprocess.y_locked.is_empty() {
        for (&cid, &y) in &preprocess.y_locked {
            eprintln!(
                "  y-lock: {} → y={}",
                circuit.components()[cid.raw()].ref_(),
                y
            );
        }
    }

    // ============================================================
    // 频谱调试: 输出 spectral 初始化策略的初排 SVG
    // ============================================================
    {
        // --- 频谱布局调试 ---
        eprintln!("=== 频谱布局初排 ===");
        let (_v2, _v3, spectral_placements) =
            spectral_debug_positions(&circuit, &board, &preprocess);
        if !spectral_placements.is_empty() {
            let mut sl_layout = Layout::new(&circuit);
            for (i, slot) in spectral_placements.iter().enumerate() {
                if let Some(p) = slot {
                    sl_layout.place(circuit.components()[i].id(), *p);
                }
            }
            let svg = knead_net::render::to_svg(&circuit, &board, &sl_layout);
            let path = format!("{outputs_dir}/layout-spectral.svg");
            fs::write(&path, &svg).expect("写 spectral SVG 失败");
            eprintln!("Spectral SVG → {path} ({} 字节)", svg.len());
        }
    }

    // SA 是随机算法; 跑 n_seeds 次独立模拟, 取 cost 最低的解
    //
    // 两种预设:
    // - 慢模式 (默认): n_seeds=100 + max_iters=1M, 接近能力上限 (10秒)。
    // - 快模式 (--quick): n_seeds=10 + max_iters=5000, ~5 秒。
    //   只用于反复试参数 / 调试; 质量差很多 (wire 数可能 超 过慢模式 1.5 倍)。
    //
    // 想只快速看一眼布局结构时可添 `--quick` 走快模式; 最终生成 SVG 走默认。
    let quick_mode = std::env::args().any(|a| a == "--quick");
    let profile_mode = std::env::args().any(|a| a == "--profile");
    if profile_mode {
        knead_net::layout::sa::reset_profile();
        knead_net::layout::cost::reset_cost_profile();
    }
    let sa_config = if quick_mode {
        SAConfig {
            use_spectral: true,
            max_iters: 5_000,
            t_start: 40.0,
            t_end: 0.01,
            n_seeds: 10,
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        }
    } else {
        SAConfig {
            use_spectral: true,
            max_iters: 1_000_000,
            t_start: 40.0,
            t_end: 0.01,
            n_seeds: 100,
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        }
    };
    if let Err(errors) = layout.place_sa(&board, &sa_config) {
        eprintln!("布局错误 ({} 个):", errors.len());
        for e in &errors {
            eprintln!("  - {e:?}");
        }
    }
    if profile_mode {
        knead_net::layout::sa::dump_profile("main");
        knead_net::layout::cost::dump_cost_profile("main");
    }

    println!("=== 摆放 (SA + 压缩) ===");
    for c in circuit.components() {
        // Component.footprint 是 FootprintId, 查一下拿名字
        let footprint_name = c
            .footprint()
            .and_then(|fid| circuit.footprints().get(fid.raw()))
            .map(|fp| fp.name())
            .unwrap_or("<none>");
        match layout.placement(c.id()) {
            Some(p) => match p {
                Placement::OnBoard { position, rotation } => println!(
                    "  {:<3} ({:<4}) {:<48} -> ({:>2}, {}) {:?}",
                    c.ref_(),
                    c.kind(),
                    footprint_name,
                    position.x,
                    position.y,
                    rotation
                ),
                Placement::Bridged { .. } => println!(
                    "  {:<3} ({:<4}) {:<48} -> 桥接",
                    c.ref_(),
                    c.kind(),
                    footprint_name
                ),
            },
            None => println!(
                "  {:<3} ({:<4}) {:<48} -> 未摆放",
                c.ref_(),
                c.kind(),
                footprint_name
            ),
        }
    }

    // 5. 接线: PathFinder 把所有 net 串起来。
    // 有冲突时, 不 return, 用 `from_layout_lossy` 尽力搭一个 occupancy 继续走。
    let bridged_pins = layout.bridged_pins();
    let (wires, occ) = match layout.occupancy(&board) {
        Ok(occ) => {
            let router = PathFinderRouter {
                max_iterations: 200,
                history_increment: 1.0,
            };
            let wires = router.route(&circuit, &board, &occ, &bridged_pins);
            (wires, occ)
        }
        Err(errs) => {
            eprintln!(
                "布局不合法, 仍画板子 ({} 个冲突, 见上); 用尽力 occupancy 接线",
                errs.len()
            );
            let occ = knead_net::layout::Occupancy::from_layout_lossy(&layout, &board);
            let router = PathFinderRouter {
                max_iterations: 200,
                history_increment: 1.0,
            };
            let wires = router.route(&circuit, &board, &occ, &bridged_pins);
            (wires, occ)
        }
    };
    println!("=== 接线 ({} 根 wire) ===", wires.len());
    for w in &wires {
        let from_pos = board.hole(w.from).position;
        let to_pos = board.hole(w.to).position;
        let net = &circuit.nets()[w.net.raw()];
        println!(
            "  wire #{} (net '{}'): ({:>2},{}) <-> ({:>2},{})",
            w.id.raw(),
            net.name(),
            from_pos.x,
            from_pos.y,
            to_pos.x,
            to_pos.y
        );
    }
    for w in &wires {
        layout.add_wire(w.clone());
    }

    // 6. 打完 wire 再打印一遍占用
    println!("=== 最终占用 (含 wire, lossy) ===");
    for hole in board.holes() {
        let Some(occupant) = occ.occupant_at(hole.id) else {
            continue;
        };
        let pos = hole.position;
        let desc = match occupant {
            Occupant::Pin(pin_id) => {
                let pin = &circuit.pins()[pin_id.raw()];
                let comp = &circuit.components()[pin.component().raw()];
                match pin.pinfunction() {
                    Some(f) => format!("{} pad {} ({})", comp.ref_(), pin.num(), f),
                    None => format!("{} pad {}", comp.ref_(), pin.num()),
                }
            }
            Occupant::Wire(wire_id) => format!("wire #{}", wire_id.raw()),
            Occupant::RailTie(tie_id) => format!("rail tie #{}", tie_id.raw()),
            Occupant::Blocked(cid) => {
                let comp = &circuit.components()[cid.raw()];
                format!("body of {}", comp.ref_())
            }
        };
        println!("  ({:>2}, {}): {}", pos.x, pos.y, desc);
    }

    // 7. 渲染 SVG (总是画, 有冲突也画)
    let svg = knead_net::render::to_svg(&circuit, &board, &layout);
    let svg_path = format!("{outputs_dir}/layout.svg");
    fs::write(&svg_path, &svg).expect("写 SVG 失败");
    println!("=== SVG 已写入 {svg_path} ({} 字节) ===", svg.len());
}

/// 仅渲染三种预设的空板到 `output/board-{preset}.svg`。供调试/核实用。
///
/// 输出:
/// - `board-170.svg`: 17 cols, 无电源轨
/// - `board-400.svg`: 30 cols, 上下各 2 条 5×5 电源轨
/// - `board-800.svg`: 63 cols, 上下各 2 条 10×5 电源轨 (左右各空 2 格)
fn render_empty_boards_to(outputs_dir: &str) {
    let cases: &[(Preset, usize)] = &[
        (Preset::Hole170, 17),
        (Preset::Hole400, 30),
        (Preset::Hole800, 63),
    ];
    for &(preset, cols) in cases {
        let board = preset.make(cols);
        let main_rows = board.main_rows();
        let blocked = board.blocked_rows().len();
        let rail_holes = board
            .power_rails()
            .map(|pr| {
                pr.top
                    .rows
                    .iter()
                    .chain(pr.bottom.rows.iter())
                    .fold(0usize, |acc, r| {
                        acc + r
                            .groups
                            .iter()
                            .map(|g| g.end() - g.start() + 1)
                            .sum::<i32>() as usize
                    })
            })
            .unwrap_or(0);
        eprintln!(
            "preset_{}({}): {}×{} main, {} blocked rows, {} rail holes = {} total",
            preset.name(),
            cols,
            cols,
            main_rows,
            blocked,
            rail_holes,
            board.len()
        );
        let svg = knead_net::render::to_svg_board(&board);
        let path = format!("{outputs_dir}/board-{}.svg", preset.name());
        fs::write(&path, &svg).expect("写空板 SVG 失败");
        eprintln!("  → {path} ({} 字节)", svg.len());
    }
}
