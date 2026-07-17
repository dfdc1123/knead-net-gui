# ADR: 面包板电源轨的物理连通模型

- 状态：Accepted
- 日期：2026-07-15
- 关联：`docs/layout-repair-plan.md` R0 / A9

## 背景

本产品所建模的面包板中，同一行电源轨由一条连续导体构成，因此该行上的所有孔天然导通。几何上的 5 孔 group 和 group 之间没有可插孔的位置，但这只是孔位与视觉分组，不是底层导体的断口。顶部和底部的同极性电源轨则是两条独立导体，不天然导通。

当前实现按 polarity 给 top 与 bottom 的所有同极性电源轨孔分配同一个 `rail_id`。同一行连通符合产品物理模型，但 top/bottom 在没有跳线时也被 cost、router 和 validation 当作零成本连通，这部分是隐式的产品假设，无法显示、占孔或验证。

需要在两个产品方案中作出决定：

- **A：物理默认。** 每条完整电源轨行是一个 `ConductiveIsland`，top 与 bottom 分别独立；仅显式 `RailTie` 能连接两行。优点是开箱状态完全符合裸板，且不会凭空假设用户接线；缺点是默认布局会改变当前产品一直采用的“同极性 top/bottom 已短接”体验。
- **B：产品便利默认。** 使用与 A 相同的独立 island 物理底模，但 400/830 preset 为每种 polarity 默认带一条显式 top/bottom `RailTie`，表示产品假设用户会按图短接同极性上下轨。优点是保留当前的有效连通语义，同时所有假设都可见、可序列化、可占孔并可验证；代价是默认结果中会出现两根真实跳线，用户删除 tie 后上下轨连通性也随之改变。

## 决策

采用 **B**。A 不是被否定的物理模型，而是所有板型的底层事实；B 只是在 preset 实例上显式添加默认对象。top/bottom 的连通不得再通过共享 `rail_id`、相同 polarity 或其他隐式规则产生。同一行跨 5 孔 group 的连通是板内连续导体的固有属性，不需要 `RailTie`。

### 1. `ConductiveIsland`

`ConductiveIsland` 是由面包板内部一片连续导体天然短接的最大孔集合。集合内任意两孔无需外部导线即可导通；两个不同 island 之间默认不导通。

- 主插孔区中每个真实的 5 孔纵向铜片是一个 island。
- 电源轨中每一条完整的 `PowerRail` 行是一个 island。`PowerRail.groups` 仅列出该连续导体上存在插孔的 x 范围；同一行的相邻 group 天然导通，不按 group 拆分 island。
- top 与 bottom 是不同的 `PowerRail` 行，因此永远属于不同 island；polarity 是显示及默认绑定提示，不是 top/bottom 连通关系。
- island 标识只描述板内铜片，不因 `RailTie` 的增删而合并或重新编号。原 `rail_id` 若继续存在，只能作为 island id，不能表示 effective connectivity。

### 2. `bound_net`

`bound_net` 是对一个 `ConductiveIsland` 的可选 net 归属约束：该 island 为空时，它预留给该 net；被占用时，所有进入该 island 的 pin、普通 wire 端点和 tie 必须与该 net 一致。它不是虚拟 pin、不是自动生成的导线，也不单独建立两个 island 间的连通。

绑定通过 `RailTie` 的连通闭包传播：同一 effective component 内所有非空 `bound_net` 必须相同；只有一个非空值时，整个 component 的有效绑定为该值；全部为空时则未绑定。按 polarity 选择 VCC/GND 的 UI 操作必须展开为对目标 islands 的明确绑定，不能把 polarity 本身当作绑定。

### 3. `RailTie`

`RailTie` 是用户可观察的、端点分别位于两个不同 `ConductiveIsland` 的外部导体。它表示真实安装的跳线或等效短接片，具有稳定 id、两个 `HoleId` 端点、来源以及可选标签。

- `source` 为 `preset` 或 `user`；二者电气语义相同，区别仅用于 UI、重置默认值和迁移。
- tie 的两个端点必须存在、不同、属于不同 island，且各自占用真实孔位；不能与元件 pin、普通 wire 端点或另一 tie 端点重复占孔。
- tie 不拥有独立 net。它连接的 effective component 决定其 net；连接两个互斥 `bound_net` 或已被不同 net 占用的 component 是硬错误。
- tie 是布局中的固定几何，不由 router 移动、删除或悄悄替换。用户显式编辑 tie 时才改变它。

### 4. 400 与 830 preset 的默认 ties

400 与 830 preset 都有四个电源轨 island：top negative、top positive、bottom negative、bottom positive。同一行内部天然导通，无须也不得生成 group 间 tie。preset 只为每种 polarity 添加一条 top/bottom tie，使两个 negative island 成为一个 effective component、两个 positive island 成为另一个 effective component。

端点规则固定为使用对应 top/bottom 行最左侧的可用孔：

| preset | negative top/bottom | positive top/bottom | 合计 |
| --- | ---: | ---: | ---: |
| 400（30 列） | 1 | 1 | 2 |
| 830（63 列） | 1 | 1 | 2 |

参数化列宽或末尾不足 5 孔不会改变数量：每种 polarity 在 top 和 bottom 均有至少一个孔时生成一条 top/bottom tie。170 preset 没有电源轨，因此没有默认 tie。

默认 tie 是 preset 数据的一部分，但不是不可删除的板内铜片。创建布局时将其物化为布局对象；用户可以删除、改接或恢复 preset 默认值。

### 5. 默认连通边界

- 裸板物理模型中 top/bottom **不连接**；400/830 产品 preset 仅因上述两条显式 top/bottom `RailTie` 而默认连接。
- 同一行的相邻 5 孔组 **天然连接**，不需要默认 tie；删除任何 `RailTie` 都不能改变这一板内连通事实。
- 删除某种 polarity 的 top/bottom tie 后，该极性的上下两行必须立即断开。不得按 polarity、preset 名称或历史 `rail_id` 补回隐式连接。

### 6. GUI 展示与编辑

GUI 在板图上把 tie 画成 top 与 bottom 真实端点间的跳线，并与 router 生成的普通 net wire 使用可区分但不喧宾夺主的样式。preset tie 带“默认上下电源轨短接”标记；hover/选中时显示 id、两端 island、source 和有效 net/binding。图例必须明确：同一行色带覆盖的各个 5 孔 group 天然导通；相同 polarity 不代表 top/bottom 天然导通，只有画出的 tie 才建立上下连接。

tie 可被选中、删除和重新指定端点；删除前后都要实时刷新连通高亮与 validation。提供“恢复 preset 默认 ties”操作。导出、截图和结果预览必须包含 tie，不能只在编辑态显示。若 tie 非法，GUI 标出具体端点和冲突原因，而不是继续以隐式连通计算。

### 7. Serialization

持久化格式提升为带版本的布局 schema。island 由板型几何派生，不重复保存；tie 与逐 island binding 显式保存。规范形态如下（字段名是契约，id 的具体编码可由实现 ADR 补充）：

```json
{
  "schema_version": 2,
  "preset": "400",
  "cols": 30,
  "rail_ties": [
    {
      "id": "preset:negative:top-bottom",
      "from_hole": 0,
      "to_hole": 25,
      "source": "preset",
      "label": "default power-rail tie"
    }
  ],
  "island_bindings": [
    { "island": "power:top:positive", "bound_net": "net-id" }
  ]
}
```

`rail_ties` 即使与 preset 默认值完全相同也必须完整写出；不能用“缺省表示全接通”。这保证删除一条默认 tie 可以无歧义地 round-trip。加载时先由 `preset + cols` 重建稳定 island/hole 映射，再解析 binding 和 tie，最后做完整 validation。未知端点、重复 id、重复占孔、同 island tie 或 binding 冲突均拒绝加载，不做猜测性修复。

### 8. Effective connectivity 的统一计算

cost、router 和 validation 必须消费同一个派生的 connectivity view，禁止各自用 polarity 或 `rail_id` 实现捷径。计算过程为：

1. 每个 `ConductiveIsland` 建立一个节点；电源轨同一行的所有 group 已属于同一个节点，板内连通为固有零长度关系。
2. 每条通过结构与占孔检查的 `RailTie` 在两个 island 节点间加边。
3. 对该图求连通分量（例如 union-find）；分量即 effective component。
4. 汇总 component 内的 `bound_net`、pin net 和普通 wire 所属 net；出现两个不同 net 即硬冲突。

具体消费者语义：

- **cost**：同一 effective component 内的电气距离为 0；跨 component 按真实候选 jumper 距离计费。preset tie 本身是固定、已存在几何，不重复计入待路由 MST，也不因免费连通而消失。
- **router**：把同一 effective component 作为一个已连通集合，避免生成重复跨接；只能使用仍为空的孔作为新 wire 端点，并把 tie 端点视为已占用。router 不自动创造 `RailTie`；需要连接不同 component 时生成普通 routed wire，除非用户明确执行“创建 tie”。
- **validation**：先验证 tie 的端点和占用，再验证 effective component 的单 net/binding 一致性。任何非法 tie 都不能参与闭包，且整个布局返回结构化错误；不能在忽略坏 tie 后继续写回“部分有效”结果。

### 9. 旧布局迁移

旧模型把同极性所有电源轨隐式视为短接。为保持旧文件的电气结果，缺少 `schema_version` 或版本为 1 的布局在加载时执行一次确定性迁移：

1. 按旧文件的 preset 和 cols 重建独立 islands；每条完整电源轨行重建为一个 island。
2. 对 400/830 物化本 ADR 第 4 节规定的全部默认 ties，标记 `source: preset`；170 不生成。
3. 旧的 polarity-level power binding 展开到该 polarity 的 top 与 bottom 两个 island。由于默认 tie 已把它们连成一个 component，语义与旧模型一致。
4. 用新规则验证 placements、wires、ties 和 bindings；若旧布局占用了某个默认 tie 端点或暴露出不同 net 冲突，迁移失败并报告可操作的端点冲突，不静默换孔或删除对象。
5. 仅在用户保存后写出完整 v2 数据；读取本身不覆盖原文件。再次加载 v2 时不得重新追加 preset ties。

该迁移优先忠实保留旧实现的“同极性 top/bottom 已短接”语义。若用户过去实际没有安装上下轨跳线，旧文件没有信息可以恢复这种意图；迁移后用户需显式删除对应 top/bottom tie，随后保存为 v2。

## 影响

此决策把“板内铜片”“net 归属”和“用户安装的短接线”拆成三个独立概念。产品仍可默认提供全轨供电便利，但 GUI、持久化和所有算法都会面对同一组真实对象。实现阶段需要替换当前同极性共享 `rail_id` 的行为，并为默认 tie 的占孔冲突设计清晰的首次迁移反馈；这些代码与测试变更不属于本 ADR 修改。
