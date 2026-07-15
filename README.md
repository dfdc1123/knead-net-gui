# Knead Net

Knead Net 是一个实验性的桌面工具，用来把 KiCad 电路工程转换为可操作的面包板布局与装配指引。

它会读取 KiCad 的 PCB 连接与封装信息，自动选择元件摆位并生成跳线方案；如果工程中包含同名原理图，还可以在原理图、面包板和装配清单之间联动查看元件与网络。

> 项目仍处于早期预览阶段。目前可以从源码运行，首个可下载安装的 release 即将提供。

## 软件截图

### 1. 导入 KiCad 工程

![导入 KiCad 工程并预览原理图](docs/screenshots/tab1.png)

### 2. 选择面包板

![选择面包板规格并预览](docs/screenshots/tab2.png)

### 3. 计算布局

![计算元件布局与跳线](docs/screenshots/tab3.png)

### 4. 装配视图

![原理图、面包板与装配清单联动](docs/screenshots/tab4.png)

## 当前功能

- 从文件夹中识别并配对同名的 `.kicad_sch` 与 `.kicad_pcb` 文件
- 在应用内预览 KiCad 原理图
- 提供 170、400 和 800 孔面包板预设，并支持调整列数
- 使用 Spectral 初始布局、并行模拟退火和 routing 自动计算元件位置与跳线
- 提供快速、标准、完整三档计算强度，并实时显示计算过程
- 允许提前中断模拟退火，使用当前最佳结果继续布线
- 在原理图、面包板和清单之间同步高亮元件与网络
- 生成按孔位排列的元件与跳线装配清单，并可勾选记录装配进度
- 根据系统语言显示简体中文或英文界面

## 使用流程

1. 选择包含 KiCad 工程文件的文件夹。
2. 选择工程，并确认原理图预览与 PCB 文件已正确载入。
3. 选择面包板规格和列数。
4. 选择计算强度并开始计算。
5. 在装配视图中对照原理图、面包板和清单完成搭建。

`.kicad_pcb` 是进行布局计算的必要输入；同名的 `.kicad_sch` 用于原理图预览和联动，但不是必需的。两个文件需要直接位于所选文件夹中。

## 从源码运行

目前只提供 Tauri 桌面 GUI，不提供命令行界面。

开始前请先安装：

- Node.js 与 pnpm
- Rust stable toolchain
- Tauri 2 对应平台的系统依赖

安装前端依赖并启动开发版：

```bash
pnpm install
pnpm tauri dev
```

仓库的 `examples/folders/` 中包含可用于体验的 KiCad 示例工程。

## 本地检查

```bash
pnpm check
cargo test --workspace
```

## 工作原理

```text
KiCad PCB
   ↓
电路、封装与网络解析
   ↓
Spectral 初始布局
   ↓
多 seed 模拟退火摆位
   ↓
PathFinder / MST 跳线生成
   ↓
面包板与装配视图
```

## 项目结构

```text
knead-net-gui/
├── src/                    # SvelteKit 前端与 Rust 布局核心
│   ├── lib/components/     # 四步工作流与面包板组件
│   ├── routes/             # 应用页面
│   ├── input/              # KiCad PCB 解析
│   └── layout/             # 摆位、成本计算与布线算法
├── src-tauri/              # Tauri 桌面应用及原理图解析
├── examples/folders/       # 示例 KiCad 工程
├── package.json
└── Cargo.toml
```

## 当前限制

- 项目仍在开发中，复杂电路和少见封装可能无法得到理想布局。
- 结果导出功能尚未完成。
- 暂无可下载安装的构建；release 即将提供。
