use std::fs;

use knead_net::input::footprint::parse_many as parse_footprints;
use knead_net::input::netlist::parse_netlist;

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
    let netlist_path = format!("{kicad_dir}/bjt_led.net");
    let netlist_text = fs::read_to_string(&netlist_path).unwrap();
    let netlist = parse_netlist(&netlist_text).unwrap();

    // 3. 组合成 Circuit (footprint ref 在这一步自动连到 FootprintId)
    let circuit = netlist.into_circuit(&footprints);

    println!("{circuit:#?}");
}
