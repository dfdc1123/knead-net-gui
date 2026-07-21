# KneadNet

[English](README.md) | [简体中文](README.zh-CN.md)

> Knead what your nets need.

原理图 -> 面包板

## 快速开始

1. 安装并启动 KneadNet
2. 选择或拖入 KiCad 工程文件夹
3. 选择面包板；绑定电源轨
4. 对照原理图、面包板和装配清单搭建电路

## 软件截图

### 导入 KiCad 工程

![导入 KiCad 工程并预览原理图](docs/screenshots/tab1.png)

### 选择面包板

![选择面包板规格并预览](docs/screenshots/tab2.png)

### 计算布局

![计算元件摆位与跳线](docs/screenshots/tab3.png)

### 使用装配视图

![原理图、面包板和装配清单联动](docs/screenshots/tab4.png)

## 亮点🌟

- 支持深色/高dpi
- 极小安装包体积
- 快捷键支持

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

Release 中还包含用于核对下载文件的 `SHA256SUMS`，以及可以直接体验的 [`KneadNet-examples-0.2.3.zip` 示例压缩包](https://github.com/dfdc1123/knead-net-gui/releases/download/v0.2.3/KneadNet-examples-0.2.3.zip)。

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

最简单的方式是从 GitHub 直接下载 [`KneadNet-examples-0.2.3.zip` 示例压缩包](https://github.com/dfdc1123/knead-net-gui/releases/download/v0.2.3/KneadNet-examples-0.2.3.zip)。解压后，在 KneadNet 中打开其中一个具体工程文件夹即可。

也可以直接浏览仓库中的 [`examples/` 目录](examples/)。各示例的用途参见 [`examples/README.md`](examples/README.md)。

## 平台说明

- **Windows：** 已测试，受支持
- **AUR：** 已测试，受支持
- **其他：** 未测试，欢迎反馈


## 参与贡献

欢迎提交问题报告和范围清晰的 Pull Request。请提供 KneadNet 版本、操作系统、软件包格式、KiCad 版本和复现步骤；只有在许可与隐私允许时才附上最小工程，请勿擅自上传私有原理图。

## 许可证

KneadNet 采用 [GNU General Public License v3.0](LICENSE)，SPDX 标识为 `GPL-3.0-only`。
