use std::fs;

use knead_net::input::footprint::parse_many as parse_footprints;
use knead_net::input::netlist::parse_netlist;
use knead_net::{Breadboard, Layout, Occupant, PathFinderRouter, Router, SAConfig};

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
    let circuit = netlist.into_circuit(&footprints);

    // 4. 布局: 模拟退火 + 压缩
    // 标准板: 30 cols × 12 rows, rows 5..7 是中央通道 (物理占位),
    // 上下半各自独立 rail, 同列不同 rail 互不连通。
    let board = Breadboard::standard();
    let mut layout = Layout::new(&circuit);
    // SA 是随机算法; 跑 10 次取最低 cost (MST cost 下大部分能找到 cost=0 的零跳线布局)
    if let Err(errors) = layout.place_sa(
        &board,
        &SAConfig {
            use_force_directed: true,
            max_iters: 50000,
            t0: 30.0,
            cool_rate: 0.999,
            n_seeds: 10,
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
            Some(p) => println!(
                "  {:<3} ({:<4}) {:<48} -> ({:>2}, {}) {:?}",
                c.ref_(),
                c.kind(),
                footprint_name,
                p.position.x,
                p.position.y,
                p.rotation
            ),
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
    let (wires, occ) = match layout.occupancy(&board) {
        Ok(occ) => {
            let router = PathFinderRouter {
                max_iterations: 200,
                history_increment: 1.0,
            };
            let wires = router.route(&circuit, &board, &occ);
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
            let wires = router.route(&circuit, &board, &occ);
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
