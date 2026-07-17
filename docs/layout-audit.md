必须先修的问题

1. 桥接初始化的“选最低成本孔位”实际上永远选候选 0。

[bridge.rs:253](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/cost/bridge.rs:253) 枚举候选时只改 `active_bridge_idx`，但直到循环结束才把 `bridged` 设为 `true`；而 [cost_fast.rs:44](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/cost/cost_fast.rs:44) 只在 `bridged=true` 时读取候选。因此 K 次全量 cost 都在计算同一个 OnBoard 状态，纯属白跑。

现有“选最低 cost”测试没发现，是因为候选 0 恰好已被启发式排在前面。应构造“最优候选明确不是 0”的回归测试，并把 OnBoard 也纳入初始化比较。

2. 默认 greedy 初排的合法性保证是假的。

[from_greedy](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/cost/state.rs:118) 按 R0、未锁 y 搜索，找到位置后才覆盖 `y_locked`；随后 [simulate](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/sa.rs:763) 又事后旋成 R90。原先做过的越界、碰撞、列冲突检查全部可能失效。

GUI 使用 spectral，暂时绕开了一部分；但公开默认配置是 greedy。更糟的是非法初态会立即成为 `best_state`，后续全局合法性检查可能让所有单元件扰动都无法修复它。

3. 已摆元件和已有 wire 基本不参与 SA。

[layout_impl.rs:143](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/layout_impl.rs:143) 排除所有已摆元件：

- 固定 OnBoard 元件完全不进入 cost；
- 固定 Bridged 只注入两个 pin，不注入 body bbox；
- 已有 wire 也不作为障碍。

所以 SA 可以直接覆盖固定元件或桥接 body，最后才在 `validate` 时报错，而且 placement 已经写回，失败不是事务性的。

4. `Placement::Bridged` 可以静默漏脚。

[placement.rs:195](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/placement.rs:195) 只验证“PinId 属于元件”和“两孔不同”，却不验证：

- 两个 PinId 不同；
- 元件恰好有两脚；
- 两个 PinId 完整覆盖元件引脚。

因此同一 PinId 插两个孔、三脚元件只摆两脚，都可能通过 `validate`，随后 cost/router 会把缺失引脚当作不存在。

5. 电源轨绑定没有进入最终合法性验证。

[breadboard.rs:134](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/breadboard.rs:134) 声称错 net 插到绑定电源轨会被 occupancy 捕获，但 [occupancy.rs:214](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/occupancy.rs:214) 只比较真实 pin/wire，从未把 binding 当作 rail 的期望 owner。

手动把 signal/GND 脚插进 VCC rail，可能仍然 `validate == Ok`。这里应增加明确的 `RailBindingConflict`。

6. aggressive bridge init 与配置语义冲突。

[bridge.rs:236](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/cost/bridge.rs:236) 无条件把所有候选设为 Bridged。因此 `p_toggle_bridge=0` 并不是“关闭桥接探索”，而是“全部强制桥接且永远无法翻回”。早期取消、零迭代时也可能直接返回相互碰撞的桥接初态。

建议改成明确策略：

```rust
BridgePolicy::Disabled
BridgePolicy::Explore { initial: OnBoard | BestOfBoth }
BridgePolicy::Forced
```

7. 成本函数依赖元件排列顺序。

[cost_fast.rs:322](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/cost/cost_fast.rs:322) 的列冲突以第一个 net 为基准计数；同一组 `[A,B,B]` 和 `[B,A,B]` 罚分不同。

同文件的 [row_squash](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/cost/cost_fast.rs:355) 边更新 `min_y` 边设置 bitmap，先遇 y=4、后遇 y=2 时会把两个不同 row 都算成 bit 0。相同物理布局仅改变 component 顺序，成本就可能改变。

8. 温度计划实际上不受 `max_iters` 稳定控制。

无效 move 在 [sa.rs:849](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/sa.rs:849) 直接 `continue`，只有成功计算成本后才在 [sa.rs:937](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/sa.rs:937) 降温。桥接比例越高，Flip/ShiftY 等 dead move 越多，实际温度时间尺度越慢。

此外产品三档统一使用 `.99999`：[compute.rs:42](/home/dfdc/Documents/Projects/knead-net-gui/src-tauri/src/compute.rs:42)。假设每步都有效：

- Quick 5000 步：40 → 约 38，几乎没退火；
- Standard 200k：40 → 约 5.4；
- Full 1M：40 → 约 0.0018。

建议由 `T_start/T_end/max_iters` 推导 schedule，按尝试次数推进，而不是暴露一个难以解释的 `cool_rate`。

## 电源轨模型需要显式区分板内连通与外部短接

[PowerRail.groups](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/breadboard.rs:87) 描述同一行中有插孔的 5 孔范围以及 group 之间没有插孔的位置；这些孔位间隔不是底层导体断口。本产品模型中，一条完整电源轨行天然导通，因此同一行的所有 group 属于同一个 `ConductiveIsland`。

[breadboard.rs:311](/home/dfdc/Documents/Projects/knead-net-gui/src/layout/breadboard.rs:311) 当前按 polarity 把同一行以及 top/bottom 都赋成同一个 `rail_id`。其中同一行共享 id 符合物理模型；问题仅在于 top 与 bottom 是两条独立导体，其连通不能伪装成板内短接。如果没有实际 top/bottom 跳线，当前模型会少算 wire，甚至输出电气不通的布局。模型应拆成：

- `ConductiveIslandId`：真实物理短接单元；每条完整电源轨行是一个 island；
- `bound_net`：期望该 island 属于哪个 net；
- `RailTie`：用户明确声明的外部短接。

产品决策记录在 [power-rail-connectivity.md](adr/power-rail-connectivity.md)：400/830 preset 默认各包含两条可显示、可占孔、可验证的 `RailTie`，分别短接 negative 和 positive 的 top/bottom。相邻 5 孔 group 天然导通，不生成 group 间 tie。

## 注释与诊断已按当前实现校正

R9/R11 已使 fast cost、breakdown 和 debug 同源，并补齐 congestion。相关代码注释现只描述当前不变量：SA 原地 mutation + 完整 backup、Bridged 的两脚与 body AABB、绑定 rail 才能生成 bridge catalog、九项成本权重、state-aware move 和 attempt-based schedule。版本演进与旧 RNG 消费策略不再写进源码注释。

Seed 可复现契约定义为“同一算法版本、同一输入 + 同一 seed”。

## 推荐重构顺序

1. 先止血：

   - 修 bridge init 和 Toggle 完整回滚；
   - greedy/spectral 共用 preprocess-aware 初排器并返回 `Result`；
   - 固定 placement/wire 进入 cost；
   - 严格验证 Bridged 引脚和 rail binding；
   - 修排列相关成本；
   - 先验证临时结果，再事务性写回 Layout。

2. 再统一模型：

```rust
struct AnnealProblem {
    fixed_geometry: PlacedGeometry,
    bridge_catalog: BridgeCatalog,
    // immutable precomputed data
}

enum ComponentPose {
    OnBoard { position: Position, rotation: Rotation },
    Bridged { candidate: BridgeCandidateId },
}
```

用它替掉 `is_bridgeable/bridged/bridged_pin_pairs/active_bridge_idx/x/y/rotation` 这一排平行 Vec。桥接候选属于 immutable problem，不应跟着每次 state clone。

3. 重做退火内核：

   - hard constraint 与 soft cost 分离；
   - 只生成当前状态下有效的 move；
   - `ToggleBridge` 和 `ChangeBridgeCandidate` 分开；
   - 加 global relocate、swap、双向 group move；
   - 温度按归一化进度计算；
   - 记录 acceptance/dead/invalid move 比例。

4. 最后优化性能：

   - Toggle 已不再内嵌 K 次全量 cost；
   - release benchmark 保留 rail capacity 预计算、MST length/degree 合并及 cost/legality 共享 collection；
   - catalog 跨 seed、候选邻接、独立 delta cache 未达到收益/风险 gate，不引入；
   - bbox 闭式交集等价但整体收益不足 2%，已按 gate 回退。

完整基准、质量对照和各子项结论见 [layout-performance.md](layout-performance.md)。

## 最优先补的测试

- 最优桥接候选不是 index 0；
- 任意 move reject 后整个状态完全相等；
- 固定 OnBoard、固定 Bridged body、已有 wire 的增量 SA；
- Bridged 重复 PinId、漏脚、三脚元件必须失败；
- 错 net 插入绑定电源轨必须失败；
- cost 对 component permutation 不变；
- 同一 Circuit 更换 power binding 后 eligibility 不残留；
- 真实 PCB 的 parse → prepare → SA → route → 连通图校验。

验证方面，我运行了全 workspace 测试，共 188 个通过；核心 crate Clippy 也通过。没有修改项目文件。测试全绿，但目前主要是在固化已有行为，没有覆盖上述关键不变量。
