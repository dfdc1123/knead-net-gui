use std::fs;

use knead_net::input::footprint::parse_many as parse_footprints;
use knead_net::input::netlist::parse_netlist;
use knead_net::{
    Breadboard, FDConfig, Layout, Occupant, PathFinderRouter, Placement, PowerRailBinding, Router,
    SAConfig, fd_debug_positions, spectral_debug_positions,
};

fn main() {
    let kicad_dir = "examples/kicad";

    // 1. 收齐 examples/kicad 下所有 .kicad_mod 文件
    let mut footprint_paths: Vec<String> = fs::read_dir(kicad_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("kicad_mod"))
        .filter_map(|p| p.to_str().map(String::from))
        .collect();
    // 排个序, 保证 FootprintId 分配顺序稳定
    footprint_paths.sort();

    let footprint_texts: Vec<String> = footprint_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    let footprints = parse_footprints(footprint_texts).unwrap();

    // 2. 读 .net 文件
    let netlist_path = format!("{kicad_dir}/h-bridge-power.net");
    let netlist_text = fs::read_to_string(&netlist_path).unwrap();
    let netlist = parse_netlist(&netlist_text).unwrap();

    // 3. 组合成 Circuit (footprint ref 在这一步自动连到 FootprintId)
    let mut circuit = netlist.into_circuit(&footprints);

    // 3b. 自动标记可桥接元件: 2 pin + 一腿 power 一腿 signal
    // 名字列表是独立维护的 power-net 别名表; 标准板的 positive / negative
    // 名字列表在 `breadboard::standard_power_rails` 里硬编码, 跟这里互不相关。
    let power_names = ["GND", "+12V", "VCC", "5V", "3V3"];
    knead_net::input::netlist::auto_mark_bridgeable(&mut circuit, &power_names);
    for c in circuit.components() {
        if c.bridgeable {
            eprintln!("  bridgeable: {} (kind={})", c.ref_(), c.kind());
        }
    }

    // 4. 布局: 模拟退火 + 压缩
    // 标准板: 50 cols × 12 rows, rows 5..7 是中央通道 (物理占位),
    // 上下半各自独立 rail, 同列不同 rail 互不连通。 上下各一组 power rail。
    //
    // `MASK_LOWER_HALF`: 是否屏蔽下半 (rows 7..12)?
    //   - true  → 下半全标 blocked, 元件只能摆在 rows 0..5 (上半 5 行, 中央通道被屏蔽)
    //   - false → 完整标准板, 上下各 5 行都能用
    // 改这一行就能切换; SA / 路由 / 渲染 都会自动尊重 blocked row。
    const MASK_LOWER_HALF: bool = true;
    let mut board = {
        let mut blocked: Vec<usize> = vec![5, 6]; // 标准中央通道
        if MASK_LOWER_HALF {
            blocked.extend(7..12); // 屏蔽下半
        }
        Breadboard::with_power_rails(50, 12, blocked, knead_net::standard_power_rails(50))
    };
    if MASK_LOWER_HALF {
        eprintln!("⚠ 下半已屏蔽, 元件只能摆在 rows 0..5 (上半)");
    } else {
        eprintln!("板子使用完整标准板 (rows 0..5 上半 + rows 7..11 下半, 中央 5/6 blocked)");
    }

    // 4b. 把电源轨绑到具体 net (让 SA/路由把 rail 强制接进电路)
    // - 负极 → GND
    // - 正极 → +12V (h-bridge-power.net 里用这个名字)
    // 找不到 net 就跳过, 退回原来的"不绑定"行为
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
    // 频谱 + FD 调试: 输出两种初始化策略的对比
    // ============================================================
    {
        // --- 频谱布局调试 ---
        eprintln!("=== 频谱布局初排 ===");
        let (_v2, _v3, spectral_placements) = spectral_debug_positions(&circuit, &board);
        if !spectral_placements.is_empty() {
            let mut sl_layout = Layout::new(&circuit);
            for (i, slot) in spectral_placements.iter().enumerate() {
                if let Some(p) = slot {
                    sl_layout.place(circuit.components()[i].id(), *p);
                }
            }
            let svg = knead_net::render::to_svg(&circuit, &board, &sl_layout);
            let path = format!("{kicad_dir}/layout-spectral.svg");
            fs::write(&path, &svg).expect("写 spectral SVG 失败");
            eprintln!("Spectral SVG → {path} ({} 字节)", svg.len());
        }

        // --- FD 调试 (保留对比) ---
        let fd_config = FDConfig::default();
        let (fd_positions, fd_placements) = fd_debug_positions(&circuit, &board, &fd_config);
        if !fd_positions.is_empty() {
            let placeable: Vec<_> = circuit
                .components()
                .iter()
                .filter_map(|c| {
                    c.footprint()?;
                    Some(c.id())
                })
                .collect();
            let svg_fd_cts = knead_net::render::to_svg_fd_continuous(
                &circuit,
                &board,
                &placeable,
                &fd_positions,
            );
            let fd_cts_path = format!("{kicad_dir}/layout-fd-continuous.svg");
            fs::write(&fd_cts_path, &svg_fd_cts).expect("写 FD continuous SVG 失败");
            eprintln!(
                "FD continuous SVG → {fd_cts_path} ({} 字节)",
                svg_fd_cts.len()
            );

            let mut fd_layout = Layout::new(&circuit);
            for (i, slot) in fd_placements.iter().enumerate() {
                if let Some(p) = slot {
                    fd_layout.place(circuit.components()[i].id(), *p);
                }
            }
            let svg_fd_snap = knead_net::render::to_svg(&circuit, &board, &fd_layout);
            let fd_snap_path = format!("{kicad_dir}/layout-fd-snapped.svg");
            fs::write(&fd_snap_path, &svg_fd_snap).expect("写 FD snapped SVG 失败");
            eprintln!(
                "FD snapped SVG    → {fd_snap_path} ({} 字节)",
                svg_fd_snap.len()
            );
        }
    }

    // SA 是随机算法; 跑 n_seeds 次独立模拟, 取 cost 最低的解
    if let Err(errors) = layout.place_sa(
        &board,
        &SAConfig {
            use_spectral: true,
            max_iters: 50000,
            t0: 40.0,
            cool_rate: 0.99999,
            n_seeds: 30,
            ..SAConfig::default()
        },
    ) {
        eprintln!("布局错误 ({} 个):", errors.len());
        for e in &errors {
            eprintln!("  - {e:?}");
        }
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
    let svg_path = format!("{kicad_dir}/layout.svg");
    fs::write(&svg_path, &svg).expect("写 SVG 失败");
    println!("=== SVG 已写入 {svg_path} ({} 字节) ===", svg.len());
}
