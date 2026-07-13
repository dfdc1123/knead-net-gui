# KiCad .kicad_sch → SVG 渲染完整指南

本文档详细描述如何将 KiCad 原理图文件 (`.kicad_sch`) 渲染为 SVG，重点是坐标变换逻辑。
读完本文后，你可以用任意语言（C、Rust、JS...）实现同样的效果。

---

## 1. 文件格式概述

`.kicad_sch` 是 **S-Expression** 格式的文本文件（类 Lisp 语法）。

### 1.1 顶层结构

```lisp
(kicad_sch
    (version 20260306)
    (generator "eeschema")
    (paper "A4")

    (lib_symbols           ; ← 库符号定义（每种元器件的图形模板）
        (symbol "Device:R" ...)
        (symbol "Device:D" ...)
        (symbol "power:GND" ...)
        ...
    )

    (junction (at 160.02 93.98) ...)   ; ← 导线交叉节点
    (wire (pts (xy x1 y1) (xy x2 y2))) ; ← 导线线段
    (symbol                              ; ← 符号实例（放置在原理图上的具体元件）
        (lib_id "Device:R")
        (at 134.62 78.74 180)           ; ← 位置 + 旋转角
        (mirror x)                       ; ← 可选的镜像
        (property "Reference" "R2" ...)
        ...
    )
    ...
)
```

### 1.2 关键坐标字段

| 字段 | 含义 | 示例 |
|------|------|------|
| `(at x y rotation)` | 放置位置 (mm) + 旋转角（度） | `(at 134.62 78.74 180)` |
| `(xy x y)` | 局部坐标点 (mm) | `(xy 1.27 -2.54)` |
| `(mirror x)` | 在 X 轴方向镜像 | 翻转 Y |
| `(mirror y)` | 在 Y 轴方向镜像 | 翻转 X |

---

## 2. S-Expression 解析

用 tokenizer + 递归下降解析器即可。Token 类型只有三种：

```
"quoted string"    →  去掉引号，保留为字符串
(                  →  开始新列表
)                  →  结束当前列表
bareword           →  保留为字符串
```

**正则 tokenizer**：

```python
# 匹配: 引号字符串 | 左括号 | 右括号 | 非空白非括号的原子
tokens = re.findall(r'"[^"]*"|\(|\)|[^\s()"]+', text)
```

**伪代码解析器**：

```
function parse(tokens, pos):
    result = []
    while pos < len(tokens):
        t = tokens[pos]
        if t == '(':
            sublist, pos = parse(tokens, pos + 1)
            result.append(sublist)
        elif t == ')':
            return result, pos + 1
        else:
            result.append(unquote(t))
            pos += 1
    return result, pos
```

---

## 3. 需要提取的数据

### 3.1 库符号 (`lib_symbols`)

#### 单单元符号

大多数符号只有一个单元（电阻、二极管等），子符号命名：
- **`xxx_0_1`** — 本体图形 (unit=0, style=1): polyline, rectangle, circle, arc
- **`xxx_1_1`** — 引脚 (unit=1, style=1): pin

```lisp
(symbol "Device:R"
    ...
    (symbol "R_0_1"
        (rectangle (start -1.016 -2.54) (end 1.016 2.54) ...)   ; 电阻本体
    )
    (symbol "R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "" ) (number "1"))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "" ) (number "2"))
    )
)
```

#### 多单元符号

如 74HC00（四路与非门 + 电源单元），子符号命名规则：**`NAME_UNIT_STYLE`**

```
74HC00_1_1  →  unit=1, style=1  (正常与非门，含本体+引脚)
74HC00_1_2  →  unit=1, style=2  (De Morgan 替代形状)
74HC00_5_0  →  unit=5, style=0  (电源单元，只有 VCC/GND 引脚)
74HC00_5_1  →  unit=5, style=1  (电源单元，矩形本体)
```

- **unit=0**: 所有单元共用（单单元符号的 `R_0_1` 即 unit=0）
- **unit=1~N**: 第 N 个单元
- **style**: body_style，1=正常形状，2=De Morgan 替代

实例通过 `(unit N)` 和 `(body_style N)` 选择对应子符号。

**渲染策略**：
1. 本体图形**只取**匹配 body_style 的子符号（避免两种形状重叠）
2. 引脚**取所有 style**（因为电源单元把引脚和本体分在了不同 style）
3. 引脚按 `(number, at)` 去重


**引脚字段说明**：

```
(pin <电气类型> <形状>         ; 如 passive line, input line, power_in line
    (at x y rotation)          ; 引脚在局部坐标系中的连接点位置和方向
    (length L)                  ; 引脚线长度（从连接点向 rotation 方向延伸）
    (name "pin_name")          ; 引脚名称
    (number "1")               ; 引脚编号
)
```

- 引脚 direction: 0°=右, 90°=上(Y-up)/下(Y-down), 180°=左, 270°=下(Y-up)/上(Y-down)
- 导线的连接点在引脚 `(at)` 位置，引脚线从连接点向 rotation 方向延伸 length 距离

### 3.2 符号实例 (`symbol`)

从顶层 `(symbol ...)` 节点提取：

```
lib_id    ← (lib_id "Device:R")       # 库符号名称
at_x      ← (at x y rotation) 中的 x
at_y      ← (at x y rotation) 中的 y
rotation  ← (at x y rotation) 中的 rotation（度）
mirror_x  ← 是否有 (mirror x) 子节点
mirror_y  ← 是否有 (mirror y) 子节点
properties ← 所有 (property "Key" "Value" ...) 子节点
unit      ← (unit N) 中的 N，默认 1        # 多单元符号用
body_style ← (body_style N) 中的 N，默认 1  # 形状选择
```

### 3.3 导线和节点

- **wires**: `(wire (pts (xy x1 y1) (xy x2 y2) ...))` — 导线路径（可能多个点）
- **junctions**: `(junction (at x y) ...)` — 交叉点

---

## 4. 坐标变换 ⭐ 核心

这是整个程序最关键的部分。错误的变换会导致元器件的方向、翻转、位置全错。

### 4.1 坐标系约定

| 坐标系 | X 轴 | Y 轴 | 用途 |
|--------|------|------|------|
| 库符号局部 | → 右 | ↑ 上 | 符号定义（NPN 集电极在 +Y，发射极在 −Y） |
| 原理图全局 | → 右 | ↓ 下 | 元件放置（+12V 的 Y 值小于 GND） |
| SVG | → 右 | ↓ 下 | 屏幕渲染 |

**关键**：库符号用 **Y-up**（数学惯例），原理图用 **Y-down**（屏幕惯例），两者 Y 轴方向相反。

### 4.2 变换顺序

KiCad 内部的变换顺序（经过验证）：

```
1. Rotate     → 在局部坐标系中逆时针旋转（库空间是 Y-up，所以是 CCW）
2. Mirror     → 对旋转后的坐标做镜像
3. Flip Y     → Y-up 局部 → Y-down 全局
4. Translate  → 平移到放置位置
```

### 4.3 变换矩阵

设：
- 局部坐标: `(lx, ly)` (mm)
- 放置位置: `(at_x, at_y)` (mm)
- 旋转角: `rotation` (度)
- 镜像标志: `mirror_x`, `mirror_y`

#### 步骤 1: 旋转 (CCW)

```
rad = rotation * π / 180
rx = lx * cos(rad) - ly * sin(rad)
ry = lx * sin(rad) + ly * cos(rad)
```

> **注意**：这里**不取负**。KiCad 的 rotation 值定义在 Y-up 库空间中，
> 逆时针旋转是自然方向。否定角度（CW）会导致 rotation=90/270 的元件出错。

#### 步骤 2: 镜像

对**已旋转**的坐标做镜像：

```
if mirror_x:  ry = -ry    # 水平翻转（相对于 X 轴）
if mirror_y:  rx = -rx    # 垂直翻转（相对于 Y 轴）
```

> **注意**：mirror 在 rotation **之后**执行，而非之前。顺序错了会导致
> rotation=90 + mirror_x 的元件（如 D3/D4）方向错误。

#### 步骤 3: Y 轴翻转 + 平移

```
gx = rx + at_x
gy = at_y - ry            # 注意这里是减法！Y-up → Y-down
```

> **为什么是 `at_y - ry`？**
> 库符号在 Y-up 空间中定义（NPN 集电极 Y>0，发射极 Y<0）。
> 全局坐标是 Y-down（+12V 在顶部 = Y 小，GND 在底部 = Y 大）。
> 用 `gy = at_y - ry` 实现 Y-up → Y-down 转换。

### 4.4 完整代码（Python）

```python
import math

def transform_point(lx, ly, at_x, at_y, rotation, mirror_x, mirror_y):
    """库局部坐标 → 原理图全局坐标"""
    # 1. CCW 旋转（在 Y-up 空间中）
    rad = math.radians(rotation)
    cos_r, sin_r = math.cos(rad), math.sin(rad)
    rx = lx * cos_r - ly * sin_r
    ry = lx * sin_r + ly * cos_r

    # 2. 镜像（对旋转后的坐标）
    if mirror_x:
        ry = -ry
    if mirror_y:
        rx = -rx

    # 3. Y 翻转 + 平移
    gx = rx + at_x
    gy = at_y - ry          # ← 注意是减法
    return gx, gy
```

### 4.5 全局坐标 → SVG 坐标

全局和 SVG 都是 Y-down，直接线性映射即可：

```python
SCALE = 10.0  # 像素/mm

def to_svg(gx, gy, origin_x, origin_y):
    sx = (gx - origin_x) * SCALE
    sy = (gy - origin_y) * SCALE    # 不翻转！两者都是 Y-down
    return sx, sy
```

其中 `origin_x = min_x - margin`, `origin_y = min_y - margin`（SVG viewBox 的左上角）。

### 4.6 引脚的特殊处理

引脚的 `(at x y direction)` 中的 direction 也是局部坐标系的角度。

引脚线从连接点 `(x, y)` 向 direction 方向延伸 length 距离：

```python
# 引脚尖端（在局部坐标中）
tip_x = pin_x + pin_length * cos(pin_direction_rad)
tip_y = pin_y + pin_length * sin(pin_direction_rad)

# 然后用相同的 transform_point 变换连接点和尖端
gx1, gy1 = transform_point(pin_x, pin_y, ...)
gx2, gy2 = transform_point(tip_x, tip_y, ...)
```

### 4.7 常见踩坑点

| 问题 | 原因 | 正确做法 |
|------|------|----------|
| 整体上下颠倒 | SVG 映射时多余翻转 Y | `sy = (gy - min_y) * SCALE`，不取负 |
| 二极管方向反 | 用了 CW 旋转（取负角度） | `rad = rotation * π/180`，不取负 |
| 镜像元件方向错 | Mirror 在 Rotate 之前执行 | 先 Rotate，再 Mirror |
| 镜像后引脚位置错 | 对原始坐标镜像而非旋转后坐标 | 对 rx/ry 做镜像 |
| GND/+12V 方向反 | 缺少 Y-flip | `gy = at_y - ry`，不能是 `at_y + ry` |

---

## 5. SVG 渲染

### 5.1 图形元素

库符号中的图形元素直接映射到 SVG：

| KiCad 元素 | SVG 元素 | 注意 |
|------------|----------|------|
| `polyline` | `<polyline>` | 所有点都需经过 `transform_point` |
| `rectangle` | `<polygon>` | 需要用四个角（而非仅 start/end，因为旋转后矩形可能变菱形） |
| `circle` | `<circle>` | 圆心变换即可，半径不受旋转/镜像影响 |
| `arc` | `<path>` (A 指令) | 三点定圆，见 5.1.1 |
| `pin` | `<line>` + `<circle>` | 引脚线 + 连接点圆点 |

#### 5.1.1 Arc（圆弧）

Arc 由三个点定义，常见于与门/与非门的半圆弧：

```lisp
(arc
    (start x1 y1)     ; 起点
    (mid   x2 y2)     ; 圆弧上的一点
    (end   x3 y3)     ; 终点
    (stroke (width w) (type default))
    (fill (type background))   ; background=白色填充遮罩, none=不填充
)
```

**渲染步骤**：

1. start / mid / end 三点经 `transform_point` 变换到 SVG 坐标
2. 三点确定一个圆 — 中垂线法求圆心和半径
3. 用 SVG `<path>` 椭圆弧指令 `A` 绘制

**求圆心（中垂线法）**：

设三点为 S(sx,sy), M(mx,my), E(ex,ey)。

```
弦 SM 的中垂线:  a1·x + b1·y = c1
弦 ME 的中垂线:  a2·x + b2·y = c2

其中:
  a1 = 2·(mx − sx)    b1 = 2·(my − sy)
  c1 = mx² + my² − sx² − sy²
  (a2,b2,c2 同理)

交点即圆心:
  det = a1·b2 − a2·b1
  cx = (c1·b2 − c2·b1) / det
  cy = (a1·c2 − a2·c1) / det
  r  = √((sx−cx)² + (sy−cy)²)
```

**SVG 弧线方向判断**：

```python
# 叉积判断方向
cross_se = (sx−cx)*(ey−cy) − (sy−cy)*(ex−cx)   # start→end
cross_sm = (sx−cx)*(my−cy) − (sy−cy)*(mx−cx)   # start→mid

# mid 在 start→end 的短弧上 → large_arc=0；在长弧上 → large_arc=1
large_arc = 1 if (cross_se * cross_sm) < 0 else 0
sweep = 1 if cross_se >= 0 else 0
```

最终 SVG：

```xml
<path d="M sx,sy A r,r 0 large_arc,sweep ex,ey"
      fill="#ffffff" stroke="#000000" stroke-width="..."/>
```

- `fill="#ffffff"` — 当 `(fill (type background))` 时，白色填充遮住背后线条
- `fill="none"` — 当 `(fill (type none))` 时，仅绘制弧线

### 5.2 线宽处理

KiCad 中线宽为 0 表示"使用默认值"（通常 0.254mm）。渲染时需给一个最小可见宽度：

```python
stroke_width = max(line_width * SCALE, 1.0)  # 至少 1 像素
```

### 5.3 导线绘制

导线定义为折线段。每段渲染为一个 `<line>`：

```python
for i in range(len(wire_points) - 1):
    x1, y1 = to_svg(*wire_points[i])
    x2, y2 = to_svg(*wire_points[i + 1])
    svg.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" .../>')
```

### 5.4 文本标注

符号的 `Reference` 和 `Value` 属性的位置已在全局坐标中（由 EEschema 计算），直接映射到 SVG 即可：

```python
if not property.hide:
    tx, ty = property.at_x, property.at_y  # 已是全局坐标
    sx, sy = to_svg(tx, ty)
    svg.append(f'<text x="{sx}" y="{sy}">...</text>')
```

---

## 6. 完整渲染流程

```
.kicad_sch 文件
    │
    ├─[1] S-Expression 解析 → 嵌套列表树
    │
    ├─[2] 提取 lib_symbols → {名称: {graphics: [...], pins: [...]}}
    │
    ├─[3] 提取 junctions → [(x, y), ...]
    │
    ├─[4] 提取 wires → [[(x1,y1), (x2,y2), ...], ...]
    │
    ├─[5] 提取 symbol instances → [{lib_id, at_x, at_y, rotation, mirror_x, mirror_y, properties}, ...]
    │
    ├─[6] 计算包围盒 → min_x, max_x, min_y, max_y (+ margin)
    │
    ├─[7] 绘制背景网格
    │
    ├─[8] 绘制导线 (wires)
    │
    ├─[9] 绘制节点 (junctions)
    │
    └─[10] 渲染每个符号实例:
              ├─ 对每个图形元素: 局部坐标 → transform_point() → to_svg()
              ├─ 对每个引脚: 同上
              └─ 文本标注: 全局坐标 → to_svg()
```

---

## 7. 数据类型参考

### 7.1 库符号字典结构

库符号按 **(unit, body_style)** 两级索引存储：

```python
lib_symbols = {
    # 单单元符号
    "Device:R": {
        0: { 1: [                                     # unit=0, style=1 = 本体
            {"type": "rectangle", "start": (x,y), "end": (x,y)},
            {"type": "arc", "start": ..., "mid": ..., "end": ...},
        ]},
        1: { 1: [                                     # unit=1, style=1 = 引脚
            {"type": "pin", "at": (x,y,rot), "length": L, "number": "1"},
        ]},
    },
    # 多单元符号
    "74xx:74HC00": {
        1: {                                          # 单元 1
            1: [{...}, ...],                          # style 1 = 正常形状
            2: [{...}, ...],                          # style 2 = De Morgan
        },
        5: {                                          # 单元 5 = 电源
            0: [{"type": "pin", ...}],                # style 0 = 引脚
            1: [{"type": "rectangle", ...}],          # style 1 = 本体
        },
    },
}
```

渲染时：本体取匹配 body_style 的，引脚取所有 style 的并去重。

### 7.2 符号实例字典结构

```python
symbol_instance = {
    "lib_id": "Device:R",
    "at_x": 134.62,       # mm
    "at_y": 78.74,        # mm
    "rotation": 180,      # 度
    "mirror_x": False,
    "mirror_y": False,
    "properties": {
        "Reference": {"value": "R2", "hide": False, "at": [x, y, rot]},
        "Value":    {"value": "10k", "hide": False, "at": [x, y, rot]},
        ...
    }
}
```

---

## 8. 变换速查表

| 场景 | 变换 |
|------|------|
| 旋转 θ | `(lx·cosθ − ly·sinθ, lx·sinθ + ly·cosθ)` |
| mirror_x | `ry = −ry` |
| mirror_y | `rx = −rx` |
| Y-up → Y-down | `gy = at_y − ry` |
| 全局→SVG | `sx = (gx − origin_x) × SCALE`, `sy = (gy − origin_y) × SCALE` |
| 执行顺序 | Rotate → Mirror → Flip Y → Translate |

---

## 9. 验证方法

用以下方法验证变换是否正确：

1. **引脚对齐测试**: 检查每个元件的引脚变换后的全局坐标是否与导线端点坐标一致（差值 < 0.01mm）
2. **二极管方向**: 旋转 270° 的二极管，阴极横线应在顶部（gy 较小）
3. **NPN 方向**: 集电极在顶部（gy 较小），发射极在底部（gy 较大）
4. **GND 方向**: 接地符号线条在连接点下方（gy 较大）
5. **mirror_y**: Q4/Q6 的基极应在右侧而非左侧

---

## 10. 完整示例

目录中的 `render_sch.py` 是一个完整可运行的实现（约 620 行 Python），可作为参考。

运行方式：
```bash
python3 render_sch.py [input.kicad_sch] [output.svg]
```
