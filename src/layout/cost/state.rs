//! `SAState`: SA 内部状态 (每个元件的 (x, y, rotation) + 桥接候选)。
//!
//! SA 主循环原地应用 move；拒绝时用 move 对应的 backup 完整恢复。只有保存
//! best state 或生成 progress snapshot 时才克隆整个状态。

use std::collections::HashMap;

use crate::circuit::{Circuit, ComponentId, PinId};
use crate::layout::breadboard::{Breadboard, HoleId};
use crate::layout::placement::{BBox, PlacedFootprint, Rotation, rotate};
use crate::layout::preprocess::PreprocessResult;
use crate::layout::problem::AnnealProblem;

use super::legalize::{PlacementHints, legalize};
use super::spectral::{compute_fiedler, compute_second_evec, spectral_hints};
use super::{Weights, cost_with_problem};

pub(super) struct InitialGeometry {
    pins: Vec<(i32, i32, Option<crate::circuit::NetId>)>,
    bbox: BBox,
}

impl InitialGeometry {
    pub(super) fn new(
        component: &crate::circuit::Component,
        circuit: &Circuit,
        rotation: Rotation,
    ) -> Self {
        let footprint =
            &circuit.footprints[component.footprint.expect("placeable 必有 footprint").raw()];
        let mut pins = Vec::with_capacity(component.pins.len());
        let points: Vec<crate::circuit::Position> = footprint
            .pins()
            .iter()
            .map(|pin| rotate(pin.offset, rotation))
            .collect();
        for &pin_id in &component.pins {
            let pin = &circuit.pins[pin_id.raw()];
            let physical = footprint
                .physical_pin_for(pin)
                .expect("footprint 缺 pin (解析阶段就该爆)");
            let offset = rotate(physical.offset, rotation);
            pins.push((offset.x, offset.y, pin.net));
        }
        Self {
            pins,
            bbox: BBox::from_points(points).unwrap_or(BBox {
                min_x: 0,
                max_x: 0,
                min_y: 0,
                max_y: 0,
            }),
        }
    }
}

#[derive(Clone)]
pub(super) struct InitialOccupancy {
    occupied: std::collections::HashSet<(i32, i32)>,
    rail_owners: HashMap<u32, Option<crate::circuit::NetId>>,
}

impl InitialOccupancy {
    pub(super) fn new(problem: &AnnealProblem) -> Self {
        Self {
            occupied: problem.fixed_geometry.occupied_cells.clone(),
            rail_owners: problem.fixed_geometry.rail_owners.clone(),
        }
    }

    pub(super) fn try_reserve(
        &mut self,
        board: &Breadboard,
        geometry: &InitialGeometry,
        x: i32,
        y: i32,
        allow_channel_crossing: bool,
    ) -> bool {
        let mut candidate_owners: HashMap<u32, Option<crate::circuit::NetId>> = HashMap::new();
        for &(offset_x, offset_y, net) in &geometry.pins {
            let Some(hole) = board.at(x + offset_x, y + offset_y) else {
                return false;
            };
            let rail = board.effective_rail_id_of(hole);
            if self
                .rail_owners
                .get(&rail)
                .is_some_and(|owner| *owner != net)
                || candidate_owners
                    .get(&rail)
                    .is_some_and(|owner| *owner != net)
            {
                return false;
            }
            candidate_owners.entry(rail).or_insert(net);
        }

        for offset in geometry.bbox.iter_cells() {
            let cell_x = x + offset.x;
            let cell_y = y + offset.y;
            if cell_x < 0
                || cell_x >= board.cols() as i32
                || cell_y < 0
                || cell_y >= board.main_rows() as i32
            {
                return false;
            }
            let has_hole = board.at(cell_x, cell_y).is_some();
            if (!has_hole && !allow_channel_crossing)
                || (has_hole && self.occupied.contains(&(cell_x, cell_y)))
            {
                return false;
            }
        }

        for offset in geometry.bbox.iter_cells() {
            let cell = (x + offset.x, y + offset.y);
            if board.at(cell.0, cell.1).is_some() {
                self.occupied.insert(cell);
            }
        }
        self.rail_owners.extend(candidate_owners);
        true
    }

    /// Reserve an already-projected placement such as a Bridged component.
    /// Cells outside the board are ignored, matching `Occupancy::from_layout` for a
    /// body suspended between a power rail and the main board.
    pub(super) fn try_reserve_placed(
        &mut self,
        board: &Breadboard,
        placed: &PlacedFootprint,
        circuit: &Circuit,
    ) -> bool {
        let mut candidate_owners: HashMap<u32, Option<crate::circuit::NetId>> = HashMap::new();
        for pin_hole in &placed.pin_holes {
            let position = board.hole(pin_hole.hole).position;
            if self.occupied.contains(&(position.x, position.y)) {
                return false;
            }
            let rail = board.effective_rail_id_of(pin_hole.hole);
            let net = circuit.pins[pin_hole.pin.raw()].net;
            if self
                .rail_owners
                .get(&rail)
                .is_some_and(|owner| *owner != net)
                || candidate_owners
                    .get(&rail)
                    .is_some_and(|owner| *owner != net)
            {
                return false;
            }
            candidate_owners.entry(rail).or_insert(net);
        }

        if let Some(bbox) = placed.bbox
            && bbox.iter_cells().any(|position| {
                board.at(position.x, position.y).is_some()
                    && self.occupied.contains(&(position.x, position.y))
            })
        {
            return false;
        }

        for pin_hole in &placed.pin_holes {
            let position = board.hole(pin_hole.hole).position;
            self.occupied.insert((position.x, position.y));
        }
        if let Some(bbox) = placed.bbox {
            self.occupied.extend(
                bbox.iter_cells()
                    .filter(|position| board.at(position.x, position.y).is_some())
                    .map(|position| (position.x, position.y)),
            );
        }
        self.rail_owners.extend(candidate_owners);
        true
    }
}

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

    /// 顺序初始状态: 按元件顺序构造位置提示，由公共 beam legalizer 保留多个
    /// 合法部分布局，并以完整状态的真实 SA cost 选优。
    ///
    /// 两个初排 (`from_greedy` / `from_spectral`) 都
    /// 做这个检查, 因此初排结果**保证**不引入列短路 — SA 后续只在 `Flip` /
    /// `ShiftX` 偶尔重新引入时捕捉并罚分。
    #[cfg(test)]
    pub(crate) fn from_greedy(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
        preprocess: &PreprocessResult,
        problem: &AnnealProblem,
    ) -> Result<Self, crate::layout::LayoutError> {
        Self::from_greedy_with_weights(
            placeable,
            circuit,
            board,
            preprocess,
            problem,
            &Weights::default(),
        )
    }

    pub(crate) fn from_greedy_with_weights(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
        preprocess: &PreprocessResult,
        problem: &AnnealProblem,
        weights: &Weights,
    ) -> Result<Self, crate::layout::LayoutError> {
        let n = placeable.len();
        let valid_rows: Vec<i32> = (0..board.main_rows() as i32)
            .filter(|row| !board.is_blocked(*row as usize))
            .collect();
        let row_preferences = placeable
            .iter()
            .map(|component| {
                preprocess
                    .y_locked
                    .get(component)
                    .map_or_else(|| valid_rows.clone(), |row| vec![*row])
            })
            .collect();
        legalize(
            placeable,
            circuit,
            board,
            preprocess,
            problem,
            weights,
            PlacementHints {
                order: (0..n).collect(),
                target_x: vec![0; n],
                row_preferences,
            },
        )
    }

    /// 频谱布局初排: 图拉普拉斯 2D 嵌入 → 网格填充。
    ///
    /// 流程:
    /// 1. Net-star 图: 每个 net 作为虚拟节点, 元件只连到所属 net
    ///    (避免 pairwise 展开形成的虚假吸引团)
    /// 2. 拉普拉斯 + 幂迭代 → v₂, v₃ (只取元件节点对应分量)
    /// 3. v₂/v₃ → 位置提示, 公共 beam legalizer → 紧凑合法格点
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
        problem: &AnnealProblem,
    ) -> Result<Self, crate::layout::LayoutError> {
        Self::from_spectral_with_weights(
            placeable,
            circuit,
            board,
            seed,
            preprocess,
            problem,
            &Weights::default(),
        )
    }

    pub(crate) fn from_spectral_with_weights(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
        seed: u64,
        preprocess: &PreprocessResult,
        problem: &AnnealProblem,
        weights: &Weights,
    ) -> Result<Self, crate::layout::LayoutError> {
        let n = placeable.len();
        if n <= 2 {
            return Self::from_greedy_with_weights(
                placeable, circuit, board, preprocess, problem, weights,
            );
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
        // Phase 3: v₂/v₃ → 位置提示，公共 legalizer 负责合法落格与真实 cost 选优。
        // ============================================================
        let neg_v2: Vec<f64> = v2.iter().map(|value| -*value).collect();
        let neg_v3: Vec<f64> = v3.iter().map(|value| -*value).collect();
        [(&v2, &v3), (&neg_v2, &v3), (&v2, &neg_v3), (&v3, &v2)]
            .into_iter()
            .map(|(horizontal, vertical)| {
                let hints = spectral_hints(horizontal, vertical, board, &placeable, preprocess)?;
                let state = legalize(
                    placeable.clone(),
                    circuit,
                    board,
                    preprocess,
                    problem,
                    weights,
                    hints,
                )?;
                let cost = cost_with_problem(&state, circuit, board, problem, weights);
                Ok((cost, state))
            })
            .collect::<Result<Vec<_>, crate::layout::LayoutError>>()?
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, state)| state)
            .ok_or(crate::layout::LayoutError::NoLegalInitialPlacement {
                component: placeable[0],
            })
    }
}
