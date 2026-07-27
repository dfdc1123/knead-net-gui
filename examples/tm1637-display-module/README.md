# TM1637 display module / TM1637 数码管模块

This KiCad project was converted from the GPL-3.0
[TM1637 display module published by `zhbaa`](https://oshwhub.com/zhbaa/tm1637-shu-ma-guan-mo-kuai)
on OSHWHub and normalized to the project name `tm1637-display-module`. KiCad
local history, nested Git metadata, lock files, and local preference files are
intentionally omitted.

All PCB component pads are through-hole. The project is kept as a manual
example and a regression fixture for physical footprint bounds. In particular,
`LED1` uses `Display_7Segment:CA56-12CGKWA`: its pins occupy a much smaller
area than the display body drawn on `F.Fab`. KneadNet must use the footprint
body outline for rendering and collision checks, with the pin extent only as a
fallback when no usable footprint graphics exist.

## 中文说明

该工程来自 `zhbaa` 在立创开源硬件平台以 GPL 3.0 发布的
[TM1637 数码管模块](https://oshwhub.com/zhbaa/tm1637-shu-ma-guan-mo-kuai)，
由嘉立创 EDA 转换为 KiCad 后将工程名统一为 `tm1637-display-module`。本地历史、
嵌套 Git 数据、锁文件和本机偏好文件不纳入示例。

PCB 中的元件焊盘均为直插。该工程既用于手动测试，也用于封装物理边界回归测试。尤其是
`LED1` 使用 `Display_7Segment:CA56-12CGKWA`：引脚覆盖范围远小于 `F.Fab`
层描述的数码管本体。KneadNet 应使用封装本体外形进行渲染和碰撞检测，只有在找不到可用
封装图形时才回退到引脚范围。

## License / 许可证

The imported project remains attributed to `zhbaa` and distributed under the
upstream project's stated GPL 3.0 license.

导入工程保留对原作者 `zhbaa` 的署名，并遵循原项目标明的 GPL 3.0 许可证。
