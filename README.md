# knead-net

把 KiCad PCB 文件 (`.kicad_pcb`) 投影到面包板上, 自动摆位 + 布线, 输出 SVG 调试图。

数据流: `.kicad_pcb` → [`Circuit`] → 模拟退火摆位 → A\* 风格布线 → SVG。

## 快速开始

```bash
cargo run --release
# 读 examples/inputs/h-bridge.kicad_pcb
# 输出 layout.svg / layout-spectral.svg 到 output/
```

跑参数扫描:

```bash
cargo run --release --bin sa_sweep
```

## 输入格式

只需要一个 `.kicad_pcb` 文件 (KiCad 的 PCB 文件, S-expression 格式)。
文件里内联了所有信息: 元件编号 (Reference)、元件值 (Value)、封装焊盘几何、
网络连接、引脚功能 (pinfunction)。不需要分开的网表和封装库文件。

加新电路时只需把 `.kicad_pcb` 放到 `examples/inputs/` 下,
然后改 `src/main.rs` 里的文件名即可。

```bash
cargo run --release
```

## 目录结构

```
src/
├── lib.rs          库根, pub mod 索引
├── main.rs         主 driver (跑 SA → 布线 → 写 SVG)
├── circuit.rs      领域模型 (Component / Net / Pin / Footprint / ...)
├── render.rs       SVG 渲染
├── input/          KiCad 格式解析
│   ├── pcb.rs       .kicad_pcb 解析器 (单文件, 一步到位)
│   └── sexp.rs      S-expression 解析小工具
└── layout/         摆位 + 布线核心
    ├── mod.rs        类型 / trait / re-export
    ├── breadboard.rs 面包板几何 + 电源轨
    ├── placement.rs  元件摆位规则
    ├── occupancy.rs  孔位占用追踪
    ├── routing.rs    A* 风格 wire 路径搜索
    ├── sa.rs         模拟退火主循环
    └── cost/         代价函数 (按功能分文件)

examples/
└── inputs/         测试电路 (.kicad_pcb)

output/             cargo run 渲染出来的 SVG (gitignored)
```

## 状态

实验性项目。核心算法 (SA + 路由) 可跑, 周边工程化 (CI / 正式测试 / CLI 框架) 是后续工作。

## License

未指定。
