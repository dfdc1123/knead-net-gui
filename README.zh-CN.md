# KneadNet

[English](README.md) | [简体中文](README.zh-CN.md)

> Knead what your nets need.

KneadNet 是一款跨平台桌面应用，用于将电子原理图转换为面包板布局和布线建议。它读取 KiCad PCB 的连接关系与通孔封装几何信息，搜索可用的元件摆位、生成跳线方案，并把结果与原理图和装配清单联动展示。

KneadNet 仍处于早期预览阶段，适合体验和研究小型通孔电路，但不能代替人工核对电路及每一条生成的连接。

## 软件截图

界面采用四步工作流，并根据系统语言自动使用简体中文或英文。

### 导入 KiCad 工程

![导入 KiCad 工程并预览原理图](docs/screenshots/tab1.png)

### 选择面包板

![选择面包板规格并预览](docs/screenshots/tab2.png)

### 计算布局

![计算元件摆位与跳线](docs/screenshots/tab3.png)

### 使用装配视图

![原理图、面包板和装配清单联动](docs/screenshots/tab4.png)

## 当前功能

- 在所选文件夹中识别 `.kicad_pcb` 及同名 `.kicad_sch` 文件。
- 使用 KiCad PCB 的网络和通孔焊盘几何作为布局输入。
- 在存在同名原理图时显示原理图预览。
- 提供 170、400 和 830 孔面包板预设，并支持调整长度。
- 使用 Spectral 初始布局和并行模拟退火生成摆位。
- 在摆位完成后生成跳线布线建议。
- 提供快速、标准、完整三档计算强度和实时进度。
- 在原理图、面包板和装配清单之间联动选中的元件与网络。
- 在当前会话中记录元件和跳线的装配完成状态。
- 支持在桌面应用中拖入文件夹或 KiCad 文件。
- 提供简体中文和英文界面。

## 工作原理

```text
KiCad PCB 连接关系与封装几何
              |
              v
         电路与网络模型
              |
              v
       Spectral 初始摆位
              |
              v
      多 seed 模拟退火
              |
              v
   PathFinder / MST 跳线布线
              |
              v
      面包板预览与装配清单
```

布局必须使用 `.kicad_pcb` 文件。同名 `.kicad_sch` 可提供原理图预览和联动选择，但不是计算的必需输入。两个文件都必须直接位于 KneadNet 所选择的文件夹中。

## 下载

已发布的安装包位于 [GitHub Releases](https://github.com/dfdc1123/knead-net-gui/releases)。旧版本的资产可能不同；以下跨平台命名约定从 v0.2.0 开始使用。

| 平台 | 应下载的文件 | 说明 |
| --- | --- | --- |
| Windows x64 | `KneadNet_<version>_windows_x64-setup.exe` | 适合多数用户的 NSIS 安装器 |
| Windows x64 | `KneadNet_<version>_windows_x64_en-US.msi` | 适合集中管理的 MSI 安装器 |
| macOS Intel / Apple 芯片 | `KneadNet_<version>_macos_universal.dmg` | Universal 应用包 |
| Linux x86-64 | `KneadNet_<version>_linux_amd64.AppImage` | 单文件便携应用 |
| Debian / Ubuntu x86-64 | `kneadnet_<version>_amd64.deb` | Debian 软件包 |
| Fedora / RPM x86-64 | `kneadnet-<version>-1.x86_64.rpm` | RPM 软件包 |

每次跨平台发布还会包含 `SHA256SUMS` 和 `KneadNet-examples-<version>.zip`。架构名称遵循各平台习惯：Windows 使用 `x64`，Debian/AppImage 使用 `amd64`，RPM 使用 `x86_64`，同时包含 Intel 和 Apple 芯片代码的 macOS 包使用 `universal`。

当前构建尚未进行代码签名，因此 Windows SmartScreen 和 macOS Gatekeeper 可能在首次启动时警告。继续前请确认文件来自本仓库并核对 SHA-256。不要对从其他来源获得的文件绕过系统警告。

在 Linux 上核对单个文件的示例：

```bash
grep 'kneadnet_0.2.0_amd64.deb' SHA256SUMS | sha256sum --check
```

请将示例文件名替换为实际下载的资产。

通过浏览器下载的 AppImage 不一定带有可执行权限，可使用以下命令启用并启动：

```bash
chmod +x KneadNet_0.2.0_linux_amd64.AppImage
./KneadNet_0.2.0_linux_amd64.AppImage
```

### Arch Linux 与 AUR

仓库已经准备 `kneadnet-bin` 二进制包定义，但不会自动提交或更新 AUR。维护者完成手动发布后，可以使用 AUR helper 安装：

```bash
yay -S kneadnet-bin
```

在此之前，请使用 GitHub Release 软件包，或在替换明确的校验和占位符后构建本地 [`PKGBUILD`](packaging/aur/PKGBUILD)。旧 `knead-net-gui` AUR 包属于 v0.2.0 之前的命名方案。

## 快速开始

1. 安装并启动 KneadNet。
2. 选择或拖入包含 KiCad 工程的文件夹。
3. 选择带有 `.kicad_pcb` 文件的工程。
4. 选择面包板预设、长度、启用区域，以及需要的电源轨网络绑定。
5. 选择计算强度并等待摆位和布线完成；也可以提前中断退火，用当前最佳摆位继续布线。
6. 对照原理图、面包板和清单完成装配。

生成结果只应作为建议。通电前必须人工检查元件方向、引脚编号、电源轨和每一条连接。

## 示例工程

公开示例的说明位于 [`examples/README.md`](examples/README.md)。

### 方法一：下载 Release 示例包

打开 [GitHub Releases](https://github.com/dfdc1123/knead-net-gui/releases)，展开目标版本的 Assets，下载 `KneadNet-examples-<version>.zip`。解压后，在 KneadNet 中选择其中某个具体工程文件夹。

### 方法二：从仓库获取

- 在 GitHub 打开 [`examples/` 目录](https://github.com/dfdc1123/knead-net-gui/tree/main/examples)。
- 使用 GitHub 的 **Code → Download ZIP** 下载整个仓库；GitHub 本身不支持把任意文件夹直接下载为 ZIP。
- 克隆仓库：

  ```bash
  git clone https://github.com/dfdc1123/knead-net-gui.git
  cd knead-net-gui/examples
  ```

- 只需要示例时可使用 sparse checkout：

  ```bash
  git clone --filter=blob:none --no-checkout https://github.com/dfdc1123/knead-net-gui.git
  cd knead-net-gui
  git sparse-checkout set examples
  git checkout main
  ```

`examples/h-bridge_different_order` 是开发者回归测试夹具，因此不会进入 Release 示例包。

## 从源码构建

请先安装：

- Node.js 22 或更高版本。
- pnpm 11 或更高版本。
- Rust stable toolchain 与 Cargo。
- 目标平台所需的 [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)。

安装依赖并启动桌面应用：

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

构建前端或当前平台的原生软件包：

```bash
pnpm build
pnpm tauri build
```

Tauri 只能在对应宿主平台生成原生安装器。正式发布工作流会在相应的 GitHub runner 上构建各平台软件包。

## 平台说明

- **Windows：** 当系统缺少合适的 WebView2 Runtime 时，安装器会使用下载 bootstrapper，因此首次安装可能需要联网。
- **Linux：** AppImage 的兼容性取决于构建基线；正式 Release 会在 Ubuntu 22.04 上构建，而不是使用滚动发行版。
- **macOS：** DMG 同时包含 Intel 与 Apple 芯片代码；未签名版本可能需要在隐私与安全设置中明确批准。

## 仓库结构

```text
src/routes/                 SvelteKit 页面
src/lib/components/         工作流与面包板 UI 组件
src/input/                  KiCad PCB S-expression 解析
src/circuit.rs              电路领域模型
src/layout/                 摆位、成本、合法性与布线引擎
src-tauri/src/              Tauri 命令与原理图渲染
src-tauri/tests/            桌面集成测试
examples/                   公开示例与开发测试夹具
docs/screenshots/           README 截图
packaging/linux/            Linux desktop 与 AppStream 元数据
packaging/aur/              AUR 包定义和维护说明
scripts/                    版本与发行资产检查
.github/workflows/          CI 与草稿 Release 自动化
```

## 开发与测试

开发时应先运行范围最小的相关测试。提交 Pull Request 前运行：

```bash
pnpm check
pnpm test:ui
pnpm build
pnpm check:version
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

代码组织、测试要求和 Pull Request 说明参见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。

## 打包与发布

- [`docs/PACKAGING.md`](docs/PACKAGING.md) 说明产品 ID、包格式和平台细节。
- [`docs/RELEASING.md`](docs/RELEASING.md) 包含发布检查表和签名占位说明。
- [`packaging/aur/README.md`](packaging/aur/README.md) 说明 AUR 的手动维护流程。

稳定版本标签必须与 `package.json` 和 Cargo 元数据完全一致。标签工作流成功后只会创建草稿 Release，维护者完成检查和安装烟测后再公开。

## 当前限制

- 只支持通孔焊盘与封装；SMD 或混合封装工程可能导入失败。
- 工程发现只扫描所选文件夹，不递归扫描子目录。
- 原理图与 PCB 联动要求两个文件同名。
- 复杂电路或少见封装可能无法生成合法、实用的布局。
- 生成摆位和布线不等同于电气验证。
- 尚不能把结果导出为独立工程或报告。
- 没有命令行界面、自动更新器、深链接或已注册的 KiCad 文件关联。
- Release 构建目前未签名。

## 参与贡献

欢迎提交问题报告和范围清晰的 Pull Request。问题报告应包含 KneadNet 版本、操作系统、软件包格式、KiCad 版本、复现步骤，并在许可与隐私允许时附上最小工程。请勿擅自上传私有原理图。

## 许可证

KneadNet 采用 [GNU General Public License v3.0](LICENSE)，SPDX 标识为 `GPL-3.0-only`。除文件另有说明外，仓库图标、截图和公开示例随项目使用相同许可证发布。
