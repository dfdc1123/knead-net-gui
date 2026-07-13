// 临时集成测试：把 .sch 渲染成 SVG 写到 /tmp 看效果
#[test]
fn render_lm741_sch() {
    let svg = knead_net_gui_lib::test_render_sch("../examples/folders/lm741/lm741.kicad_sch")
        .expect("render failed");
    std::fs::write("/tmp/lm741.svg", &svg).unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.ends_with("</svg>"));
    eprintln!("SVG length: {} bytes", svg.len());
}

#[test]
fn render_lm741_sch_contains_step4_selection_metadata() {
    let svg = knead_net_gui_lib::test_render_sch_with_pcb(
        "../examples/folders/lm741/lm741.kicad_sch",
        "../examples/folders/lm741/lm741.kicad_pcb",
    )
    .expect("render with PCB metadata failed");

    assert!(svg.contains(r#"class="sch-component" data-component="#));
    assert!(svg.contains(r#"class="sch-net-line" data-net="#));
}
