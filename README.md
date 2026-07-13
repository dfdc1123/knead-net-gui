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

会弹出一个 800×600 的桌面窗口。目前 GUI 已支持选择 KiCad 工程目录、预览原理图，
以及选择和配置面包板；计算流程和最终布线结果页面仍在开发中。

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
├── src/                # Rust 核心/CLI 与 SvelteKit 前端源码
│   ├── lib.rs
│   ├── main.rs
│   ├── circuit.rs
│   ├── render.rs
│   ├── input/
│   ├── layout/
│   ├── app.html
│   ├── app.css
│   ├── lib/components/
│   └── routes/
├── examples/inputs/    # 示例/测试电路 (.kicad_pcb)
├── src-tauri/          # Tauri crate (workspace member)
│   ├── Cargo.toml      # depends on knead-net (path = "..")
│   └── src/
│       ├── main.rs
│       ├── lib.rs      # use knead_net::xxx; #[tauri::command]
│       └── sch.rs      # KiCad 原理图解析与 SVG 渲染
└── package.json
```

## 状态

实验性项目。Rust 核心算法（模拟退火摆位 + 路由）和 CLI 已可运行，并已有单元测试与集成测试覆盖；
GUI 的 Step 1（选择工程、原理图预览）和 Step 2（面包板配置）已可用，Step 3（计算）与
Step 4（结果展示）仍在开发中。仓库已配置基础 CI，完整 CLI 参数框架仍是后续工作。

## License

未指定。
