# knead-net

把 KiCad 网表 (`.net` + `.kicad_mod`) 投影到面包板上, 自动摆位 + 布线, 输出 SVG 调试图。

数据流: `*.net / *.kicad_mod / *.json` → [`Circuit`] → 模拟退火摆位 → A\* 风格布线 → SVG。

## 快速开始

```bash
cargo run --release
# 读 examples/footprints/ 下的 footprint + examples/inputs/h-bridge-power.net
# 输出 layout.svg / layout-spectral.svg 到 output/
```

跑参数扫描:

```bash
cargo run --release --bin sa_sweep
```

## Footprint 加载

需要的封装从两个地方找 (按顺序), 第一个命中即用:

1. `/usr/share/kicad/footprints/` (写死在 `src/main.rs` 顶上的 `KICAD_LIB_PATH` 常量)。
   这是系统装 KiCad 时自带的标准库, 里面是 `*.pretty/` 子目录, 例如
   `Package_DIP.pretty/DIP-14_W7.62mm.kicad_mod`。
   `.net` 里写的 `Package_DIP:DIP-14_W7.62mm` 会被拆成 `(lib=Package_DIP, name=DIP-14_W7.62mm)` 然后找
   `/usr/share/kicad/footprints/Package_DIP.pretty/DIP-14_W7.62mm.kicad_mod`。
2. `examples/footprints/<NAME>.kicad_mod` (flat fallback, 兼容手拷的本地图 ——
   例如你改过的封装, 或 kicad 系统库里没有的特殊件)。

加新例子时不需要再手拷 `.kicad_mod`: 只要 netlist 用的封装在系统 KiCad 库里就行。

```bash
cargo run --release
```

找不到时程序会报具体查了哪个 kicad 库路径和 fallback, 错误信息会直接告诉你缺哪个 ref。

要换 KiCad 库路径就改 `src/main.rs` 里的 `KICAD_LIB_PATH` 常量。

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
│   └── json.rs       手写小电路用 (见 examples/inputs/led_bjt.json)
└── layout/         摆位 + 布线核心
    ├── mod.rs        类型 / trait / re-export
    ├── breadboard.rs 面包板几何 + 电源轨
    ├── placement.rs  元件摆位规则
    ├── occupancy.rs  孔位占用追踪
    ├── routing.rs    A* 风格 wire 路径搜索
    ├── sa.rs         模拟退火主循环
    └── cost.rs       代价函数 (4600 行, 内部已按章节分块)

examples/
├── footprints/     .kicad_mod 物理封装
└── inputs/         测试电路 (.net / .json)

output/             cargo run 渲染出来的 SVG (gitignored)
```

## 状态

实验性项目。核心算法 (SA + 路由) 可跑, 周边工程化 (CI / 正式测试 / CLI 框架) 是后续工作。
详见 git log, 近期主要在做 SA 加速和桥接元件支持。

## License

未指定。
