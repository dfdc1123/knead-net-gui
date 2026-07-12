# knead-net

把 KiCad PCB 文件 (`.kicad_pcb`) 投影到面包板上, 自动摆位 + 布线, 输出 SVG 调试图。

数据流: `.kicad_pcb` → [`Circuit`] → 模拟退火摆位 → A\* 风格布线 → SVG。

## 快速开始

### CLI 模式（原始 Rust 入口）

```bash
cargo run --release
# 读 examples/inputs/h-bridge.kicad_pcb
# 输出 layout.svg / layout-spectral.svg 到 output/
```

### GUI 模式（Tauri 桌面应用）

```bash
pnpm install
pnpm tauri dev
```

会弹出一个 800×600 的桌面窗口，通过 SvelteKit 前端调用同一个 Rust 核心算法（不再写 SVG 文件，直接在窗口里看交互结果）。

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
knead-net-gui/
├── Cargo.toml          # workspace 根 + knead-net crate (老 Rust 核心)
├── src/                # 老 knead-net Rust 源码 (CLI 入口 + lib)
│   ├── lib.rs
│   ├── main.rs
│   ├── circuit.rs
│   ├── render.rs
│   ├── input/
│   └── layout/
├── examples/inputs/    # 测试电路 (.kicad_pcb)
├── src-tauri/          # Tauri crate (workspace member)
│   ├── Cargo.toml      # depends on knead-net (path = "..")
│   └── src/
│       ├── main.rs
│       └── lib.rs      # use knead_net::xxx; #[tauri::command]
├── src/                # SvelteKit 前端
│   ├── app.html
│   ├── app.css
│   └── routes/
└── package.json
```

## 状态

实验性项目。核心算法 (SA + 路由) 可跑, 周边工程化 (CI / 正式测试 / CLI 框架) 是后续工作。

## License

未指定。