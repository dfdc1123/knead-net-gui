# Layout SA 历史 release 性能基线

基准日期：2026-07-15。固定 workload 是 `examples/h-bridge/h-bridge.kicad_pcb`、830 preset 40 列、spectral 初始化、10 seeds × 5000 attempts、base seed `0xCAFE_F00D`。这些数据由当时的临时调试二进制采集；该二进制已随项目转为纯 GUI 入口而删除，以下结果作为历史基线保留。

计时是所有并行 seed 的各阶段累计 CPU 时间，用于比较热点占比，不等同于单次 wall time。质量 gate 是 10-seed cost 分布、最终 placements 与 routing wire 数；系统等价性另由 release T14 连通图回归验证。

## 基线与保留结果

| 指标 | R13 基线 | 保留优化后 |
|---|---:|---:|
| Attempts | 50,000 | 50,000 |
| Init | 56.03 ms | 29.13 ms |
| Move generation | 48.25 ms | 47.05 ms |
| Apply | 2.14 ms | 1.92 ms |
| 独立 hard legality | 321.69 ms | 合并进 checked cost |
| Cost / checked cost | 357.46 ms | 120.89 ms |
| 上述阶段合计 | 785.57 ms | 199.24 ms（-74.6%） |
| Cost calls | 20,372 | 51,817（所有 candidate 都做 checked collection） |
| MST/congestion | 344.73 ms | 18.46 ms |
| Best / median cost | 450.5 / 536.75 | 450.5 / 536.75 |
| Mean / max / stddev | 531.75 / 606.0 / 43.617 | 531.75 / 606.0 / 43.617 |
| Routed wires | 16 | 16 |

保留的改动：

- effective rail 的物理孔容量在 `SAContext` 构造时预计算；
- 默认启用 congestion 时，一次稳定 Kruskal 同时得到 MST 长度与 degree，避免重复建边和排序；
- SA 用 cost collection 已得到的 pin、bbox、rail owner 数据做 hard legality，hard-invalid 在 MST 前提前返回；debug 构建逐 move 与原 `state_hard_legal` 对照。

等价性证据：0–20 pins、每种规模 200 个随机样本的 MST length/degree 全相等；现有 fast/breakdown、排列不变和 reject tests 全过；固定 seed 的完整 cost 分布与 wire 数未改变；`cargo test --release --test layout_real_pcb` 两次 parse → prepare → SA → route → connectivity 用时约 0.04 秒。

## R13 子项结论

- R13.1 catalog 跨 seed 复用：不保留。完整初始化在基线中的理论上限约 7.1%，catalog 只是其中一部分；没有独立显著收益证据。
- R13.2 bridge 邻接索引：不保留。整个 generator 的理论上限约 6.1%，线性 candidate lookup 只是其中一部分。
- R13.3 Toggle/ChangeCandidate 局部 delta：不单独引入。共享 checked-cost 已覆盖主要收益，再维护 pose-specific delta cache 会扩大失效面。
- R13.4 affected cost/legality：保留上述共享 collection、提前 hard reject、rail capacity 与 MST/degree 合并；未继续引入跨 attempt 的可变增量 cache，因为当前基准已下降 74.6%，且没有更严格产品预算来证明其一致性风险合理。
- R13.5 bbox 闭式交集：随机等价测试通过，但 bbox 阶段只从约 11.1 ms 降到 8.8 ms，整体不足 2%，已回退。
- R13.6 relocate/swap/双向 group：不加入。固定 seed 的质量分布和 16-wire 结果没有预算缺口；改变 move 集会改变算法版本与 seed 轨迹，目前没有产品质量 gate 支持。

Profiler 本身也纳入严格检查：

```sh
RUSTFLAGS='--cfg profile_sa --cfg profile_cost' \
  cargo clippy -p knead-net --all-targets --all-features -- -D warnings
```
