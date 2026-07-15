# Layout 审查复核与修复计划

## 复核基线

- 复核目标：当前 `HEAD` `bd4a3ce032f3568cef6f3a2540498f13075c8d26`（2026-07-15）。
- `src/layout/**` 和 `src-tauri/src/compute.rs` 相对 `HEAD` 无工作区修改。
- `docs/layout-audit.md` 在复核时是未跟踪文件；本文件把它当作待验证的审查输入，不把它当作 `HEAD` 的一部分。
- 本轮没有修改生产代码或测试代码。
- 已执行：
  - `cargo test -p knead-net layout::`：167 passed。
  - `cargo test -p knead-net-gui compute::tests::compute_profiles_map_to_distinct_backend_configs`：1 passed。
  - `cargo test --workspace`：188 passed（171 core + 15 GUI + 2 integration）。
- 全绿不表示报告中的问题不存在。现有测试明确固化了“全部 bridgeable 初始 Bridged”和三个 GUI profile 共用 `cool_rate = 0.99999`，但没有覆盖非零最优 bridge 候选、完整 reject 回滚、固定几何、rail binding 最终验证或事务写回。

状态定义：

- **已经确认**：当前代码可由直接控制流、确定性反例或现有行为证明。
- **部分确认**：机制存在，但报告中的产品语义或影响范围还依赖尚未作出的产品决定。
- **无法复现**：按当前入口和确定性 fixture 未看到报告所述行为。
- **审查已过时**：当前 `HEAD` 已不再包含报告所述实现。

本次结果：主问题 1、2、3、4、5、7、8 均已经确认；主问题 6 部分确认；电源轨模型中 top/bottom 被隐式合并的问题已经确认，但原报告把同一行 5 孔 group 间的孔位间隔误判为导体断口，现已纠正；注释问题中的客观矛盾已经确认、风格判断部分确认。另行确认了报告在推荐顺序/测试清单中提到的 Toggle reject 不完整回滚、非事务 write-back 和 bridge eligibility 残留。没有“无法复现”或“审查已过时”的主问题。

## 当前实际调用链

### GUI 正常计算

```text
start_compute
  -> run_compute
     -> parse_pcb
     -> prepare_for_layout_with_power_nets
        -> auto_mark_bridgeable（原地修改 Circuit.components[].bridgeable）
        -> Breadboard::with_power_rail_binding
     -> Layout::new
     -> ComputeProfile::config（use_spectral=true, T0=40, cool_rate=.99999）
     -> Layout::place_sa_with_progress_and_cancellation
        -> place_sa_impl
           -> 收集尚未 placement 的 placeable
           -> preprocess_for_breadboard（扫描整个 Circuit）
           -> 每个 seed: sa::simulate
           -> 按返回 cost 选全局 best
           -> 发 PlacementComplete 快照
           -> 写入 Layout.placements
           -> Layout::validate -> Occupancy::from_layout
     -> route_with_progress
        -> 清空旧 wires
        -> Occupancy::from_layout
        -> PathFinderRouter::route
        -> 写入新 wires
        -> Layout::validate
```

GUI 每次新建 `Layout`，所以“已有 placement/wire 不参与 SA”当前主要伤害库 API、增量布局和将来恢复/编辑流程；它不是当前 GUI 新算一遍的常见入口。其他主问题仍会进入 GUI。特别是 `from_spectral` 在 placeable 数量 `<= 2` 时回退到 `from_greedy`，所以 GUI 并没有完全绕开 greedy 初始化问题。

### 单个 seed 的初始化

```text
simulate
  -> from_spectral 或 from_greedy
  -> 事后写 r90_only / rotation=R90 / y_locked
  -> populate_bridgeable_info
  -> SAContext::new
  -> fill_bridged_bboxes（SA bridge 候选 + 外部 Bridged pin + power anchor）
  -> init_bridgeable_to_bridged（无条件 aggressive init）
  -> cost_fast
  -> best_state = state.clone()
  -> 进入 0..max_iters
```

因此 `max_iters=0` 并不表示“不改变初排”：spectral/greedy、预处理事后覆盖、bridge catalog 和 aggressive bridge init 全都会执行，随后这个状态直接成为 `best_state`。

### 正常 move、reject 与降温

```text
random_move
  -> apply_move（原地修改并返回 Backup；dead move 返回 None）
  -> state_y_valid（检查所有 state.y，不是完整几何合法性）
  -> state_onboard_pins_in_bounds
  -> 若 OnBoard -> Bridged Toggle：遍历 bridge 候选
  -> cost_fast
  -> Metropolis accept/reject
     -> accept: 保留原地状态，必要时 clone 到 best_state
     -> reject: Backup::revert
  -> T *= cool_rate
```

三类 `continue` 都跳过降温：`apply_move == None`、`state_y_valid == false`、`state_onboard_pins_in_bounds == false`。正常完成 cost 计算的 move 无论 accept/reject 才降温。

普通 `Flip`、`ShiftX`、`ShiftY`、`ShiftGroup` 的备份覆盖了它们实际写入的字段。`ToggleBridging` 是例外：OnBoard -> Bridged 时，候选扫描还会改 `active_bridge_idx`，但备份只保存 `old_bridged`；若 Metropolis reject，只恢复 `bridged`。

### cancel 与 zero iteration

- `cancel_compute` 只设置共享 `AtomicBool`。
- `place_sa_impl` 会跳过尚未开始且不是展示 seed 的任务，但展示 seed 必须运行以保证结果非空。
- `simulate` 在完成全部初始化和第一次 cost 之后，才在迭代循环顶部检查 cancellation。
- 因而“调用前已经 cancel”和 `max_iters=0` 都会返回 aggressive bridge 初态；如果它非法，调用方仍先收到 `PlacementComplete`，随后 placement 被写入，再由 `validate` 返回错误。
- GUI 只有在 placement 验证成功后才继续 routing；但失败前已经发布过完成快照，`Layout` 自身也已经被改写。

### 已有 placement / wire

```text
place_sa_impl
  -> 已 placement 的 component 从 placeable 排除
  -> Layout::bridged_pins 只展开已有 Bridged 的两个 pin
  -> base_placements 只用于进度快照
  -> simulate/cost 只接收 placeable + bridged_pins
```

- 已有 OnBoard：pin、bbox、net 全部不进 SA 初排和 cost。
- 已有 Bridged：两个 pin 进入 cost；body bbox 不进入。
- 已有 wire：不进初排、不进 cost，也不进 SA progress snapshot。
- 最终 `Occupancy::from_layout` 才把所有 placement bbox、pin 和 wire 端点放到同一占用表中检查。

## 逐项结论

### A1. Bridge 初始化永远选择候选 0

状态：**已经确认**。

实际链：`simulate` -> `init_bridgeable_to_bridged` (`src/layout/cost/bridge.rs:253`) -> 循环只写 `active_bridge_idx[i]` -> `cost_fast` (`src/layout/cost/cost_fast.rs:44`) 仅在 `state.bridged[idx]` 为真时读取该候选 -> 循环结束后才写 `bridged[i] = true`。

每个 `j` 的成本调用看到的都是同一个 OnBoard 元件；严格 `<` 只会保留第一个候选。之前已经初始化为 Bridged 的其他元件会参与这些 cost，但当前元件的候选仍不参与，所以结论不受多元件顺序影响。

受影响路径：所有含 bridgeable 元件的 seed 初始化、`max_iters=0`、预取消、正常退火起点、Quick/Standard/Full。现有 `init_bridgeable_to_bridged_picks_lowest_cost_pair` 在初始化完成后才重新比较候选，且 fixture 的 index 0 恰好最优，无法捕捉该错误。

最小失败测试 `T01_bridge_init_selects_nonzero_candidate`：手工把两个合法候选排成“index 1 无碰撞且成本严格更低”，调用 init 后断言 `active_bridge_idx == 1`。当前实现稳定得到 0。

### A2. Greedy 初排的合法性保证失效

状态：**已经确认**。

实际链：`SAState::from_greedy` (`src/layout/cost/state.rs:125`) 按 R0 和搜索到的 `try_y` 检查 bbox、越界、碰撞、列冲突；找到后才用 `preprocess.y_locked` 覆盖 `fy`。`simulate` (`src/layout/sa.rs:763`) 随后再把 `r90_only` 旋成 R90 并再次覆盖 y。搜索阶段证明的几何与最终初态不是同一个几何。

`grid_fill_2d` 会在搜索时使用 R90 和 y lock，因此 placeable `> 2` 的 spectral 路径没有这个特定错位；但 `from_spectral` 对 `n <= 2` 直接调用 `from_greedy`，GUI 小电路仍受影响。公开 `SAConfig::default()` 使用 greedy，也直接受影响。

受影响路径：默认库 API、小于等于两个待摆元件的 GUI/spectral、已有 placement 使剩余 placeable 降到两个以内、zero iteration、早期 cancel。非法初态立即成为 `best_state`；bbox/列冲突只作为 soft cost，不能保证一次单元件 move 能离开非法盆地。

最小失败测试 `T02_greedy_applies_preprocess_before_search`：两脚 footprint 位于 `(0,0)`、`(0,3)` 且不同 net，使 preprocess 标记 R90-only；R0 first-fit 会选 `x=0`，事后 R90 令第二脚到 `x=-3`。断言初始化返回 `Result::Ok` 且所有 pin/bbox 合法。当前路径产生 OOB 初态。

### A3. 已有 placement 和 wire 不构成 SA 固定几何

状态：**已经确认**。

实际链：`Layout::place_sa_impl` (`src/layout/layout_impl.rs:143`) 排除已有 placement；`bridged_pins` 仅注入已有 Bridged pin；`SAContext::new` 只为 `state.placeable` 建 `CompInfo`；`cost_fast` 的 bbox 列表只来自这些 state 元件，外部 Bridged 分支只追加 pin；没有接收 `Layout::wires` 的参数。

受影响路径：

- 固定 OnBoard 可被新元件的 pin 或 body 完全覆盖；MST 也看不到它的 net 端点。
- 固定 Bridged 的孔位会参与 pin/rail/MST，但它的 body 可被覆盖。
- 已有 wire 的两个插孔可被新元件覆盖。
- 最终 `validate` 会发现一部分几何冲突，但发生在 write-back 之后；如果固定对象只影响优化目标而不形成最终冲突，SA 的选优仍然错误。
- GUI 当前从空 `Layout` 开始，所以这一项在 GUI fresh run 上暂不触发；公开增量 API 和未来编辑/恢复路径受影响。

最小失败测试 `T03_fixed_geometry_blocks_initial_and_sa_moves`：表驱动三种 fixture——固定 OnBoard bbox、固定 Bridged body、已有 wire endpoint——各加一个未摆元件并使用 `max_iters=0`。期望得到不覆盖固定几何的合法结果；当前实现会重叠并在 write-back 后报错，或忽略固定 net 对成本的影响。

### A4. `Placement::Bridged` 可以漏脚或重复脚

状态：**已经确认**。

实际链：`Placement::apply` Bridged 分支 (`src/layout/placement.rs:195`) 只检查两个 PinId 各自属于 component、HoleId 存在、两个 hole 不同。它不检查 PinId 互异、component 恰好两脚、集合完整相等。`Occupancy::from_layout` 信任 `PlacedFootprint.pin_holes`；`Layout::bridged_pins` 也原样展开；router 的 `PinId -> HoleId` HashMap 对重复 PinId 会覆盖其中一个位置，未列出的真实 pin 根本不会出现。

受影响路径：手工 placement、从持久化数据恢复的 placement、最终 validation、cost、routing、进度/结果渲染。SA 候选生成有 `debug_assert_eq!(pins.len(), 2)`，但 public `Layout::place` 不受该保护。

最小失败测试 `T04_bridged_requires_exact_two_pin_bijection`：

1. 两个不同孔都写同一个 PinId；
2. 三脚 component 只写其中两脚；
3. 两脚 component 重复一脚从而漏另一脚。

三种都必须得到明确的 bridge placement 错误。当前三种可通过单 placement 验证（只要 hole 不同且所写 PinId 属于 component）。

### A5. Power-rail binding 没有进入最终 legality

状态：**已经确认**。

实际链：binding 被 cost (`cost_fast:216`) 和 router (`routing.rs:154`) 作为虚拟 anchor 注入；但 `Layout::validate` -> `Occupancy::from_layout` 只把真实 pin/wire 放入 `by_rail` (`occupancy.rs:59`)。rail 冲突检查 (`occupancy.rs:214`) 没有加入绑定所要求的 owner，因此一个或多个同为 signal/GND 的真实 endpoint 可以独占 VCC rail 并通过 validation。

受影响路径：手工 OnBoard/Bridged、已有 wire、持久化布局、zero-iteration/final validation。SA 自己生成的 bridge 候选只扫 matching rail，且 SA cost 有虚拟 anchor，所以 fresh SA 通常被软成本保护；这不能替代 public validation 的硬合法性。

最小失败测试 `T05_bound_rail_rejects_wrong_net_endpoint`：正极绑定 VCC，只放一个 SIGNAL pin 到正极轨孔，断言 `Layout::validate` 返回 `RailBindingConflict`（或等价的专用错误）。当前 `by_rail` 只有一个 endpoint，返回 `Ok`。

### A6. Aggressive bridge init 与配置语义

状态：**部分确认**。

机制已经确认：`simulate` 无条件调用 `init_bridgeable_to_bridged`，`p_toggle_bridge=0` 只让 `random_move` 不产生 Toggle，结果是全部可桥接元件从初始化起保持 Bridged；zero iteration 和预取消也返回这个状态。init 没有 hard-legality gate，多元件 bridge body/pin 可以相撞，最后才 validation。

“`p_toggle_bridge=0` 应等于完全关闭桥接”属于尚未写进现有字段契约的产品语义。当前注释把它定义为“关闭 Toggle 区间”，所以报告提出的 `BridgePolicy` 是合理且必要的消歧，但不能把旧字段名本身当作已经承诺的 Disabled policy。

受影响路径：配置 API、所有 seed 初始化、zero iteration、预取消、低迭代 profile、固定 bridgeable 集合。

最小失败测试 `T06_bridge_policy_controls_zero_and_cancel`：先用现有 API 写 `p_toggle_bridge=0 + max_iters=0` 应保持 OnBoard 的行为断言，当前会稳定失败；引入明确 policy 后把该意图迁到 Disabled，并继续断言 Explore/BestOfBoth 不选比 OnBoard 更差或 hard-illegal 的 bridge、Forced 遇到互撞不能返回成功、预先 cancel 返回最近的完整合法初态而不是部分/aggressive 初态。

### A7. Cost 依赖 component 顺序

状态：**已经确认**。

实际链：

- `cost_fast:322` 取 `rail_owners[0]` 为 base，只统计后续与 base 不同的 owner。`[A,B,B]` 得 2，`[B,A,B]` 得 1。
- `cost_fast:355` 的 row bitmap 在遍历中同时降低 `min_y`。先看 y=4 再看 y=2 时，两项都写 bit 0；反序会写 bit 0 和 bit 2。
- `cost_breakdown_inner` 复制了同样两段逻辑，所以诊断也顺序相关。

受影响路径：初始化候选比较、每次 move 的 delta、Metropolis 接受、全局 best seed、最终 cost 报告和 expensive-seed 诊断。仅改变 `SAState.placeable` 及其平行数组的排列就可能改变结果，违反硬不变量。

最小失败测试 `T07_cost_is_invariant_under_state_permutation`：

- 权重只开 column conflict，对同一物理 `[A,B,B]` 的两个 state permutation 断言成本相等；
- 权重只开 row squash，对 y=4/y=2 的两个 permutation 断言成本相等；
- 同时断言 `cost_fast == cost_breakdown.total`。

当前前两项稳定失败。

### A8. 温度计划不由 `max_iters` 稳定决定

状态：**已经确认**。

实际链：`sa.rs:849`、`:861`、`:866` 三类失败 move 都 `continue`，只有走到 `sa.rs:937` 才执行 `t *= cool_rate`。因此温度按“成功算过 cost 的 move 数”而不是 `iteration/max_iters` 前进。bridge 比例、边界位置和 y lock 会改变 dead/invalid 比例，从而改变同一 profile 的有效温度曲线。

GUI 三档在 `compute.rs:42` 共用 `.99999`。假设每次尝试都有效：

- Quick：`40 * .99999^5000 ~= 38.05`；
- Standard：`40 * .99999^200000 ~= 5.41`；
- Full：`40 * .99999^1000000 ~= 0.00182`。

实际 dead/invalid move 只会让结束温度更高。公开默认值还有另一表现：`T0=10, cool_rate=.95` 大约 315 个有效 move 就低于 `1e-6` 并提前结束，`max_iters=10000` 并不是实际执行预算。

受影响路径：所有 SA profile、bridge/y-locked 密集电路、seed 可复现边界、进度中 iteration 的解释。

最小失败测试 `T08_profiles_reach_same_configured_end_temperature`：按每个 profile 的 `max_iters` 计算最后一次尝试温度，断言均到同一个 `T_end`；另用全 dead-move fixture 断言温度仍按 attempt index 前进。当前第一个断言用现有 profile 即可失败，第二个需要内部 trace/metrics 暴露 attempt temperature。

### A9. 电源轨行与 top/bottom `rail_id` 的物理模型

状态：top/bottom 隐式合并问题 **已经确认**；原报告的 group 断口判断 **已经纠正**。

`PowerRail.groups` 表示一条电源轨行中有插孔的 5 孔范围及 group 之间没有插孔的位置；它不表示底层导体断开。本产品模型已经明确：同一条完整电源轨行天然导通，所有 group 属于同一个 conductive island；top 与 bottom 是两个独立 island。

构造器 `breadboard.rs:311` 只按 polarity 分配两个 rail_id，使同极性的同一行、top 和 bottom 全部共享 id。跨 group 共享 id 是正确的板内连通；top/bottom 共享 id 则把外部短接伪装成板内连通。`connected_to`、MST、conflict validation、bridge matching 和 router 因而无法区分“同一行天然导通”和“上下轨由跳线连接”。现有跨 group 连通测试应保留；要求 top/bottom 无条件共享 rail 的测试应由显式 `RailTie` 场景替换。

产品决定见 `docs/adr/power-rail-connectivity.md`：采用独立 top/bottom islands，并在 400/800 preset 中为 negative 和 positive 各物化一条默认 top/bottom `RailTie`。同一行不生成 group 间 tie。

受影响路径：`Breadboard::connected_to`、`rail_id_of`、cost MST/congestion/rail conflict、bridge 候选 matching、router 的 net dedup/empty-hole 选择、最终 wire 数、GUI 结果图。

最小失败测试 `T09_power_rows_and_top_bottom_ties_match_physics`：断言同一行相邻 5 孔 group 属于同一个 conductive island；无 `RailTie` 时同极性 top/bottom 属于不同 island 且不连通；应用 preset 的两条默认 ties 后，对应 polarity 的上下轨才形成 effective connection，且结果快照能看到 tie。当前实现会在无 tie 时错误地连通 top/bottom。

### A10. 注释与诊断不等于当前设计

状态：客观矛盾 **已经确认**；“像 changelog、不适合作设计文档”的风格判断 **部分确认**。

当前仍存在：

- `cost/state.rs:3` 说每轮 clone，实际是 in-place + `Backup`，只在新 best/observer clone。
- `placement.rs:142` 说 Bridged 没 bbox，`:236` 实际生成 bbox。
- `cost/bridge.rs:120` 说无绑定 fallback 扫 rail，`:70-76` 明确已删除 fallback。
- `cost/mod.rs:54` 说八项权重，`Weights` 实际九项；`cost_fast` 计算 congestion，`cost_breakdown_inner` 和 `debug.rs` 不计算/展示它，诊断 total 可与真实 cost 不同。
- `sa.rs:370` 说不分配、SmallVec/array，`:487-489` 和 group helper 实际创建多个 `Vec`。
- `sa.rs` 顶部保留 v7/历史演进说明，部分内容是历史而不是可执行不变量。

最小失败测试 `T10_cost_breakdown_matches_real_cost_with_congestion`：构造 congestion 非零的 state，断言 breakdown total 等于 `cost()` 且有独立 congestion 字段。当前 breakdown 漏项。纯注释清理不应为字符串写脆弱测试，完成条件应由语义 diff review 和 doctest/全量编译保证。

### A11. Toggle reject 不能完整恢复 `SAState`

状态：**已经确认**。

实际链：`apply_move(ToggleBridging)` 的 `Backup` 只保存 `old_bridged`；从 OnBoard 翻到 Bridged 后，`simulate:879-895` 可能把 `active_bridge_idx` 改为另一个候选；reject 在 `:934` 只恢复 bool。其余平行 Vec 未被该 move 修改。

受影响范围比“所有 reject”窄：只有 OnBoard -> Bridged、有多个候选、候选选择改变 index、且 move 最终 reject 的路径。可见 placement 当下回到 OnBoard，但 inactive bridge state 已被污染，违反 AGENTS.md 的完整恢复不变量，并会成为后续 Toggle/调试/等价性问题。

最小失败测试 `T11_rejected_toggle_restores_all_parallel_state`：保存 `SAState` 所有字段，Toggle、模拟候选 index 改变、调用 backup revert，逐字段断言完全相等。当前只有 `active_bridge_idx` 不等。

### A12. 重复 prepare 会残留 bridge eligibility

状态：**已经确认**。

实际链：`prepare_for_layout_with_net_ids` -> `auto_mark_bridgeable` (`src/input/pcb.rs:183`)。该函数只在 XOR 成立时写 `comp.bridgeable = true`，从不在下一次调用前清零或给不符合的新配置写 false。同一 `Circuit` 改变 positive/negative binding 后，旧 true 会残留，`bridgeable_components` 随后直接收集残值。

当前 GUI 每次 `run_compute` 都重新 parse，所以 fresh GUI run 不触发；复用 Circuit 的库调用、交互式重新绑定和未来缓存路径受影响。

最小失败测试 `T12_prepare_recomputes_bridgeable_from_scratch`：同一 Circuit 先用让某两脚元件满足 XOR 的 binding prepare，再换成让两脚同为 power 或都非 power 的 binding prepare，断言第二次为 false。当前仍为 true。

### A13. SA write-back 不是事务性的

状态：**已经确认**。

实际链：`place_sa_impl:303` 在验证前发 `PlacementComplete`；`:312-333` 直接覆盖 `self.placements`；`:335` 才调用 `validate`。发生 fixed geometry、bridge init、greedy 或 binding 错误时，函数返回 `Err`，但新 placement 已留在 `Layout`，进度消费者还可能先看到“完成”。

受影响路径：所有最终 validation 失败的正常、zero-iteration、cancel、增量布局。`placeable.is_empty()` 是例外，它直接 validate，不写入。

最小失败测试 `T13_failed_place_sa_preserves_layout_and_emits_no_complete`：先放一个合法或故意非法的固定 placement，并保留未摆 slot/已有 wires；制造确定性最终 validation 错误，调用 SA 后断言 placements、wires 与调用前逐项相同，且没有 `PlacementComplete`。当前未摆 slot 被写入且完成事件先发出。

## 报告中重构与性能建议的当前性

这些条目多数是架构/性能建议而不是可单独复现的 correctness bug，但报告描述的当前实现事实也仍然成立：

| 报告建议 | 当前 HEAD 核验 | 归入顺序 |
|---|---|---|
| 用 immutable `AnnealProblem` 和单一 `ComponentPose` 取代平行 Vec | **已经确认仍未实现**。候选 catalog、pose 和 preprocess flags 都在 `SAState` 平行 Vec 中，best/progress clone 会一起复制 catalog | R4、R8 |
| hard constraint 与 soft cost 分离 | **已经确认只做了一部分**。y/OnBoard pin OOB 在 loop 前硬拒绝；bbox、pin collision、column conflict 仍主要靠大权重，binding 甚至不在 final validator | R2、R4、R5 |
| 只生成当前状态下有效的 move | **已经确认仍未实现**。Bridged 上的 Flip/ShiftY、无邻接候选的 ShiftX、部分 group 都会成为 dead move | R10 |
| `ToggleBridge` 与 `ChangeBridgeCandidate` 分开 | **已经确认仍未实现**。Toggle 内嵌 K 次全 cost；candidate 变化还隐含在 bridged ShiftX/ShiftGroup 中 | R8、R10、R13 |
| global relocate、swap、双向 group move | **已经确认当前没有这些 move**；但这是解质量能力缺口，不是已证明的 correctness failure | R13 的 benchmark/质量门，不阻塞 R1-R12 |
| 记录 acceptance/dead/invalid 比例 | **已经确认缺失**。现有 profile 只有 clone/cost/move 时间与总 iteration | R10 |
| bridge catalog 跨 seed 复用 | **已经确认缺失**。每个 `simulate` 都重新 `populate_bridgeable_info` 和填 context | R13 |
| 候选左右邻接预计算 | **已经确认缺失**。bridged ShiftX/Group 用 `.iter().position(...)` 线性查找 | R13 |
| move 只重算受影响 net/bbox/rail | **已经确认缺失**。每次 `cost_fast` 都 `buf.clear()` 并重建全部 pin、bbox、net bucket、rail map | R13 |
| bbox 交集不用逐 cell 枚举 | **已经确认缺失**。`cost_fast` 和 breakdown 都遍历 `bi.iter_cells()` 统计矩形交集 | R13 |

性能收益大小本轮没有做 release benchmark，因此上述性能优先级只能标为**部分确认**；不得用静态推测跳过基准或牺牲 cost 等价性。

## 共享状态、共享文件与依赖

### 共享状态

| 状态 | 写入者 | 读取者 | 当前风险 |
|---|---|---|---|
| `Circuit.components[].bridgeable` | `prepare`/`auto_mark_bridgeable` | bridge catalog、UI preparation result | 跨 binding 调用残留 |
| `Breadboard.holes[].rail_id` | board 构造器 | occupancy、cost、bridge、router | 正确表示同一电源轨行的内部连通，但又错误地把 top/bottom 外部 tie 和 polarity binding 混入同一个 id |
| `Breadboard.power_rail_binding` | preparation | cost、bridge、router；validation 未完整读取 | 软优化与最终 legality 不一致 |
| `Layout.placements` / `Layout.wires` | public mutator、SA write-back、router | snapshots、occupancy、routing | 失败后仍被修改；已有几何未进入 SA |
| `SAState` 平行 Vec | init、move、bridge candidate selection | cost、best clone、write-back | 字段需同步；Toggle reject 漏恢复 index |
| `PreprocessResult` | `preprocess_for_breadboard` | greedy、spectral、simulate move constraints | greedy 在搜索后才应用 |
| `CancellationToken` | GUI command/回调 | seed 调度、simulate loop | 初始化阶段不检查，cancel 仍形成 aggressive 初态 |
| `SAContext` / `CostBuf` | 每个 seed 内部 | 同 seed 的 cost 调用 | 它们不是跨 seed 共享状态；并行修复不应误加全局锁 |
| `base_placements` / `bridged_pins` | `place_sa_impl` 的只读快照 | progress/cost | base 只用于展示，bridged 只含 pin，均不是完整 fixed geometry |

### 高冲突共享文件

- `src/layout/sa.rs`：初始化、move/reject、cancel、温度计划、bridge policy 都会修改；相关任务禁止并行。
- `src/layout/layout_impl.rs`：fixed geometry 收集、事务写回、progress 和 final validation 共用；相关任务禁止并行。
- `src/layout/cost/bridge.rs`、`state.rs`、`context.rs`、`cost_fast.rs`：bridge catalog/init、初始化器和 cost 不变量交叉；必须按下方顺序落地。
- `src/layout/breadboard.rs`、`occupancy.rs`、`routing.rs`：共享 `rail_id`/连通语义；电源轨模型必须先定再改 validation 和 routing。
- `src/layout/tests.rs`、`src/layout/cost/tests.rs` 及 `sa.rs` 内联 tests：多个修复都会改 fixture/helper；按串行顺序复用，不能各自复制出不同的“合法性”定义。
- `src-tauri/src/compute.rs`：profile schedule、cancel/progress 和可能的显式 RailTie 展示交叉；只在核心语义稳定后调整。

## 严格线性修复顺序

以下是单一路径：`R0 -> R1 -> ... -> R13`。不得并行，也不得越过尚未满足的完成条件。correctness 修复必须先提交最小失败测试，确认在旧实现上因预期原因失败，再改允许范围内的生产代码；产品决策和纯性能项使用各自列出的 gate。

### R0. 冻结电源轨产品语义（决策门）

- 对应：A9。
- 前置：无。
- 最小验收场景：记录 T09 的期望答案——同一电源轨行的各 5 孔 group 天然导通，默认无 tie 时 top/bottom 独立；如果产品假设预接线，列出每一条默认 `RailTie` 以及 UI 如何展示。
- 允许修改范围：仅 ADR/产品文档；本阶段不改 Rust/测试。
- 完成条件：**已完成**。`docs/adr/power-rail-connectivity.md` 已明确 `ConductiveIsland`、`bound_net`、`RailTie` 三者语义；每条完整电源轨行是一个 island；400/800 preset 默认各有两条 top/bottom ties。

### R1. 纠正 conductive-island / RailTie 基础模型

- 实施状态：**已完成**（2026-07-15）。T09 已先证明旧实现会在无 tie 时错误连接 top/bottom；修复后 physical island、effective connectivity、默认 tie 占孔、cost、router、validation 和 GUI frame 使用同一语义。
- 对应：A9；为 A5、A3、A7 的最终语义打基础。
- 最小失败测试：T09；旧的“同 polarity 全共享 rail_id”测试必须按 R0 决定替换，不能简单删除断言。
- 允许修改范围：`src/layout/{breadboard,mod,routing,prepare}.rs`，以及因新连通 API 必须同步的 `src/layout/cost/{mst,context,cost_fast,bridge}.rs`；若 R0 选择显式 ties，可改 `src-tauri/src/{lib,compute}.rs`、`src/lib/components/{Step2SelectBoard,BreadboardPreview,Step4Result}.svelte` 和 `src/lib/i18n.ts`。不得改 SA move/temperature。
- 完成条件：物理内部连通只由 island 表示；同一电源轨行跨 5 孔 group 天然导通；top/bottom 外部连接只由 RailTie 表示；cost、occupancy、router 对同一 effective connectivity graph 给出一致答案；400/800 的行内连通及 top/bottom tie 测试通过。

### R2. 收紧 public legality：Bridged bijection + rail binding

- 实施状态：**已完成**（2026-07-15）。T04/T05 已先证明旧实现会接受重复 Bridged PinId、三脚元件漏脚，以及 bound rail 上没有第二个真实端点时的错 net pin/wire；现在这些状态分别返回精确的 Bridged pin-set 错误或 `RailBindingConflict`。
- 对应：A4、A5。
- 依赖：R1 的最终 island/tie 语义。
- 最小失败测试：T04、T05。
- 允许修改范围：`src/layout/mod.rs`（新增精确错误）、`src/layout/{placement,occupancy}.rs`、必要的 `src/layout/breadboard.rs` 只读查询，以及这些模块的测试。不得改 SA/cost 以“用大罚分代替验证”。
- 完成条件：Bridged 必须完整且唯一表示恰好两脚的 component；真实 pin/wire 与每个 bound island 的预期 net 冲突会得到稳定的专用错误；`validate == Ok` 足以建立这两条硬不变量。

### R3. 建立事务性 SA write-back

- 对应：A13，且先为后续修复提供失败隔离。
- 依赖：R2 的错误集合稳定。
- 最小失败测试：T13。
- 允许修改范围：`src/layout/layout_impl.rs`、必要的 `progress.rs` 事件时序和 `src/layout/tests.rs`。不得在这一步改变搜索算法或 cost。
- 完成条件：best state 先写入临时 placement 集合并用完整 Layout candidate 验证；只有验证成功才原子替换 `self.placements`；错误时 placements/wires 保持调用前值；`PlacementComplete` 只在候选通过验证后发布。

### R4. 把已有 placement/wire 提升为 immutable problem geometry

- 对应：A3。
- 依赖：R2 保证输入 placement 可解释；R3 保证失败不污染 Layout。
- 最小失败测试：T03，另加固定 OnBoard net 端点会改变 MST 的断言。
- 允许修改范围：可新增 `src/layout/problem.rs`；可改 `src/layout/{layout_impl,occupancy}.rs`、`src/layout/cost/{context,cost_fast,state,spectral}.rs` 中可复用的几何提取，以及对应 tests。不得改随机 move 分布和温度。
- 完成条件：一个 immutable problem 对象完整包含固定 OnBoard pin+bbox、固定 Bridged pin+bbox、已有 wire endpoints/net 及其连接的两个 conductive islands；greedy/spectral 初始化和每次 cost 使用同一对象；固定几何绝不进入可移动 `SAState`；已有 wire 同时是孔位障碍和不可删除的电气连接；progress snapshot 保留已有 wires。

### R5. 统一 preprocess-aware 合法初始化器并返回 `Result`

- 对应：A2；覆盖初始化、zero iteration 和小型 spectral fallback。
- 依赖：R4，因为 first-fit 必须同时避开固定几何。
- 最小失败测试：T02，再对 `use_spectral=false/true(n<=2)/true(n>2)` 跑同一 legality assertion。
- 允许修改范围：`src/layout/cost/{state,spectral}.rs`、`src/layout/{preprocess,sa}.rs` 的初始化入口、R4 的 problem 模块和对应 tests。不得在调用者处 catch panic 后伪装成合法结果。
- 完成条件：rotation/y lock 在候选搜索前确定；初排检查 pin、bbox、blocked、rail conflict、fixed geometry；greedy 和 spectral 共用同一 hard-legality predicate；装不下返回结构化 `Result`；`max_iters=0` 也只能产出合法初态或错误。

### R6. 让 bridge eligibility 每次 prepare 都从零重算

- 对应：A12。
- 依赖：R5 后初始化契约稳定；必须早于 bridge policy 修复。
- 最小失败测试：T12。
- 允许修改范围：`src/input/pcb.rs` 的 eligibility 函数、`src/layout/prepare.rs` 和对应 tests；如需保留手工 override，允许在 `circuit.rs` 把“自动 eligibility”和“用户 policy”拆字段。不得靠 GUI 每次 reparse 规避。
- 完成条件：同一 Circuit 对同一 binding 幂等；改变 binding 后结果只由当前 nets/policy 决定；手工 override 若保留，优先级有测试和文档。

### R7. 先封死所有 reject 的完整状态恢复

- 对应：A11 和 AGENTS.md 的 rejected-move invariant。
- 依赖：R5 后 `SAState` 字段形态稳定，避免随后重写测试。
- 最小失败测试：T11，并把 Flip/ShiftX/ShiftY/ShiftGroup/Toggle 做成同一逐字段 property table。
- 允许修改范围：`src/layout/sa.rs` 的 `Backup`、`apply_move`、candidate-selection/revert 代码及内联 tests。不得顺便改 move 概率或 cost。
- 完成条件：任何 `apply_move` 返回 `None` 都是零修改；任何已应用 move 经 reject 后 `SAState` 所有字段逐项等于 move 前；Toggle 的候选 index 也恢复。

### R8. 引入明确 BridgePolicy 并修正初始化候选比较

- 对应：A1、A6；覆盖 bridge 初始化、cancel、zero iteration。
- 依赖：R1/R2 的 rail/bridge legality、R5 的合法 OnBoard baseline、R7 的 reject invariant、R6 的准确 eligibility。
- 最小失败测试：T01、T06；保留一个“最优就是 index 0”的正例但不能只靠它。
- 允许修改范围：`src/layout/cost/{bridge,state}.rs`、`src/layout/{sa,layout_impl,mod}.rs`、`src/lib.rs` 的公开配置导出、`src-tauri/src/compute.rs` 的 profile 配置和相关 tests。不得修改 router。
- 完成条件：候选 cost 评估时当前元件确实处于 Bridged；Explore/BestOfBoth 同时比较合法 OnBoard 和所有合法 bridge candidates；Disabled 不建 catalog/不 bridge，Forced 的非法初态返回错误；cancel/zero 返回完整合法状态；`p_toggle_bridge` 若保留只表示 Explore 中的 move rate，不再承担 policy 语义。

### R9. 使 cost 和 diagnostics 对排列不变且同源

- 对应：A7、A10 的 congestion 漏项。
- 依赖：R1/R4/R8 后 cost 输入集合和连通语义稳定。
- 最小失败测试：T07、T10；对固定 geometry、SA Bridged、power anchor 混合输入也做 permutation assertion。
- 允许修改范围：`src/layout/cost/{cost_fast,context,mod,tests}.rs`、`src/layout/debug.rs`。不得在测试里排序输入来掩盖生产代码顺序依赖。
- 完成条件：column conflict 使用与顺序无关的 owner 计数/集合定义；row bitmap 先求稳定基准或直接使用稳定集合；`cost_fast` 与 breakdown 共享计算或至少逐项一致；congestion 出现在 breakdown 和 debug total；任意 component permutation 成本相等。

### R10. 生成 state-aware move，并用归一化 attempt progress 定义温度

- 对应：A8，以及报告中“只生成有效 move、拆分 Toggle/ChangeCandidate、记录 move 比例”的建议。
- 依赖：R7/R8 后 dead/invalid move 定义稳定，便于指标解释。
- 最小失败测试：T08；另加 `T08b_generator_never_returns_mode_inapplicable_move`，在 OnBoard/Bridged/y-locked/无相邻候选状态采样或枚举 move，断言不会返回必然 `apply_move == None` 的类型；GUI profile test 改为断言共同 `T_start/T_end` 语义，而不是共同 magic cool rate。
- 允许修改范围：`src/layout/sa.rs` 的 schedule/config/metrics、`src-tauri/src/compute.rs` profile 和相关 tests；如公开 API 兼容需要，可对 `cool_rate` 做弃用过渡，但不得继续让它覆盖归一化 schedule。
- 完成条件：move generator 按当前 pose/lock/catalog 只选择可应用的 move class；`ToggleBridge` 与 `ChangeBridgeCandidate` 是独立操作；若状态确实没有任何 move，显式记录一次 no-candidate attempt，而不是伪造 dead mutation；温度由 `attempt_index / max_iters` 纯函数决定，no-candidate、hard-invalid、accept、reject 都消耗一次 schedule attempt；Quick/Standard/Full 在各自预算末端达到同一个配置 `T_end`；指标区分 attempted/no-candidate/invalid/evaluated/accepted。

### R11. 清理 correctness 路径上的失真注释和诊断说明

- 对应：A10 的非功能部分。
- 依赖：R1-R10，避免为即将改变的实现写第二遍说明。
- 最小失败检查：T10 已在 R9 保证诊断事实；本步不添加针对注释文本的脆弱单测。
- 允许修改范围：本报告列出的 `src/layout/cost/{state,bridge,mod}.rs`、`src/layout/{placement,sa}.rs` 注释，以及必要 ADR；生产逻辑零变化。
- 完成条件：注释只描述当前不变量、单位、前置条件和原因；删除错误的 clone/bbox/fallback/SmallVec/权重数量说法；历史版本叙述移到 ADR/git；`cargo fmt --check` 的代码 diff 应为空或仅注释换行。

### R12. 加真实 PCB 全链路连通图回归

- 对应：审查报告“最优先补的测试”的最终一项，用来封住所有任务组合后的系统行为。
- 依赖：R1-R11 全部完成。
- 最小失败测试：`T14_real_pcb_parse_prepare_sa_route_connectivity`，使用仓库真实 PCB fixture，固定算法版本/seed，执行 parse -> prepare -> SA -> route -> validate，再构造 effective connectivity graph；每个 net 的所有真实 pin 和绑定 anchor 必须在该 net 的同一连通分量，且不同 net 不得共享 conductive island。
- 允许修改范围：优先只新增 root 或 `src-tauri/tests` integration test，并复用 `examples/inputs` fixture；本步默认不允许生产改动。若测试暴露新缺陷，必须另开有最小范围和失败原因的修复项，不能放宽图断言。
- 完成条件：真实 fixture 在固定 seed 下稳定通过；同时至少覆盖 OnBoard、Bridged、power binding、wire、固定算法版本的 seed 可复现定义；全 workspace test、fmt、clippy 全过。

### R13. 最后做 benchmark-gated 的质量与性能重构

- 对应：报告的 global relocate/swap/双向 group 与五项性能建议；它们不是 R1-R12 correctness 的替代品。
- 依赖：R12 建立端到端语义保护后才能开始。
- 最小失败 gate：先建立 release benchmark 和等价性 harness。只有当前实现超过产品确认的时间/内存/解质量预算，才把该 benchmark 作为失败用例；没有预算或基准证据时不以“看起来更快”为由改热路径。每个优化还必须在随机 state corpus 上证明优化前后逐项 cost/legality 完全一致。
- 严格子顺序：`R13.1 catalog 跨 seed 复用 -> R13.2 候选邻接索引 -> R13.3 Toggle/ChangeCandidate 局部 delta -> R13.4 受影响 net/bbox/rail 增量 cost -> R13.5 矩形交集闭式计算 -> R13.6 由质量基准决定是否加入 relocate/swap/双向 group`。子项同样不得并行。
- 允许修改范围：R4 的 immutable problem/catalog、`src/layout/cost/{bridge,context,cost_fast}.rs`、`src/layout/sa.rs` 及专用 benchmark/tests；不得改变 hard legality，不能以近似 cost 替代精确等价，不能顺带改 GUI。
- 完成条件：每个子项单独给出基准前后数据和等价性结果；无显著收益的子项回退；算法行为若有意改变必须提升算法版本并重新确认 seed 契约；完成后重新审阅 R11 注释并重跑 T01-T14 与全量验证。

## 最终完成门槛

修复序列完成时必须同时满足：

1. T01-T13 的 correctness regression 均先在对应旧实现上因目标原因失败，再在修复后通过；T14 是系统级防回归，可在旧实现上通过，但不得被削弱。R0/R13 分别由产品决策和已确认性能预算 gate。
2. reject 后完整 state 相等；invalid candidate 不写回 Layout；cancel/zero 只返回合法、完整状态或结构化错误。
3. fixed OnBoard、fixed Bridged body/pins、已有 wire 都是 immutable geometry，并同时进入初排、cost 和 final validation。
4. public validation 独立于 soft cost，严格覆盖 Bridged bijection 和 rail binding。
5. cost 对 component permutation 不变，breakdown 与真实 total 一致。
6. 电源轨物理 island、bound net、显式 RailTie 的语义在 cost/router/validation/UI 中一致。
7. `cargo fmt --check`、相关最小测试、`cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 全部通过。
8. 不以保持旧 RNG 消费数量为理由保留 dead move；seed 可复现只承诺“同算法版本 + 同 seed + 同完整配置/输入”。
