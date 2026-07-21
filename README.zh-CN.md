# KneadNet

[English](README.md) | [简体中文](README.zh-CN.md)

> Knead what your nets need.

KneadNet 是一款跨平台桌面应用，用于把 KiCad 工程中的连接关系转换为面包板布局和跳线建议。它将自动元件摆位与原理图、面包板预览和装配清单结合在一起，方便用户对照搭建电路。

KneadNet 仍处于早期预览阶段。生成的布局只是建议，并不等同于电气验证；通电前请检查元件方向、引脚编号、电源轨和每一条连接。

## 快速开始

1. 安装并启动 KneadNet。
2. 选择或拖入包含 KiCad 工程的文件夹。
3. 选择带有 `.kicad_pcb` 文件的工程。
4. 选择面包板规格和需要的电源轨网络绑定。
5. 选择计算强度并开始搜索布局。
6. 对照原理图、面包板和装配清单搭建电路。

布局必须使用 `.kicad_pcb` 文件。同名 `.kicad_sch` 可以提供原理图预览和联动选择，但不是布局计算的必需输入。两个文件都必须直接位于所选文件夹中。

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

## 主要功能

- 在所选文件夹中识别 `.kicad_pcb` 和同名 `.kicad_sch` 文件。
- 使用 KiCad PCB 的网络与通孔焊盘几何作为布局输入。
- 预览同名原理图，并与面包板和装配清单联动选择。
- 提供可调整的 170、400 和 830 孔面包板预设。
- 自动搜索元件摆位并生成跳线布线建议。
- 在原理图、面包板和装配清单之间联动元件与网络。
- 在当前会话中记录装配进度。
- 支持拖入文件夹或 KiCad 文件。

## 下载

请从 [GitHub Releases](https://github.com/dfdc1123/knead-net-gui/releases) 下载已发布版本。

| 平台 | 推荐文件 | 说明 |
| --- | --- | --- |
| Windows x64 | `KneadNet_<version>_windows_x64-setup.exe` | 适合多数 Windows 用户的安装程序 |
| Windows x64 | `KneadNet_<version>_windows_x64_en-US.msi` | 适合集中管理环境的 MSI |
| macOS Intel / Apple 芯片 | `KneadNet_<version>_macos_universal.dmg` | Universal 应用包 |
| Linux x86-64 | `KneadNet_<version>_linux_amd64.AppImage` | 单文件便携应用 |
| Debian / Ubuntu x86-64 | `kneadnet_<version>_amd64.deb` | Debian 软件包 |
| Fedora / RPM x86-64 | `kneadnet-<version>-1.x86_64.rpm` | RPM 软件包 |

Release 中还包含用于核对下载文件的 `SHA256SUMS`，以及可以直接体验的 `KneadNet-examples-<version>.zip`。

当前构建尚未进行代码签名，因此 Windows SmartScreen 或 macOS Gatekeeper 可能显示警告。请只运行从本仓库下载的文件，并尽可能核对 SHA-256 校验值。

通过浏览器下载的 AppImage 可能没有可执行权限，可以使用：

```bash
chmod +x KneadNet_<version>_linux_amd64.AppImage
./KneadNet_<version>_linux_amd64.AppImage
```

Arch Linux 用户可以从 [AUR](https://aur.archlinux.org/packages/kneadnet-bin) 安装二进制包：

```bash
paru -S kneadnet-bin
```

原 `knead-net-gui` 包属于 v0.2.0 之前的命名方案；当前版本请使用 `kneadnet-bin`。

## 示例工程

最简单的方式是从应用对应的 GitHub Release 下载 `KneadNet-examples-<version>.zip`。解压后，在 KneadNet 中打开其中一个具体工程文件夹即可。

也可以直接浏览仓库中的 [`examples/` 目录](examples/)。各示例的用途参见 [`examples/README.md`](examples/README.md)。

## 平台说明

- **Windows：** 如果系统中没有合适的 WebView2 Runtime，安装时可能需要联网。
- **Linux：** AppImage 的兼容性取决于发行版和系统库版本。
- **macOS：** Universal DMG 同时支持 Intel 和 Apple 芯片，但未签名构建需要在 macOS 隐私与安全设置中明确批准。

## 当前限制

- 只支持通孔焊盘与封装；SMD 或混合封装工程可能导入失败。
- 工程发现不会递归扫描子文件夹。
- 原理图预览和联动选择要求原理图与 PCB 文件同名。
- 复杂电路或少见封装可能无法生成合法、实用的布局。
- 尚不能把结果导出为独立工程或报告。
- 没有命令行界面、自动更新器、深链接或已注册的 KiCad 文件关联。
- 关闭应用后不会保留装配进度。

## 从源码构建

源码构建需要 Node.js 22+、pnpm 11+、Rust stable toolchain，以及当前操作系统所需的 [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)。

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

使用 `pnpm tauri build` 可以生成当前宿主平台支持的软件包。开发环境、测试命令和 Pull Request 要求参见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。

## 参与贡献

欢迎提交问题报告和范围清晰的 Pull Request。请提供 KneadNet 版本、操作系统、软件包格式、KiCad 版本和复现步骤；只有在许可与隐私允许时才附上最小工程，请勿擅自上传私有原理图。

## 许可证

KneadNet 采用 [GNU General Public License v3.0](LICENSE)，SPDX 标识为 `GPL-3.0-only`。
