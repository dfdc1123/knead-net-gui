# KneadNet example projects

These KiCad projects are provided for learning the KneadNet workflow and for manual smoke testing. Select an individual project directory—not the parent `examples/` directory—when opening it in KneadNet.

| Directory | Contents | Release bundle |
| --- | --- | --- |
| `NE555+CD4017/` | Developer fixture for schematic rendering tests | No |
| `SNx4HC00/` | KiCad project using an SNx4HC00 device | Yes |
| `h-bridge/` | H-bridge KiCad project | Yes |
| `lm741/` | LM741 KiCad project | Yes |
| `h-bridge_different_order/` | Developer fixture for component-order regression tests | No |
| `tm1637-display-module/` | THT TM1637 display fixture with an oversized seven-segment body | No |

Each public project includes `.kicad_pro`, `.kicad_sch`, and `.kicad_pcb` files. KneadNet requires the PCB file; the same-name schematic provides the preview and cross-selection.

Generated layouts are suggestions. Verify component orientation, pin numbering, power rails, and every connection before applying power.

## 中文说明

这些 KiCad 工程用于学习 KneadNet 的操作流程和手动烟测。在 KneadNet 中应选择某个具体工程文件夹，而不是父级 `examples/` 目录。

`SNx4HC00`、`h-bridge` 和 `lm741` 会进入 Release 示例包。`NE555+CD4017`
用于验证 NE555 特有的上划线、共享电源引脚和引脚文字渲染；
`h-bridge_different_order` 用于验证改变元件迭代顺序不会改变布局成本。
这两个目录都是开发测试夹具，不作为面向用户的示例发布。

`tm1637-display-module` 是一个直插 TM1637 数码管工程，同时用于验证元件占位框来自 KiCad
封装的物理外形，而不只是最外圈引脚。它来自 `zhbaa` 以 GPL 3.0 发布的
[TM1637 数码管模块](https://oshwhub.com/zhbaa/tm1637-shu-ma-guan-mo-kuai)。请参阅
[`tm1637-display-module/README.md`](tm1637-display-module/README.md)。

通电前必须人工检查元件方向、引脚编号、电源轨和每一条连接。

## License

KneadNet-authored examples are distributed under [`GPL-3.0-only`](../LICENSE).
The imported `tm1637-display-module` example remains attributed to `zhbaa`
under its upstream GPL 3.0 license.
