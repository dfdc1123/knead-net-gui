# knead-net

把 KiCad PCB 文件 (`.kicad_pcb`) 投影到面包板上, 自动摆位 + 布线, 输出 SVG 调试图。

数据流: `.kicad_pcb` → [`Circuit`] → Spectral 初排 → 多 seed 模拟退火 →
PathFinder/MST 跳线生成 → 面包板渲染。

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

GUI 已支持选择 KiCad 工程目录、预览原理图、配置面包板，并在 Step 3 连续展示
Spectral 初排、固定观察 seed 的 SA 过程以及全局最佳布局的最终布线。

Step 3 有三档计算强度：开发环境默认使用“快速”（8 seeds × 5,000 次），生产构建
默认使用“标准”（32 seeds × 200,000 次）。“完整”会运行 100 seeds × 1,000,000 次，
建议配合 release 构建使用。过程动画只观察一个固定 seed，最终结果仍从全部 seed 中
选择成本最低者；进度采样不参与算法决策。

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
│       ├── lib.rs      # Tauri 状态与 commands
│       ├── compute.rs  # 核心布局进度 → GUI 事件适配
│       └── sch.rs      # KiCad 原理图解析与 SVG 渲染
└── package.json
```

## 状态

实验性项目。Rust 核心算法（Spectral + 模拟退火摆位 + 路由）和 CLI 已可运行，并已有
单元测试与集成测试覆盖。GUI 的 Step 1（选择工程、原理图预览）、Step 2（面包板配置）
和 Step 3（计算过程与最终布线）已可用；Step 4 的结果整理与导出仍在开发中。仓库已配置
基础 CI，完整 CLI 参数框架仍是后续工作。

## License

未指定。
