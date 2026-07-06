# knead-net

把 KiCad 网表 (`.net` + `.kicad_mod`) 投影到面包板上, 自动摆位 + 布线, 输出 SVG 调试图。

数据流: `*.net / *.kicad_mod / *.json` → [`Circuit`] → 模拟退火摆位 → A\* 风格布线 → SVG。

## 快速开始

```bash
cargo run --release
# 读 examples/kicad/ 下的 footprint + h-bridge-power.net
# 输出 layout.svg / layout-spectral.svg 到同目录
```

跑参数扫描:

```bash
cargo run --release --bin sa_sweep
```

## 目录结构

```
src/
├── lib.rs          库根, pub mod 索引
├── main.rs         主 driver (跑 SA → 布线 → 写 SVG)
├── circuit.rs      领域模型 (Component / Net / Pin / Footprint / ...)
├── render.rs       SVG 渲染
├── input/          各种格式 parser
│   ├── netlist.rs    KiCad .net
│   ├── footprint.rs  KiCad .kicad_mod (lisp s-expression)
│   ├── sexp.rs       s-expression 解析小工具
│   └── json.rs       手写小电路用 (见 examples/led_bjt.json)
└── layout/         摆位 + 布线核心
    ├── mod.rs        类型 / trait / re-export
    ├── breadboard.rs 面包板几何 + 电源轨
    ├── placement.rs  元件摆位规则
    ├── occupancy.rs  孔位占用追踪
    ├── routing.rs    A* 风格 wire 路径搜索
    ├── sa.rs         模拟退火主循环
    └── cost.rs       代价函数 (4600 行, 内部已按章节分块)
```

## 状态

实验性项目。核心算法 (SA + 路由) 可跑, 周边工程化 (CI / 正式测试 / CLI 框架) 是后续工作。
详见 git log, 近期主要在做 SA 加速和桥接元件支持。

## License

未指定。
