use std::fs;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    Breadboard, Layout, Occupant, PathFinderRouter, Placement, PowerRailBinding, Router, SAConfig,
    spectral_debug_positions,
};

// profile helpers live in sa.rs (pub(super) gated). We re-import via a small
// shim in `lib.rs` if needed; for now keep them private and reach via a debug
// helper exposed from the layout module.

fn main() {
    let inputs_dir = "examples/inputs";
    let outputs_dir = "output";
    fs::create_dir_all(outputs_dir).expect("创建 output 目录失败");

    // ── 读 .kicad_pcb 文件 (一步到位: 封装几何 + 网络连接都在里面) ──
    let pcb_path = format!("{inputs_dir}/SNx4HC00.kicad_pcb");
    let pcb_text = fs::read_to_string(&pcb_path).unwrap();
    let mut circuit = parse_pcb(&pcb_text).unwrap();
    eprintln!(
        "从 {pcb_path} 加载: {} 元件, {} net",
        circuit.components().len(),
        circuit.nets().len()
    );

    // 3b. 自动标记可桥接元件: 2 pin + 一腿 power 一腿 signal
    // 名字列表是独立维护的 power-net 别名表; 标准板的 positive / negative
    // 名字列表在 `breadboard::standard_power_rails` 里硬编码, 跟这里互不相关。
    let power_names = ["GND", "+12V", "VCC", "5V", "3V3"];
    knead_net::input::pcb::auto_mark_bridgeable(&mut circuit, &power_names);
    for c in circuit.components() {
        if c.bridgeable {
            eprintln!("  bridgeable: {} (kind={})", c.ref_(), c.kind());
        }
    }

    // 4. 布局: 模拟退火 + 压缩
    // 标准板: 50 cols × 12 rows, rows 5..7 是中央通道 (物理占位),
    // 上下半各自独立 rail, 同列不同 rail 互不连通。 上下各一组 power rail。
    //
    // 两个独立开关, 能随时切换:
    //   - MASK_UPPER_HALF=true  → 上半 rows 0..4 全 blocked
    //   - MASK_LOWER_HALF=true  → 下半 rows 7..11 全 blocked
    // 组合:
    //   - 两个都 false → 完整标准板, 上下各 5 行都能用
    //   - 只 MASK_LOWER_HALF=true (默认) → 上半可用, 原本测试状态
    //   - 只 MASK_UPPER_HALF=true  → 下半可用 (rows 7..11)
    //   - 两个都 true    → 上下都屏蔽, 只剩中央 5/6 行 (本身也 blocked), 元件无处可放
    // 改任一行就能切换; SA / 路由 / 渲染 都会自动尊重 blocked row。
    const MASK_UPPER_HALF: bool = false;
    const MASK_LOWER_HALF: bool = false;
    let mut board = {
        let mut blocked: Vec<usize> = vec![5, 6]; // 标准中央通道
        if MASK_UPPER_HALF {
            blocked.extend(0..5); // 屏蔽上半
        }
        if MASK_LOWER_HALF {
            blocked.extend(7..12); // 屏蔽下半
        }
        Breadboard::with_power_rails(50, 12, blocked, knead_net::standard_power_rails(50))
    };
    match (MASK_UPPER_HALF, MASK_LOWER_HALF) {
        (false, false) => {
            eprintln!("板子使用完整标准板 (rows 0..5 上半 + rows 7..11 下半, 中央 5/6 blocked)")
        }
        (true, false) => eprintln!("⚠ 上半已屏蔽, 元件只能摆在 rows 7..11 (下半)"),
        (false, true) => eprintln!("⚠ 下半已屏蔽, 元件只能摆在 rows 0..5 (上半)"),
        (true, true) => eprintln!("⚠ 上下都屏蔽, 中央 5/6 blocked, 元件无处可放"),
    }

    // 4b. 把电源轨绑到具体 net (让 SA/路由把 rail 强制接进电路)
    let gnd_net = circuit.nets().iter().find(|n| n.name() == "GND");
    let v12_net = circuit.nets().iter().find(|n| n.name() == "+12V");
    if let (Some(gnd), Some(v12)) = (gnd_net, v12_net) {
        board = board.with_power_rail_binding(PowerRailBinding {
            positive: v12.id(),
            negative: gnd.id(),
        });
        eprintln!(
            "Power rail binding: − → GND ({:?}), + → +12V ({:?})",
            gnd.id(),
            v12.id()
        );
    } else {
        eprintln!("(电路里没找到 GND / +12V net, 电源轨不绑定)");
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
            t0: 40.0,
            cool_rate: 0.99999,
            n_seeds: 10,
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        }
    } else {
        SAConfig {
            use_spectral: true,
            max_iters: 1_000_000,
            t0: 40.0,
            cool_rate: 0.99999,
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
