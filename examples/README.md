# KneadNet example projects

These KiCad projects are provided for learning the KneadNet workflow and for manual smoke testing. Select an individual project directory—not the parent `examples/` directory—when opening it in KneadNet.

| Directory | Contents | Release bundle |
| --- | --- | --- |
| `NE555+CD4017/` | KiCad project using NE555 and CD4017 devices | Yes |
| `SNx4HC00/` | KiCad project using an SNx4HC00 device | Yes |
| `h-bridge/` | H-bridge KiCad project | Yes |
| `lm741/` | LM741 KiCad project | Yes |
| `h-bridge_different_order/` | Developer fixture for component-order regression tests | No |

Each public project includes `.kicad_pro`, `.kicad_sch`, and `.kicad_pcb` files. KneadNet requires the PCB file; the same-name schematic provides the preview and cross-selection.

Generated layouts are suggestions. Verify component orientation, pin numbering, power rails, and every connection before applying power.

## 中文说明

这些 KiCad 工程用于学习 KneadNet 的操作流程和手动烟测。在 KneadNet 中应选择某个具体工程文件夹，而不是父级 `examples/` 目录。

`NE555+CD4017`、`SNx4HC00`、`h-bridge` 和 `lm741` 会进入 Release 示例包。`h-bridge_different_order` 用于验证改变元件迭代顺序不会改变布局成本，是开发测试夹具，不作为面向用户的示例发布。

通电前必须人工检查元件方向、引脚编号、电源轨和每一条连接。

## License

The examples are distributed with KneadNet under [`GPL-3.0-only`](../LICENSE).
