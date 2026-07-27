# Infrared transceiver / 红外发射接收模块

This directory contains a KiCad export of the open-source hardware project
[红外发射接收模块_2026-06-08_16-19-02](https://oshwhub.com/xindingsheng/project_xbshrerd)
by [芯鼎盛-硬件吴工 (`xindingsheng`)](https://oshwhub.com/xindingsheng).
The upstream project describes an infrared wireless communication module and
is published under the GPL 3.0 license.

The imported directory and KiCad project have been renamed to
`infrared-transceiver`. KiCad local-history data, lock files, and local
preference files are intentionally not included; the schematic, PCB, project
file, symbol library, and symbol-library table are retained.

## KneadNet compatibility

This is an unsupported-input regression fixture, not a ready-to-layout
KneadNet example. Its PCB currently uses SMD footprints, including the C0603
footprint on `C1`. KneadNet should reject it with an actionable error and must
not crash.

To use the circuit with KneadNet:

1. Open `infrared-transceiver.kicad_pro` in KiCad.
2. Replace every SMD footprint with an appropriate through-hole footprint,
   checking symbol pin numbers against the physical component.
3. Update the PCB from the schematic as needed and save both
   `infrared-transceiver.kicad_sch` and `infrared-transceiver.kicad_pcb`.
4. Import this directory into KneadNet again.

## 中文说明

本目录来自开源硬件项目
[红外发射接收模块_2026-06-08_16-19-02](https://oshwhub.com/xindingsheng/project_xbshrerd)，
原作者为[芯鼎盛-硬件吴工（`xindingsheng`）](https://oshwhub.com/xindingsheng)，
上游页面标注的开源协议为 GPL 3.0。

当前 PCB 默认使用了贴片封装，例如 `C1` 的 C0603 封装，因此它是“不支持输入”的回归
测试工程，不是可以直接计算布局的 KneadNet 示例。正确行为是给出明确错误而不闪退。
如需实际使用，请先在 KiCad 中把所有贴片封装换成合适的直插封装，核对符号引脚号与实物，
更新并保存原理图和 PCB 后再导入 KneadNet。

