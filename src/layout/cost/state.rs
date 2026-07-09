//! `SAState`: SA 内部状态 (每个元件的 (x, y, rotation) + 桥接候选)。
//!
//! `SAState` 在 simulate() 入口构造一次, SA 主循环里反复拷 (clone) 试新解, 拒绝解丢弃。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, ComponentId, PinId};
use crate::layout::breadboard::{Breadboard, HoleId};
use crate::layout::placement::Rotation;
use crate::layout::preprocess::PreprocessResult;

use super::spectral::{compute_fiedler, compute_second_evec, grid_fill_2d};

/// SA 内部状态: 每个元件显式持有 `(x, y, rotation)`.
///
/// 当前架构是显式 2D 布局: x 不再由 order 推导, SA 可以把元件放到板子任意位置。
/// order 还在, 但只用于标识; `placeable[i]` 的 i 索引对应 `x[i] / y[i] / rotation[i]`。
///
/// `is_bridgeable[i] / bridged[i] / bridged_pin_pairs[i] / active_bridge_idx[i]`
/// 是桥接探索的四个字段, 长度都 == `placeable.len()`。`is_bridgeable` 由
/// [`Layout::place_sa`] 内部 → `populate_bridgeable_info` 在 `sa::simulate`
/// 主循环前根据 `Component.bridgeable` + 启发式是否找到合法桥接位决定;
/// `bridged` 初始全 false (默认 OnBoard), SA 通过 `Move::ToggleBridging`
/// 翻成 true; `bridged_pin_pairs[i]` 是启发式算出的**所有合法** `(HoleId, PinId)`
/// 对 (按 "signal pin 离同 net 中心最近" 排序), bridged=true 时由 `cost()` 用
/// `active_bridge_idx[i]` 选中的那对替代 OnBoard 的 `(x, y, rotation)` 计算 pin
/// 位置; `active_bridge_idx[i]` 默认 0 (启发式最优), SA Toggle 到 bridge 时会
/// 遍历候选、按 cost 重选。
#[derive(Debug, Clone)]
pub struct SAState {
    pub placeable: Vec<ComponentId>,
    /// `i` 是否允许 Toggle 进 Bridged 模式 (启发式 + 桥接规则双满足)。
    /// 非桥接元件此处恒为 false; 即使 `Component.bridgeable = true`,
    /// 启发式找不到合法 (hole, rotation) 时也是 false。
    pub is_bridgeable: Vec<bool>,
    /// `i` 当前是否处于 Bridged 模式; 初始 false, 由 `Move::ToggleBridging` 翻转。
    pub bridged: Vec<bool>,
    /// `i` 的预计算桥接 pin 对**列表** (按启发式质量排序: signal pin 离同 net 中心
    /// 最近排第一)。`is_bridgeable[i] = false` 时此 Vec 为空。bridged=true 时由
    /// `active_bridge_idx[i]` 选出实际使用的那对。
    pub bridged_pin_pairs: Vec<Vec<[(HoleId, PinId); 2]>>,
    /// `i` 当前选中的候选下标 (仅 bridged[i] = true 时有意义); 默认 0。
    /// SA 在 ToggleBridging 翻到 bridge 模式时会遍历 `bridged_pin_pairs[i]` 按 cost
    /// 重选并写回这里。
    pub active_bridge_idx: Vec<usize>,
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub rotation: Vec<Rotation>,
    /// 每个元件是否只能使用 R90 / R270 (由预处理标记)。
    pub r90_only: Vec<bool>,
    /// 元件 y 坐标锁定值 (None = 不锁定)。由预处理为跨通道元件设置。
    pub y_locked: Vec<Option<i32>>,
}

impl SAState {
    pub fn n(&self) -> usize {
        self.placeable.len()
    }

    /// 当前 `i` 在 Bridged 模式下使用的 pin 对 (None 表示当前不是 bridge 模式
    /// 或没有候选)。cost() / place_sa 写回 placement 时都通过这里取值, 避免
    /// 直接散落读 `bridged_pin_pairs[i][active_bridge_idx[i]]` 引发越界。
    pub fn active_bridge_pair(&self, idx: usize) -> Option<[(HoleId, PinId); 2]> {
        if !self.bridged[idx] {
            return None;
        }
        self.bridged_pin_pairs[idx]
            .get(self.active_bridge_idx[idx])
            .copied()
    }

    /// 拼装辅助: 默认"所有元件不可桥接"的桥接字段。
    /// 给测试用 struct update 语法 `..SAState::no_bridging(n)`,
    /// 其中 n = placeable.len()。`placeable / x / y / rotation` 留空,
    /// 上面覆盖。
    pub fn no_bridging(n: usize) -> Self {
        Self {
            placeable: Vec::new(),
            is_bridgeable: vec![false; n],
            bridged: vec![false; n],
            bridged_pin_pairs: vec![Vec::new(); n],
            active_bridge_idx: vec![0; n],
            x: Vec::new(),
            y: Vec::new(),
            rotation: Vec::new(),
            r90_only: vec![false; n],
            y_locked: vec![None; n],
        }
    }

    /// 简单构造: 给定元件顺序, 全部 R0, 全部同一行, x 按顺序累加 (gap=1)。
    /// 主要给测试用——真实初始状态用 [`SAState::from_greedy`]。不填桥接信息:
    /// 桥接字段 (`is_bridgeable` / `bridged` / `bridged_pin_pairs` / `active_bridge_idx`) 默认全
    /// `false` / `Vec::new()` / `0` / `0`。
    /// 调用方若需要探索桥接, 走 [`Self::from_greedy`] + [`populate_bridgeable_info`]。
    pub fn from_order(order: Vec<ComponentId>, row: i32, widths: &[i32]) -> Self {
        let n = order.len();
        let mut x = Vec::with_capacity(n);
        let mut cur = 0i32;
        for &w in widths {
            x.push(cur);
            cur += w + 1;
        }
        Self {
            placeable: order,
            is_bridgeable: vec![false; n],
            bridged: vec![false; n],
            bridged_pin_pairs: vec![Vec::new(); n],
            active_bridge_idx: vec![0; n],
            x,
            y: vec![row; n],
            rotation: vec![Rotation::R0; n],
            r90_only: vec![false; n],
            y_locked: vec![None; n],
        }
    }

    /// 贪心 first-fit 初始状态: 按元件顺序, 找第一个有效 `(x, y)` (按行从上到下、
    /// 列从左到右扫)。"有效" = 所有 pin 都在板内 + 包围盒不撞已摆元件 bbox +
    /// 不引入列冲突 (同列同 rail 不同 net 的 pin)。
    ///
    /// 两个初排 (`from_greedy` / `from_spectral`) 都
    /// 做这个检查, 因此初排结果**保证**不引入列短路 — SA 后续只在 `Flip` /
    /// `ShiftX` 偶尔重新引入时捕捉并罚分。
    pub(crate) fn from_greedy(placeable: Vec<ComponentId>, circuit: &Circuit, board: &Breadboard, preprocess: &PreprocessResult) -> Self {
        let n = placeable.len();
        let mut x = vec![0i32; n];
        let mut y = vec![0i32; n];
        let rotation = vec![Rotation::R0; n];
        let mut occupied: HashSet<(i32, i32)> = HashSet::new();
        // (col, rail_top) → 第一个放进去的 pin 的 net; 后续 pin 若 net 不同则为冲突。
        // None 表示该位置的 pin 未连 net (unconnected); unconnected 与 connected 互冲。
        let mut col_owner: HashMap<(i32, i32), Option<crate::circuit::NetId>> = HashMap::new();

        for idx in 0..n {
            let comp_id = placeable[idx];
            let component = &circuit.components[comp_id.0];
            let fid = component.footprint.expect("placeable 必有 footprint");
            let footprint = &circuit.footprints[fid.0];

            // 该元件在 net 上的 pin: (本地 offset, net) — 用于列冲突检查
            let pin_info: Vec<(i32, i32, Option<crate::circuit::NetId>)> = component
                .pins
                .iter()
                .map(|&pin_id| {
                    let pin = &circuit.pins[pin_id.0];
                    let physical = footprint
                        .pins()
                        .iter()
                        .find(|p| p.name() == pin.num())
                        .expect("footprint 缺 pin (解析阶段就该爆)");
                    (physical.offset.x, physical.offset.y, pin.net)
                })
                .collect();
            // bbox 仍用 footprint 全部物理 pin 算 (保留原 from_greedy 行为):
            // 即便 component 只用 1 个 pin, footprint 本体的 silk / 镂空也算"占用",
            // 不能让别的元件挤进来。
            let (min_x, max_x, min_y, max_y) = footprint.pins().iter().fold(
                (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
                |(lx, rx, ly, ry), p| {
                    (
                        lx.min(p.offset.x),
                        rx.max(p.offset.x),
                        ly.min(p.offset.y),
                        ry.max(p.offset.y),
                    )
                },
            );
            let bbox_cells: Vec<(i32, i32)> = (min_y..=max_y)
                .flat_map(|yy| (min_x..=max_x).map(move |xx| (xx, yy)))
                .collect();

            let mut found: Option<(i32, i32)> = None;
            'outer: for try_y in 0..board.rows() as i32 {
                if board.is_blocked(try_y as usize) {
                    continue;
                }
                for try_x in 0..board.cols() as i32 {
                    let oob_or_blocked = bbox_cells.iter().any(|&(dx, dy)| {
                        let x = try_x + dx;
                        let y = try_y + dy;
                        x < 0
                            || x >= board.cols() as i32
                            || y < 0
                            || y >= board.rows() as i32
                            || board.is_blocked(y as usize)
                    });
                    if oob_or_blocked {
                        continue;
                    }
                    let collides = bbox_cells
                        .iter()
                        .any(|&(dx, dy)| occupied.contains(&(try_x + dx, try_y + dy)));
                    if collides {
                        continue;
                    }
                    // 列冲突检查: 任一 pin 落在 (col, rail_top) 上, 而该位置
                    // 已有不同 net 的 pin (包括 None 视为一类), 则此位置不合法。
                    let col_conflict = pin_info.iter().any(|&(lx, ly, pin_net)| {
                        let abs_x = try_x + lx;
                        let abs_y = try_y + ly;
                        if abs_x < 0
                            || abs_x >= board.cols() as i32
                            || abs_y < 0
                            || abs_y >= board.rows() as i32
                            || board.is_blocked(abs_y as usize)
                        {
                            return true;
                        }
                        let rail_top = board.rail_rows(abs_y).first().copied().unwrap_or(abs_y);
                        match col_owner.get(&(abs_x, rail_top)) {
                            Some(existing) => *existing != pin_net,
                            None => false,
                        }
                    });
                    if col_conflict {
                        continue;
                    }
                    found = Some((try_x, try_y));
                    break 'outer;
                }
            }

            let (fx, mut fy) = found.unwrap_or_else(|| panic!("元件 {} 装不下这块板", comp_id.0));
        // y-locked: 覆盖为锁定值
        if let Some(&ly) = preprocess.y_locked.get(&comp_id) {
            fy = ly;
        }
            x[idx] = fx;
            y[idx] = fy;
            for &(dx, dy) in &bbox_cells {
                occupied.insert((fx + dx, fy + dy));
            }
            // 记下每个 pin 占的 (col, rail_top) 及其 net
            for &(lx, ly, pin_net) in &pin_info {
                let abs_x = fx + lx;
                let abs_y = fy + ly;
                if abs_x >= 0
                    && abs_x < board.cols() as i32
                    && abs_y >= 0
                    && abs_y < board.rows() as i32
                    && !board.is_blocked(abs_y as usize)
                {
                    let rail_top = board.rail_rows(abs_y).first().copied().unwrap_or(abs_y);
                    col_owner.entry((abs_x, rail_top)).or_insert(pin_net);
                }
            }
        }

        Self {
            placeable,
            is_bridgeable: vec![false; n],
            bridged: vec![false; n],
            bridged_pin_pairs: vec![Vec::new(); n],
            active_bridge_idx: vec![0; n],
            x,
            y,
            rotation,
            r90_only: vec![false; n],
            y_locked: vec![None; n],
        }
    }

    /// 频谱布局初排: 图拉普拉斯 2D 嵌入 → 网格填充。
    ///
    /// 流程:
    /// 1. Net-star 图: 每个 net 作为虚拟节点, 元件只连到所属 net
    ///    (避免 pairwise 展开形成的虚假吸引团)
    /// 2. 拉普拉斯 + 幂迭代 → v₂, v₃ (只取元件节点对应分量)
    /// 3. v₂ 值 → x 目标, 贪心碰撞解决 → 紧凑格点
    ///
    /// Net-star 比 pairwise 好的地方:
    /// - 大 net (6+ pin) 不会变成 O(k²) 边的团
    /// - net 权重 1/(k-1) 自动衰减大 net 的影响 (Rent-like scaling)
    /// - 电源网不再把 GND/+12V 侧元件强行聚在一起
    pub(crate) fn from_spectral(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
        seed: u64,
        preprocess: &PreprocessResult,
    ) -> Self {
        let n = placeable.len();
        if n <= 2 {
            return Self::from_greedy(placeable, circuit, board, preprocess);
        }

        // ── comp_id → placeable_idx 映射 ──
        let mut comp_to_idx: HashMap<ComponentId, usize> = HashMap::with_capacity(n);
        for (i, &cid) in placeable.iter().enumerate() {
            comp_to_idx.insert(cid, i);
        }

        // ── 收集 ≥2 个 placeable 元件的 net, 附带 1/(k-1) 权重 ──
        let mut active_nets: Vec<(Vec<usize>, f64)> = Vec::with_capacity(circuit.nets().len());
        for net in circuit.nets() {
            let mut comps: Vec<usize> = net
                .pins()
                .iter()
                .filter_map(|&pid| comp_to_idx.get(&circuit.pins[pid.0].component).copied())
                .collect();
            comps.sort();
            comps.dedup();
            let k = comps.len();
            if k >= 2 {
                let weight = 1.0 / (k - 1) as f64; // Rent-like: 大 net 权重小
                active_nets.push((comps, weight));
            }
        }

        let n_nets = active_nets.len();
        let total_n = n + n_nets;

        // ============================================================
        // Phase 1: Net-star 图 — 元件 ↔ net (不做元件间 pairwise)
        // ============================================================
        let mut w = vec![vec![0.0f64; total_n]; total_n];
        for (net_idx, (comps, weight)) in active_nets.iter().enumerate() {
            let vnet = n + net_idx;
            for &c in comps {
                w[c][vnet] += weight;
                w[vnet][c] += weight;
            }
        }

        let mut l = vec![vec![0.0f64; total_n]; total_n];
        for i in 0..total_n {
            let deg: f64 = w[i].iter().sum();
            l[i][i] = deg;
            for j in 0..total_n {
                l[i][j] -= w[i][j];
            }
        }

        // ============================================================
        // Phase 2: 幂迭代求 v₂, v₃ (全图), 只取前 n 个分量 (元件)
        // ============================================================
        let v2_all = compute_fiedler(&l, total_n, seed);
        let v3_all = compute_second_evec(&l, &v2_all, total_n, seed);
        let v2: Vec<f64> = v2_all[..n].to_vec();
        let v3: Vec<f64> = v3_all[..n].to_vec();

        // ============================================================
        // Phase 3: v₂ 值 → 目标 x, 贪心碰撞解决 → 紧凑格点
        // ============================================================
        let (x, y) = grid_fill_2d(&v2, &v3, board, n, &placeable, circuit, preprocess);

        Self {
            placeable,
            is_bridgeable: vec![false; n],
            bridged: vec![false; n],
            bridged_pin_pairs: vec![Vec::new(); n],
            active_bridge_idx: vec![0; n],
            x,
            y,
            rotation: vec![Rotation::R0; n],
            r90_only: vec![false; n],
            y_locked: vec![None; n],
        }
    }
}
