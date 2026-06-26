//! Sweep SA over many seeds; print cost distribution.
//!
//! Usage: `cargo run --bin sa_sweep` from `knead-net/`.

use std::fs;

use knead_net::input::footprint::parse_many as parse_footprints;
use knead_net::input::netlist::{auto_mark_bridgeable, parse_netlist};
use knead_net::{
    Breadboard, Layout, PathFinderRouter, Placement, PowerRailBinding, Router, SAConfig,
};

fn main() {
    let kicad_dir = "examples/kicad";

    let mut footprint_paths: Vec<String> = fs::read_dir(kicad_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("kicad_mod"))
        .filter_map(|p| p.to_str().map(String::from))
        .collect();
    footprint_paths.sort();

    let footprint_texts: Vec<String> = footprint_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    let footprints = parse_footprints(footprint_texts).unwrap();

    let netlist_path = format!("{kicad_dir}/bjt_led.net");
    let netlist_text = fs::read_to_string(&netlist_path).unwrap();
    let netlist = parse_netlist(&netlist_text).unwrap();

    let mut circuit = netlist.into_circuit(&footprints);
    auto_mark_bridgeable(&mut circuit, &["GND", "+12V", "VCC", "5V", "3V3"]);

    const MASK_LOWER_HALF: bool = true;
    let board = {
        let mut blocked: Vec<usize> = vec![5, 6];
        if MASK_LOWER_HALF {
            blocked.extend(7..12);
        }
        Breadboard::with_power_rails(30, 12, blocked, knead_net::standard_power_rails(30))
    };

    let gnd_net = circuit.nets().iter().find(|n| n.name() == "GND");
    let v12_net = circuit.nets().iter().find(|n| n.name() == "+12V");
    let board = if let (Some(_), Some(_)) = (gnd_net, v12_net) {
        board.with_power_rail_binding(PowerRailBinding {
            positive: v12_net.unwrap().id(),
            negative: gnd_net.unwrap().id(),
        })
    } else {
        board
    };

    let n_trials = 50;
    let mut mst_sums: Vec<f64> = Vec::new();
    let mut layouts: Vec<(String, String, String)> = Vec::new();
    for trial in 0..n_trials {
        let mut layout = Layout::new(&circuit);
        let _ = layout.place_sa(
            &board,
            &SAConfig {
                use_force_directed: true,
                max_iters: 50000,
                t0: 30.0,
                cool_rate: 0.999,
                n_seeds: 1,
                seed: 0xCAFE_F00D + trial as u64,
                p_toggle_bridge: 0.15,
                ..SAConfig::default()
            },
        );
        let bridged_pins = layout.bridged_pins();
        let occ = knead_net::layout::Occupancy::from_layout_lossy(&layout, &board);
        let router = PathFinderRouter {
            max_iterations: 200,
            history_increment: 1.0,
        };
        let wires = router.route(&circuit, &board, &occ, &bridged_pins);

        let d1_pos = format_placement(&layout, &circuit, "D1");
        let q1_pos = format_placement(&layout, &circuit, "Q1");
        let r1_pos = format_placement(&layout, &circuit, "R1");

        let mut mst_sum = 0.0;
        for w in &wires {
            let from_pos = board.hole(w.from).position;
            let to_pos = board.hole(w.to).position;
            let dx = (from_pos.x - to_pos.x).abs();
            let dy = (from_pos.y - to_pos.y).abs();
            mst_sum += (dx + dy) as f64;
        }
        mst_sums.push(mst_sum);
        layouts.push((d1_pos, q1_pos, r1_pos));
    }

    let min = mst_sums.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = mst_sums.iter().cloned().fold(0.0_f64, f64::max);
    let mean = mst_sums.iter().sum::<f64>() / mst_sums.len() as f64;
    let n_zero = mst_sums.iter().filter(|&&c| c == 0.0).count();
    let n_two = mst_sums.iter().filter(|&&c| c == 2.0).count();
    let n_four = mst_sums.iter().filter(|&&c| c == 4.0).count();
    let n_other = mst_sums.len() - n_zero - n_two - n_four;

    println!(
        "=== SA sweep ({} trials, max_iters=50000, t0=30, cool=0.999, ShiftX=45% Flip=20% ShiftY=20% Toggle=15%) ===",
        n_trials
    );
    println!(
        "MST sum (wire cells): min={}, max={}, mean={:.2}",
        min, max, mean
    );
    println!("  zero wires:        {}/{}", n_zero, n_trials);
    println!("  2 wires (current): {}/{}", n_two, n_trials);
    println!("  4 wires:           {}/{}", n_four, n_trials);
    println!("  other:             {}/{}", n_other, n_trials);

    println!("\n=== Layouts with 0 wires (the optimal) ===");
    for (i, c) in mst_sums.iter().enumerate() {
        if *c == 0.0 {
            let (d1, q1, r1) = &layouts[i];
            println!("  trial {}: D1={}, Q1={}, R1={}", i, d1, q1, r1);
        }
    }
}

fn format_placement(layout: &Layout, circuit: &knead_net::Circuit, ref_: &str) -> String {
    let c = circuit
        .components()
        .iter()
        .find(|c| c.ref_() == ref_)
        .unwrap();
    match layout.placement(c.id()) {
        Some(Placement::OnBoard { position, rotation }) => {
            format!("({},{}){:?}", position.x, position.y, rotation)
        }
        Some(Placement::Bridged { .. }) => "Bridged".to_string(),
        None => "unplaced".to_string(),
    }
}
