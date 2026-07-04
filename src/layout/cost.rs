//! 模拟退火用的成本函数: MST 走线估算 + pin 碰撞 + bbox 碰撞 + 越界 + 列冲突 + 紧凑度。
//!
//! 设计要点:
//! - **MST (Minimum Spanning Tree) on pin positions**: 每个 net 算一次 Kruskal
//!   MST, 边长按 breadboard 物理距离计算:
//!   - **同列同 rail: 0** (rail 短接, 无需 wire)
//!   - **同 rail 不同 col: |Δcol|** (走 jumper wire 在同一行)
//!   - **同 col 不同 rail: |Δrow|** (跨中央通道)
//!   - **不同 col 不同 rail: |Δcol| + |Δrow|** (Manhattan)
//!
//!   比 2D HPWL 准 — 普通 HPWL 把 "同列不同 row 同 rail" 算成 Δrow, MST 直接 0,
//!   推动 SA 主动寻找 rail 短接的低跳线数布局。
//! - **紧凑度**: 按 rail 分组算 union bbox 面积加和, 阻止 SA 停在"零冲突但留白大"的状态。
//!   按 rail 切分避免中央通道把"实际占 1 行"的布局算成跨 2 行。
//! - 成本是各项**加权和**, 权在 [`Weights`] 里调。
//! - `SAState` 是 SA 内部状态, 只在 layout 子模块内共享; v2 起每个元件显式
//!   持有 `(x, y, rotation)`, 不再由 order 推 x。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, Component, ComponentId, NetId, PinId, Position};
use crate::layout::breadboard::{Breadboard, HoleId, Polarity, Region};
use crate::layout::placement::{BBox, Rotation, rotate};

/// SA 成本函数的六项权重。
///
/// 成本 = `mst * MST_sum + pin_overlap * pin_pin_碰撞 + b_box_overlap * bbox_重叠格数
///       + column_conflict * 列短路对数 + out_of_bounds * 越界 pin 数
///       + compactness * (按 rail 分组的 union bbox 面积之和) + rail_crossing (用 2+ rail 时)`
///
/// 默认值见 [`Weights::default`], 经验起点; 真用时按板子拥挤程度调。
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    /// MST (Minimum Spanning Tree) 走线总长的权重。
    /// 每个 net 跑一次 Kruskal, 边权按 breadboard 物理距离 (同 rail = 0,
    /// 不同 rail = Manhattan), sum 起来乘以本权重。
    /// 比纯 HPWL 准: HPWL 算同列不同 row 的距离仍按 Δrow, MST 直接 0。
    pub mst: f64,
    pub pin_overlap: f64,
    /// bbox 碰撞总格数 (本体撞 pin 也算)。一般比 pin_overlap 略高 — 本体挤到
    /// 其它元件身体上比 pin 互相碰还要糟糕 (后面 wire 还会避开本体)。
    pub b_box_overlap: f64,
    /// 同列不同 net 的 pin 对数 (N 个 pin 同列冲突就是 N-1 + N-2 + ... + 1 = N(N-1)/2)
    pub column_conflict: f64,
    pub out_of_bounds: f64,
    /// 紧凑度: 按 rail 分组, 每组算 union bbox 面积 `(max_x - min_x + 1) * (max_y - min_y + 1)`,
    /// 各 rail 加和。**按 rail 切分** 是为了避免中央通道把"实际只占 1 行"的布局算成 2 行高。
    /// 推动 SA 把元件挤到 union bbox 最小处 (x 和 y 同等对待, 都从 area 项自然得到)。
    pub compactness: f64,
    /// 同时使用 2+ 个 rail 时的额外固定惩罚, 鼓励同 rail 排布。
    /// 跨 rail 至少要一根 ~3 孔 jumper, 这项比单 cell 紧凑更贵。
    pub rail_crossing: f64,
    /// 纵向利用率惩罚: 同一 rail 内, 元件数量 vs 实际占用的行数。
    /// `penalty = Σ max(0, n_comps - unique_rows)`, 推 SA 把元件散布到不同行,
    /// 避免所有元件挤在同一行导致水平跨度过大。
    pub row_squash: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            // 一根 5 孔 wire 省下 ~25 成本 (mst=5); 大权重推动 SA 压低跳线数。
            mst: 5.0,
            // 一次 pin 碰撞 = 让 SA 宁愿多绕 50-100 孔也不撞
            pin_overlap: 100.0,
            // bbox 重叠基本也当硬约束, 跟 pin 碰撞同量级 (一个孔算 1)。
            b_box_overlap: 100.0,
            // 同列不同 net 的 pin 会被面包板竖向 rail 短接, 这是物理电气短路,
            // 不能让走线"治愈"。惩罚拉到 out_of_bounds 同级, 让 SA 当作硬约束。
            column_conflict: 1_000_000.0,
            // 越界基本不允许; 巨大惩罚让 SA 直接拒绝
            out_of_bounds: 1_000_000.0,
            // 紧凑度: 1 cell² ≈ 0.5 MST cell 的代价, 让 MST 仍有空间优化跨列 net,
            // 但空隙会被这股力挤掉。
            compactness: 0.5,
            // 跨 rail = 多一根 jumper + 视觉割裂, 取约 5 cell MST, 比单 cell 紧凑贵
            // 但比 column_conflict 软得多, 不会让 SA 为了"必须跨 rail 的电路"去撞列冲突。
            rail_crossing: 5.0,
            // 纵向挤压: 同一 rail 内元件挤在少量行 → 加罚。
            // 1.0 等价于 ~2 cell² 紧凑度, 比 MST 的 5.0 轻, 给 SA 温和推力。
            row_squash: 1.0,
        }
    }
}

/// SA 内部状态: 每个元件显式持有 `(x, y, rotation)`.
///
/// v2 起 (显式 2D 布局), x 不再由 order 推导, SA 可以把元件放到板子任意位置。
/// order 还在, 但只用于标识; `placeable[i]` 的 i 索引对应 `x[i] / y[i] / rotation[i]`。
///
/// `is_bridgeable[i] / bridged[i] / bridged_pin_pairs[i] / active_bridge_idx[i]`
/// 是桥接探索的四个字段, 长度都 == `placeable.len()`。`is_bridgeable` 由
/// `place_sa` 在 SA 启动前根据 `Component.bridgeable` + 启发式是否找到合法桥接位
/// 决定; `bridged` 初始全 false (默认 OnBoard), SA 通过 `Move::ToggleBridging`
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
}

// ============================================================
//  预计算数据: 避免在 cost() 热路径里重复查 footprint / 算 bbox
// ============================================================

/// 每个 placeable 元件的预计算信息 (只依赖 circuit/footprint, 不随 SA 状态变)。
#[derive(Debug, Clone)]
pub struct CompInfo {
    /// 每个 pin 的预计算数据, 按 component.pins 顺序。
    /// (R0 local offset, R180 local offset, net)
    pub pins: Vec<(Position, Position, Option<NetId>)>,
    /// 该元件 footprint 在 R0 旋转下的局部坐标 bbox
    pub bbox_r0: BBox,
}

/// SA 上下文: 预计算数据 + reusable buffers。
/// 在 simulate() 入口构造一次, 所有 cost 调用复用。
pub struct SAContext {
    pub comp_infos: Vec<CompInfo>,
}

impl SAContext {
    /// 从 circuit 和 placeable 列表预计算所有组件的 footprint 信息。
    pub fn new(circuit: &Circuit, placeable: &[ComponentId]) -> Self {
        let mut comp_infos = Vec::with_capacity(placeable.len());
        for &comp_id in placeable {
            let component = &circuit.components[comp_id.0];
            let fid = component.footprint.expect("placeable 必有 footprint");
            let footprint = &circuit.footprints[fid.0];

            let mut pins = Vec::with_capacity(component.pins.len());
            let mut world_positions: Vec<Position> = Vec::with_capacity(component.pins.len());

            for &pin_id in &component.pins {
                let pin = &circuit.pins[pin_id.0];
                let physical = footprint
                    .pins()
                    .iter()
                    .find(|pp| pp.name() == pin.num())
                    .expect("footprint 缺 pin (解析阶段就该爆)");
                let offset_r0 = physical.offset;
                // R180: negate
                let offset_r180 = Position {
                    x: -offset_r0.x,
                    y: -offset_r0.y,
                };
                pins.push((offset_r0, offset_r180, pin.net));
                world_positions.push(offset_r0);
            }

            let bbox_r0 = BBox::from_points(world_positions).unwrap_or(BBox {
                min_x: 0,
                max_x: 0,
                min_y: 0,
                max_y: 0,
            });

            comp_infos.push(CompInfo { pins, bbox_r0 });
        }

        SAContext { comp_infos }
    }
}

// ============================================================
//  Reusable Buffers: 避免 cost() 内部重复分配
// ============================================================

/// 所有 cost 计算复用的缓冲区。
/// 在 simulate() 里创建一次, 每次 cost 计算前 clear 后重用。
pub(crate) struct CostBuf {
    pub holes: Vec<(i32, i32, u32)>,
    pub nets: Vec<Option<NetId>>,
    pub is_virtual: Vec<bool>,
    pub bboxes: Vec<Option<BBox>>,
    /// net_id → pin 在 holes/nets 里的 index 列表 (按 net.0 索引)
    pub net_buckets: Vec<Vec<usize>>,
    /// rail_id → net 列表
    pub rail_map: HashMap<u32, Vec<Option<NetId>>>,
    /// rail_top → bbox 列表 (用于紧凑度)
    pub compact_map: HashMap<i32, Vec<BBox>>,
    /// 用于 pin 碰撞检测的 reusable set
    pub pin_seen: HashSet<(i32, i32, u32)>,
}

impl CostBuf {
    pub fn new(num_nets: usize) -> Self {
        Self {
            holes: Vec::new(),
            nets: Vec::new(),
            is_virtual: Vec::new(),
            bboxes: Vec::new(),
            net_buckets: vec![Vec::new(); num_nets],
            rail_map: HashMap::new(),
            compact_map: HashMap::new(),
            pin_seen: HashSet::new(),
        }
    }

    /// 清理所有 buffer 以便下一轮 cost 计算复用
    fn clear(&mut self) {
        self.holes.clear();
        self.nets.clear();
        self.is_virtual.clear();
        self.bboxes.clear();
        for bucket in &mut self.net_buckets {
            bucket.clear();
        }
        for v in self.rail_map.values_mut() {
            v.clear();
        }
        for v in self.compact_map.values_mut() {
            v.clear();
        }
        self.pin_seen.clear();
    }
}

// ============================================================
//  SAState impl
// ============================================================

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
        }
    }

    /// 简单构造: 给定元件顺序, 全部 R0, 全部同一行, x 按顺序累加 (gap=1)。
    /// 主要给测试用——真实初始状态用 [`SAState::from_greedy`]。不填桥接信息:
    /// 桥接字段 (is_bridgeable / bridged / bridged_pin_pair) 默认全 false / None,
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
        }
    }

    /// 贪心 first-fit 初始状态: 按元件顺序, 找第一个有效 `(x, y)` (按行从上到下、
    /// 列从左到右扫)。"有效" = 所有 pin 都在板内 + 包围盒不撞已摆元件的 bbox +
    /// 不引入列冲突 (同列同 rail 不同 net 的 pin)。**不**考虑列短路——那由 SA 后续优化
    /// (FD 初排已避免大部分, SA 翻元件时若重新引入, cost 会捕捉并罚分)。
    pub fn from_greedy(placeable: Vec<ComponentId>, circuit: &Circuit, board: &Breadboard) -> Self {
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

            let (fx, fy) = found.unwrap_or_else(|| panic!("元件 {} 装不下这块板", comp_id.0));
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
        }
    }

    /// 力导向初排: connected 元件拉近 (弹簧), unconnected 推远 (库仑斥力)。
    /// 产出连续 2D 位置, 然后贪心映射到网格上 (按 FD x 顺序, 每个取离 FD 目标最近
    /// 的可用格子)。比 [`Self::from_greedy`] 好的地方: 同一网里的元件自然聚簇,
    /// SA 起点跟电路拓扑对齐。
    pub fn from_force_directed(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
        config: &FDConfig,
    ) -> Self {
        let n = placeable.len();
        if n == 0 {
            return Self {
                placeable,
                is_bridgeable: vec![],
                bridged: vec![],
                bridged_pin_pairs: vec![],
                active_bridge_idx: vec![],
                x: vec![],
                y: vec![],
                rotation: vec![],
            };
        }

        // 1. Build comp_id → placeable_idx mapping
        let mut comp_to_idx: HashMap<ComponentId, usize> = HashMap::with_capacity(n);
        for (i, &cid) in placeable.iter().enumerate() {
            comp_to_idx.insert(cid, i);
        }

        // 2. Build adjacency: weights[i][j] = 同一网的连接数 (高 = 强耦合)
        let mut weights = vec![vec![0.0f64; n]; n];
        for net in circuit.nets() {
            let mut comps: Vec<usize> = net
                .pins()
                .iter()
                .filter_map(|&pid| comp_to_idx.get(&circuit.pins[pid.0].component).copied())
                .collect();
            comps.sort();
            comps.dedup();
            for &i in &comps {
                for &j in &comps {
                    if i != j {
                        weights[i][j] += 1.0;
                    }
                }
            }
        }

        // 2. Initial: 圆周, 后面 FD 会把它们拉开
        let cols_f = board.cols() as f64;
        let rows_f = board.rows() as f64;
        let mut pos: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let angle = i as f64 * 2.0 * std::f64::consts::PI / n as f64;
                let r = cols_f.min(rows_f) * 0.4;
                (
                    cols_f / 2.0 + r * angle.cos(),
                    rows_f / 2.0 + r * angle.sin(),
                )
            })
            .collect();

        // 3. Fruchterman-Reingold 风格 FD 迭代
        let k = config.k;
        let mut temp = config.initial_temp;
        for _ in 0..config.max_iters {
            let mut forces = vec![(0.0f64, 0.0f64); n];
            for i in 0..n {
                for j in (i + 1)..n {
                    let dx = pos[j].0 - pos[i].0;
                    let dy = pos[j].1 - pos[i].1;
                    let dist = (dx * dx + dy * dy).sqrt().max(0.01);
                    let ux = dx / dist;
                    let uy = dy / dist;

                    // 斥力: 库仑型, 永远存在
                    let f_repel = k * k / dist;
                    forces[i].0 -= ux * f_repel;
                    forces[i].1 -= uy * f_repel;
                    forces[j].0 += ux * f_repel;
                    forces[j].1 += uy * f_repel;

                    // 引力: 胡克型, 仅对连接的元件
                    let w = weights[i][j];
                    if w > 0.0 {
                        let f_attr = dist * dist / k * w;
                        forces[i].0 += ux * f_attr;
                        forces[i].1 += uy * f_attr;
                        forces[j].0 -= ux * f_attr;
                        forces[j].1 -= uy * f_attr;
                    }
                }
            }

            // 应用力, 限幅到当前温度
            for i in 0..n {
                let (fx, fy) = forces[i];
                let fmag = (fx * fx + fy * fy).sqrt();
                if fmag < 1e-9 {
                    continue;
                }
                let scale = fmag.min(temp) / fmag;
                pos[i].0 += fx * scale;
                pos[i].1 += fy * scale;
                pos[i].0 = pos[i].0.clamp(0.0, cols_f - 1.0);
                // clamp 到 [0, rows-1] 之后, 如果 y 落在 blocked row
                // (面包板中央通道), 推到最近的非 blocked row。中央通道不是元件
                // 能用的空间; 不推开的话 FD 会把元件目标持续累积在 row 5/6
                // 附近, 后续贪心映射把这些目标"撕"到上下两半, 出现无意义的跨 rail 布局。
                let mut y = pos[i].1.clamp(0.0, rows_f - 1.0);
                if board.is_blocked(y as usize) {
                    let mut best_y = y as i32;
                    let mut best_dist = f64::INFINITY;
                    for r in 0..board.rows() {
                        if board.is_blocked(r) {
                            continue;
                        }
                        let d = (r as f64 - y).abs();
                        if d < best_dist {
                            best_dist = d;
                            best_y = r as i32;
                        }
                    }
                    y = best_y as f64;
                }
                pos[i].1 = y;
            }

            temp *= config.cool_rate;
            if temp < 0.05 {
                break;
            }
        }

        // 4. 贪心映射到网格: 按 FD x 排序, 每个元件取离 FD 目标最近的可用格
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            pos[a]
                .0
                .partial_cmp(&pos[b].0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut x = vec![0i32; n];
        let mut y = vec![0i32; n];
        let rotation = vec![Rotation::R0; n];
        let mut occupied: HashSet<(i32, i32)> = HashSet::new();
        // (col, rail_top) → 该列第一个 pin 的 net。后续 pin 若 net 不同则为列冲突。
        // 这是 SA 代价函数里 "列冲突" 定义的同构； FD 初排避免列冲突后,
        // SA 只需要在 Flip / ShiftX 偶尔引入冲突时退回去, 不会一开始就被 1M 压死。
        let mut col_owner: HashMap<(i32, i32), Option<crate::circuit::NetId>> = HashMap::new();

        for &idx in &order {
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
            // bbox 用 footprint 全部物理 pin 算 (保留原 from_force_directed 行为)。
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

            let target_x = pos[idx].0;
            let target_y = pos[idx].1;

            // 全板扫, 选离 (target_x, target_y) 最近的可用格
            let mut best: Option<(i32, i32)> = None;
            let mut best_dist_sq = f64::INFINITY;
            for try_y in 0..board.rows() as i32 {
                if board.is_blocked(try_y as usize) {
                    continue;
                }
                for try_x in 0..board.cols() as i32 {
                    // (1) OOB / blocked 检查
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
                    // (2) 与已占 cell 碰撞检查
                    let collides = bbox_cells
                        .iter()
                        .any(|&(dx, dy)| occupied.contains(&(try_x + dx, try_y + dy)));
                    if collides {
                        continue;
                    }
                    // (3) 列冲突检查: 任一 pin 落在 (col, rail_top) 上,
                    //     而该位置已有不同 net 的 pin → 跳过。
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
                    let dx = try_x as f64 - target_x;
                    let dy = try_y as f64 - target_y;
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                        best = Some((try_x, try_y));
                    }
                }
            }

            let (fx, fy) = best.expect("板太小, 装不下所有元件");
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
    pub fn from_spectral(
        placeable: Vec<ComponentId>,
        circuit: &Circuit,
        board: &Breadboard,
    ) -> Self {
        let n = placeable.len();
        if n <= 2 {
            return Self::from_greedy(placeable, circuit, board);
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
        let v2_all = compute_fiedler(&l, total_n);
        let v3_all = compute_second_evec(&l, &v2_all, total_n);
        let v2: Vec<f64> = v2_all[..n].to_vec();
        let v3: Vec<f64> = v3_all[..n].to_vec();

        // ============================================================
        // Phase 3: v₂ 值 → 目标 x, 贪心碰撞解决 → 紧凑格点
        // ============================================================
        let (x, y) = grid_fill_2d(&v2, &v3, board, n, &placeable, circuit);

        Self {
            placeable,
            is_bridgeable: vec![false; n],
            bridged: vec![false; n],
            bridged_pin_pairs: vec![Vec::new(); n],
            active_bridge_idx: vec![0; n],
            x,
            y,
            rotation: vec![Rotation::R0; n],
        }
    }
}

// ============================================================
//  频谱布局辅助函数
// ============================================================

/// 幂迭代求 Fiedler 向量 (拉普拉斯 L 的第二小特征向量)。
///
/// L 的最小特征值为 0, 对应常向量 [1,1,...,1]。
/// 对 M = cI - L 做幂迭代, 投射掉常向量分量, 收敛到 Fiedler。
fn compute_fiedler(l: &[Vec<f64>], n: usize) -> Vec<f64> {
    // c > λ_max(L)。λ_max ≤ 2·max_degree
    let max_deg = (0..n).map(|i| l[i][i]).fold(0.0f64, f64::max);
    let c = if max_deg > 0.0 { 2.0 * max_deg } else { 1.0 };

    let mut v: Vec<f64> = (0..n).map(|_| fastrand::f64() - 0.5).collect();
    project_out_constant(&mut v, n);
    normalize_vec(&mut v);

    for _ in 0..300 {
        let mut w = mat_vec_mul_shifted(l, &v, c, n);
        project_out_constant(&mut w, n);
        if !normalize_vec(&mut w) {
            break;
        }
        v = w;
    }
    v
}

/// 幂迭代求第三特征向量 v₃ (正交于常向量和 v₂)。
fn compute_second_evec(l: &[Vec<f64>], v2: &[f64], n: usize) -> Vec<f64> {
    let max_deg = (0..n).map(|i| l[i][i]).fold(0.0f64, f64::max);
    let c = if max_deg > 0.0 { 2.0 * max_deg } else { 1.0 };

    let mut v: Vec<f64> = (0..n).map(|_| fastrand::f64() - 0.5).collect();
    project_out_two(&mut v, v2, n);
    normalize_vec(&mut v);

    for _ in 0..300 {
        let mut w = mat_vec_mul_shifted(l, &v, c, n);
        project_out_two(&mut w, v2, n);
        if !normalize_vec(&mut w) {
            break;
        }
        v = w;
    }
    v
}

/// (cI - L) * v
fn mat_vec_mul_shifted(l: &[Vec<f64>], v: &[f64], c: f64, n: usize) -> Vec<f64> {
    let mut w = vec![0.0; n];
    for i in 0..n {
        w[i] = c * v[i];
        for j in 0..n {
            w[i] -= l[i][j] * v[j];
        }
    }
    w
}

/// 投射掉常向量分量: v ← v - mean(v)·1
fn project_out_constant(v: &mut [f64], n: usize) {
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    for vi in v.iter_mut() {
        *vi -= mean;
    }
}

/// 投射掉常向量和 v2 分量
fn project_out_two(v: &mut [f64], v2: &[f64], n: usize) {
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    let dot_v2: f64 = v.iter().zip(v2).map(|(a, b)| a * b).sum();
    for (i, vi) in v.iter_mut().enumerate() {
        *vi = *vi - mean - dot_v2 * v2[i];
    }
}

/// 归一化, 返回是否成功 (norm > 0)
fn normalize_vec(v: &mut [f64]) -> bool {
    let norm_sq: f64 = v.iter().map(|x| x * x).sum();
    if norm_sq < 1e-24 {
        return false;
    }
    let inv = 1.0 / norm_sq.sqrt();
    for vi in v.iter_mut() {
        *vi *= inv;
    }
    true
}

/// 频谱 → 格点映射: v₂ 值 → x 目标位置 (保聚类), v₃ rank → y 分布,
/// 然后贪心左紧排消碰撞。
///
/// v₂ 相近的元件 (同 net / 强耦合) 自然映射到相近的 x, 不像 rank 均匀分布
/// 那样把 5 个元件也摊满 60 列。`effective_width = min(n * 3, cols - 2)`
/// 进一步防止过散, 贪心碰撞解决保证无 pin/bbox/列冲突。
fn grid_fill_2d(
    v2: &[f64],
    v3: &[f64],
    board: &Breadboard,
    n: usize,
    placeable: &[ComponentId],
    circuit: &Circuit,
) -> (Vec<i32>, Vec<i32>) {
    let valid_rows: Vec<i32> = (0..board.rows() as i32)
        .filter(|&r| !board.is_blocked(r as usize))
        .collect();
    let n_rows = valid_rows.len().max(1);

    // ── v₂ 归一化到 [0, 1] (保留聚类信息) ──
    let v2_min = v2.iter().cloned().fold(f64::INFINITY, f64::min);
    let v2_max = v2.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let v2_range = (v2_max - v2_min).max(1e-9);

    // ── v₃ rank → y ──
    let mut order_y: Vec<usize> = (0..n).collect();
    order_y.sort_by(|&a, &b| {
        v3[a]
            .partial_cmp(&v3[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut rank_y = vec![0usize; n];
    for (rank, &idx) in order_y.iter().enumerate() {
        rank_y[idx] = rank;
    }

    // ── v₂ 排序决定从左到右的贪心放置顺序 ──
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        v2[a]
            .partial_cmp(&v2[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let cols = board.cols() as i32;

    // 有效宽度: 每个元件 ~3 列 (自身 + 间距), 上限为板宽
    let effective_width = (n as i32 * 3).max(2).min(cols - 2);

    // 目标 x: 由 v₂ 值决定, 缩放至有效宽度
    let mut target_x = vec![0i32; n];
    for i in 0..n {
        let frac = (v2[i] - v2_min) / v2_range;
        target_x[i] = (1.0 + frac * effective_width as f64) as i32;
        target_x[i] = target_x[i].clamp(0, cols - 1);
    }

    // 目标 y
    let mut target_y = vec![0i32; n];
    for i in 0..n {
        target_y[i] = valid_rows[rank_y[i] % n_rows];
    }

    // ── 贪心碰撞解决: v₂ 顺序, 从目标位置向右扫 (保持聚类顺序) ──
    let mut x = vec![0i32; n];
    let mut y = vec![0i32; n];
    let mut occupied: HashSet<(i32, i32)> = HashSet::new();
    let mut col_owner: HashMap<(i32, i32), Option<NetId>> = HashMap::new();

    for &idx in &order {
        let comp_id = placeable[idx];
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.expect("placeable 必有 footprint");
        let footprint = &circuit.footprints[fid.0];

        // pin 信息：(本地 offset, net) — 用于列冲突检查
        let pin_info: Vec<(i32, i32, Option<NetId>)> = component
            .pins
            .iter()
            .map(|&pin_id| {
                let pin = &circuit.pins[pin_id.0];
                let physical = footprint
                    .pins()
                    .iter()
                    .find(|p| p.name() == pin.num())
                    .expect("footprint 缺 pin");
                (physical.offset.x, physical.offset.y, pin.net)
            })
            .collect();

        // bbox 用 footprint 全部物理 pin 算
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

        // 从目标位置出发, 左右交替扩展, 同 row 优先, 再换行
        let mut best: Option<(i32, i32)> = None;
        'search: for dx in 0..=cols {
            for &x_sign in &[1i32, -1i32] {
                if dx == 0 && x_sign == -1 {
                    continue; // 跳过 dx=0 的重复
                }
                let try_x = target_x[idx] + x_sign * dx;
                if try_x < 0 || try_x >= cols {
                    continue;
                }
                // 优先目标行, 然后上下轮替
                for dy in 0..n_rows as i32 {
                    for &dy_sign in &[0i32, 1i32, -1i32] {
                        if dy == 0 && dy_sign != 0 {
                            continue;
                        }
                        let try_y_idx =
                            (rank_y[idx] as i32 + dy_sign * dy).rem_euclid(n_rows as i32) as usize;
                        let try_y = valid_rows[try_y_idx];

                        // OOB / blocked
                        let oob_or_blocked = bbox_cells.iter().any(|&(ox, oy)| {
                            let ax = try_x + ox;
                            let ay = try_y + oy;
                            ax < 0
                                || ax >= cols
                                || ay < 0
                                || ay >= board.rows() as i32
                                || board.is_blocked(ay as usize)
                        });
                        if oob_or_blocked {
                            continue;
                        }
                        // bbox 碰撞
                        let collides = bbox_cells
                            .iter()
                            .any(|&(ox, oy)| occupied.contains(&(try_x + ox, try_y + oy)));
                        if collides {
                            continue;
                        }
                        // 列冲突
                        let col_conflict = pin_info.iter().any(|&(lx, ly, pin_net)| {
                            let abs_x = try_x + lx;
                            let abs_y = try_y + ly;
                            if abs_x < 0
                                || abs_x >= cols
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
                        best = Some((try_x, try_y));
                        break 'search;
                    }
                }
            }
        }

        let (fx, fy) =
            best.unwrap_or_else(|| panic!("板太小, 装不下元件 {} (spectral grid fill)", comp_id.0));
        x[idx] = fx;
        y[idx] = fy;
        for &(ox, oy) in &bbox_cells {
            occupied.insert((fx + ox, fy + oy));
        }
        for &(lx, ly, pin_net) in &pin_info {
            let abs_x = fx + lx;
            let abs_y = fy + ly;
            if abs_x >= 0
                && abs_x < cols
                && abs_y >= 0
                && abs_y < board.rows() as i32
                && !board.is_blocked(abs_y as usize)
            {
                let rail_top = board.rail_rows(abs_y).first().copied().unwrap_or(abs_y);
                col_owner.entry((abs_x, rail_top)).or_insert(pin_net);
            }
        }
    }

    (x, y)
}

// ============================================================
//  桥接探测
// ============================================================

/// 为 bridgeable 元件计算所有合法桥接 pin 对 (启发式)。
///
/// **输入**: 一个 bridgeable 元件 (必有 2 pin, 一腿 power net, 一腿 signal net)。
/// **输出**: 所有让 signal 落在主区合法孔的 `(power hole, signal hole)` 对,
///          按 `HoleId` 升序 × 旋转 `[R0, R90, R180, R270]` 顺序扫。空 Vec
///          表示没找到任何合法对 (启发式失败, 该元件保持 OnBoard)。
///
/// **为什么需要 4 种旋转**: Bridged 路径下 body 浮在板外, 允许 R90/R270。
/// 对水平 footprint 的电阻 (pin offset Δx = L, Δy = 0), R0/R180 让 signal
/// pin 仍落在同一条 rail 上 (无意义), 只有 R90/R270 让 body 垂直于 rail
/// 插进主区。这是 Bridged 必须支持 R90/R270 的根因。
///
/// **为什么不存 rotation**: `Placement::Bridged` 只存 `(HoleId, PinId)` 对,
/// body 朝向是隐式的 (选哪两个孔就决定了 body 朝哪)。SA 的 `Move::Flip` 继续
/// 只作用于 OnBoard 路径, 不跟桥接路径争用。
///
/// **排序**: 返回的列表按 (matching-rail 优先, then other rail) × HoleId 升序
/// × 旋转 `[R0, R90, R180, R270]` 顺序排列。**注意**: 调用方 (`populate_bridgeable_info`)
/// 会基于 "signal pin 离同 net 中心最近" 重新排序, 启发式本身的顺序只决定 enumeration
/// 终止时机, 不影响最终结果。
pub(crate) fn propose_bridged_pairs(
    comp: &Component,
    circuit: &Circuit,
    board: &Breadboard,
    power_net_ids: &[NetId],
) -> Vec<[(HoleId, PinId); 2]> {
    debug_assert_eq!(comp.pins.len(), 2, "bridgeable 必有 2 pin");

    // 1. 分 power / signal pin
    let Some(power_pin_id) = comp
        .pins
        .iter()
        .find(|&&pid| {
            circuit.pins[pid.0]
                .net
                .map(|n| power_net_ids.contains(&n))
                .unwrap_or(false)
        })
        .copied()
    else {
        return Vec::new();
    };
    let signal_pin_id = comp
        .pins
        .iter()
        .find(|&&pid| pid != power_pin_id)
        .copied()
        .expect("bridgeable 必有 2 pin (debug_assert 已守)");
    let power_net = circuit.pins[power_pin_id.0].net;

    // 2. 查 footprint pad offsets
    let Some(fp_id) = comp.footprint else {
        return Vec::new();
    };
    let fp = &circuit.footprints[fp_id.0];
    let power_off = fp
        .pins()
        .iter()
        .find(|p| p.name() == circuit.pins[power_pin_id.0].num())
        .map(|p| p.offset);
    let signal_off = fp
        .pins()
        .iter()
        .find(|p| p.name() == circuit.pins[signal_pin_id.0].num())
        .map(|p| p.offset);
    let (Some(power_off), Some(signal_off)) = (power_off, signal_off) else {
        return Vec::new();
    };

    let delta = Position {
        x: signal_off.x - power_off.x,
        y: signal_off.y - power_off.y,
    };

    // 3. 优先扫: 那些 rail 的 bound net == power_net 的 power rail 孔。
    //    power pin 落在这种孔上后, pin 跟同 rail 的虚拟 pin 同 net,
    //    列冲突代价 = 0。如果 pin 的 net 未绑定或只绑到一种极性, 此集合可能为空。
    let matching_rail_ids = collect_matching_rail_ids(board, power_net);

    // 4. 两轮扫描: 先 matching, 后 fallback (任意 power rail)。
    //    fallback 代价高 (列冲突) 但优于 "启发式返 None, 走 OnBoard"。
    let all_power_holes: Vec<HoleId> = (0..board.holes().len())
        .map(HoleId)
        .filter(|h| board.region_of(*h) == Region::PowerRail)
        .collect();
    let (matching, other): (Vec<HoleId>, Vec<HoleId>) = all_power_holes
        .iter()
        .partition(|h| matching_rail_ids.contains(&board.rail_id_of(**h)));

    let mut out = Vec::new();
    for &h in matching.iter().chain(other.iter()) {
        let h_pos = board.hole(h).position;
        for &rot in &[Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let rotated = rotate(delta, rot);
            let signal_pos = Position {
                x: h_pos.x + rotated.x,
                y: h_pos.y + rotated.y,
            };
            if let Some(signal_h) = board.at(signal_pos.x, signal_pos.y)
                && board.region_of(signal_h) == Region::MainRail
            {
                out.push([(h, power_pin_id), (signal_h, signal_pin_id)]);
            }
        }
    }
    out
}

/// 旧 API 兼容: 返回**第一个**合法桥接对 (用于单对场景, 如向后兼容测试)。
/// 新代码请用 `propose_bridged_pairs` + `populate_bridgeable_info`。
#[cfg(test)]
pub(crate) fn propose_bridged_pair(
    comp: &Component,
    circuit: &Circuit,
    board: &Breadboard,
    power_net_ids: &[NetId],
) -> Option<[(HoleId, PinId); 2]> {
    propose_bridged_pairs(comp, circuit, board, power_net_ids)
        .into_iter()
        .next()
}

/// 收集 rail_id 集合: 这些 power rail 被 bound 到 `pin_net`。
/// `pin_net == None` 返空集 (用户不绑 → 没有 "net 匹配" 的 rail, 启发式走 fallback 扫所有 rail)。
fn collect_matching_rail_ids(
    board: &Breadboard,
    pin_net: Option<NetId>,
) -> std::collections::HashSet<u32> {
    let mut ids = std::collections::HashSet::new();
    let Some(pin_net) = pin_net else { return ids };
    let Some(binding) = board.power_rail_binding() else {
        return ids;
    };
    for (polarity, net_id) in [
        (Polarity::Negative, binding.negative),
        (Polarity::Positive, binding.positive),
    ] {
        if net_id != pin_net {
            continue;
        }
        if let Some(anchor) = board.power_rail_anchor(polarity) {
            ids.insert(board.rail_id_of(anchor));
        }
    }
    ids
}

/// 对 state 中所有 `Component.bridgeable = true` 的元件跑启发式, 填充
/// `is_bridgeable` / `bridged_pin_pairs` / `active_bridge_idx`。`bridged` 字段不动
/// (默认 false = OnBoard)。
///
/// 调用时机: `from_greedy` / `from_force_directed` 构造完 state 之后,
/// SA 启动前 (在 `sa::simulate` 内部)。`Component.bridgeable = false` 的元件
/// `is_bridgeable` 恒为 false, `bridged_pin_pairs` 为空 Vec —— `Move::ToggleBridging`
/// 不会命中它们。
///
/// **排序**: 对每个 bridgeable 元件, 启发式返回所有合法 (hole, rotation) 对。
/// 这里按 "signal pin 离同 net (signal pin 所在的 net) 其他元件 pin 的几何中心
/// 最近" 排序, 索引 0 = 最佳候选。SA 在 `ToggleBridging` 翻到 bridge 模式时会
/// 遍历这个列表、按 cost 重选并写回 `active_bridge_idx[idx]`。
///
/// **静态中心**: 用 SA 启动**前** state 里其他元件的 pin 位置算中心 (从
/// `state.x/y/rotation` 推, bridged 元件用 active_bridge_pair)。SA 跑起来后
/// 中心会变, 但我们不重算 —— SA 选 candidate 时是按 cost 选, 中心只是初始
/// 排序的 hint, 不会把真正最优的候选卡在后面 (因为候选列表只是初始偏置,
/// SA 会按 cost 重排)。
pub(crate) fn populate_bridgeable_info(
    state: &mut SAState,
    circuit: &Circuit,
    board: &Breadboard,
    power_net_ids: &[NetId],
) {
    let n = state.placeable.len();
    debug_assert_eq!(state.is_bridgeable.len(), n);
    debug_assert_eq!(state.bridged.len(), n);
    debug_assert_eq!(state.bridged_pin_pairs.len(), n);
    debug_assert_eq!(state.active_bridge_idx.len(), n);

    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        let comp = &circuit.components[comp_id.0];
        if !comp.bridgeable {
            continue;
        }
        let candidates = propose_bridged_pairs(comp, circuit, board, power_net_ids);
        if candidates.is_empty() {
            // 启发式返空: 该元件本轮无法桥接, is_bridgeable 保持 false,
            // Toggle 不会命中它 (随机退回其他 move, 不污染 seed 序列)。
            continue;
        }

        // 算 signal net 的几何中心 (用 state 当前 (x, y, rotation), bridged 元件
        // 用 active_bridge_pair)。只用于排序 hint, 精度不重要。
        let signal_net_id = comp
            .pins
            .iter()
            .map(|&pid| circuit.pins[pid.0].net)
            .find(|net_opt| net_opt.is_some() && !power_net_ids.contains(&net_opt.unwrap()))
            .flatten();
        let center = signal_net_id
            .and_then(|nid| compute_signal_net_center(circuit, board, state, nid, Some(comp_id)));

        // 按 "signal pin 离中心 Manhattan 距离" 排序, 距离小的优先。
        // 没有中心 (signal net 只此一个 pin) 时保持原顺序 (启发式扫的顺序)。
        if let Some(center) = center {
            let mut sorted: Vec<(i32, [(HoleId, PinId); 2])> = candidates
                .into_iter()
                .map(|pair| {
                    let signal_pos = board.hole(pair[1].0).position;
                    let dist = (signal_pos.x - center.x).abs() + (signal_pos.y - center.y).abs();
                    (dist, pair)
                })
                .collect();
            sorted.sort_by_key(|&(d, _)| d);
            state.bridged_pin_pairs[idx] = sorted.into_iter().map(|(_, p)| p).collect();
        } else {
            state.bridged_pin_pairs[idx] = candidates;
        }
        state.is_bridgeable[idx] = true;
        state.active_bridge_idx[idx] = 0; // 启发式最佳 = 索引 0
    }
}

/// 算一个 net 的几何中心 (各 pin 位置的平均)。排除 `exclude_comp` 的 pin
/// (避免启发式把自己要摆的 signal pin 也算进去, 造成 "候选间无差异" 的退化)。
/// 返回 None 当 net 上没有其他 pin (只有 bridgeable 自己一个)。
fn compute_signal_net_center(
    circuit: &Circuit,
    board: &Breadboard,
    state: &SAState,
    net_id: NetId,
    exclude_comp: Option<ComponentId>,
) -> Option<Position> {
    let mut sum_x: i64 = 0;
    let mut sum_y: i64 = 0;
    let mut count: i64 = 0;
    for &pid in &circuit.nets[net_id.0].pins {
        let pin = &circuit.pins[pid.0];
        if exclude_comp.is_some_and(|c| c == pin.component) {
            continue;
        }
        // 找这个 component 在 state.placeable 里的 idx
        let Some(idx) = state.placeable.iter().position(|&c| c == pin.component) else {
            continue;
        };
        // 推 pin 的世界坐标
        let pos = pin_world_pos(state, idx, pin, circuit, board);
        sum_x += pos.x as i64;
        sum_y += pos.y as i64;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some(Position {
        x: (sum_x / count) as i32,
        y: (sum_y / count) as i32,
    })
}

/// 推 `state.placeable[idx]` 的指定 pin 当前世界坐标。处理 bridged / OnBoard 两种路径。
fn pin_world_pos(
    state: &SAState,
    idx: usize,
    pin: &crate::circuit::Pin,
    circuit: &Circuit,
    board: &Breadboard,
) -> Position {
    if let Some(pair) = state.active_bridge_pair(idx) {
        // bridged: pair 里两条腿, 找 pin.num 跟哪条匹配
        for &(hole, pid) in &pair {
            if pid == pin.id {
                return board.hole(hole).position;
            }
        }
        // pin 不在该元件的桥接 pair 里 (例如该元件 3 pin 但 is_bridgeable=true 的罕见情况)
        // 退回 OnBoard 路径
    }
    let comp = &circuit.components[state.placeable[idx].0];
    let fp_id = comp.footprint.expect("placeable 元件必有 footprint");
    let fp = &circuit.footprints[fp_id.0];
    let physical = fp
        .pins()
        .iter()
        .find(|p| p.name() == pin.num())
        .expect("footprint 缺 pin (解析阶段就该爆)");
    let rotated = rotate(physical.offset, state.rotation[idx]);
    Position {
        x: state.x[idx] + rotated.x,
        y: state.y[idx] + rotated.y,
    }
}

/// 力导向初排参数。`Default` 对 30x5 / ~18 元件级别的电路是合理起点。
#[derive(Debug, Clone, Copy)]
pub struct FDConfig {
    /// 理想距离 k: 库仑斥力 k²/d, 胡克引力 d²/k 都用它。
    /// 经验上 ≈ sqrt(board_area / num_components)。
    pub k: f64,
    /// FD 迭代数; 通常 100-300 就收敛。
    pub max_iters: usize,
    /// 初始 "温度" (单步最大位移)。
    pub initial_temp: f64,
    /// 冷却率; T *= cool_rate per iter。
    pub cool_rate: f64,
}

impl Default for FDConfig {
    fn default() -> Self {
        Self {
            // 30x5 = 150 cells, 18 元件 → sqrt(150/18) ≈ 2.9
            k: 3.0,
            max_iters: 200,
            initial_temp: 2.0,
            cool_rate: 0.95,
        }
    }
}

// ============================================================
//  成本函数 (优化版: 使用预计算 context + reusable buffers)
// ============================================================

/// 评估当前状态的 cost (向后兼容接口)。
/// 内部构造临时 context/buffers, 推荐在热循环里直接用 `cost_fast`。
pub fn cost(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, super::breadboard::HoleId)],
    w: &Weights,
) -> f64 {
    let ctx = SAContext::new(circuit, &state.placeable);
    let mut buf = CostBuf::new(circuit.nets().len());
    cost_fast(state, circuit, board, bridged_pins, w, &ctx, &mut buf)
}

/// 快速版本: 复用预计算的 context 和 buffers。
/// 在 simulate() 的热循环里替代 `cost()`。
pub(crate) fn cost_fast(
    state: &SAState,
    circuit: &Circuit,
    board: &Breadboard,
    bridged_pins: &[(crate::circuit::PinId, super::breadboard::HoleId)],
    w: &Weights,
    ctx: &SAContext,
    buf: &mut CostBuf,
) -> f64 {
    buf.clear();

    let cols_i = board.cols() as i32;
    let n_comps = state.placeable.len();

    // 1. 收集所有 pin 的 (col, row, rail_id) 和所属 net, 以及每个元件的 bbox。
    for (idx, _comp_id) in state.placeable.iter().enumerate() {
        if state.bridged[idx] {
            let bridged_bbox = state.active_bridge_pair(idx).map(|pair| {
                let p0 = board.hole(pair[0].0).position;
                let p1 = board.hole(pair[1].0).position;
                BBox {
                    min_x: p0.x.min(p1.x),
                    max_x: p0.x.max(p1.x),
                    min_y: p0.y.min(p1.y),
                    max_y: p0.y.max(p1.y),
                }
            });
            buf.bboxes.push(bridged_bbox);
            continue;
        }

        let comp_info = &ctx.comp_infos[idx];
        let px = state.x[idx];
        let py = state.y[idx];
        let is_r180 = state.rotation[idx] == Rotation::R180;

        for pin_data in &comp_info.pins {
            let offset = if is_r180 { pin_data.1 } else { pin_data.0 };
            let x = px + offset.x;
            let y = py + offset.y;
            let rail_id = board
                .at(x, y)
                .map(|h| board.rail_id_of(h))
                .unwrap_or(u32::MAX);
            buf.holes.push((x, y, rail_id));
            buf.nets.push(pin_data.2);
            buf.is_virtual.push(false);
        }

        // BBox: translate precomputed R0 bbox
        let bbox_r0 = &comp_info.bbox_r0;
        let world_bbox = if is_r180 {
            BBox {
                min_x: -bbox_r0.max_x + px,
                max_x: -bbox_r0.min_x + px,
                min_y: -bbox_r0.max_y + py,
                max_y: -bbox_r0.min_y + py,
            }
        } else {
            BBox {
                min_x: bbox_r0.min_x + px,
                max_x: bbox_r0.max_x + px,
                min_y: bbox_r0.min_y + py,
                max_y: bbox_r0.max_y + py,
            }
        };
        buf.bboxes.push(Some(world_bbox));
    }

    // 1b. 注入用户预摆的 bridged 元件的 pin
    for &(pin_id, hole_id) in bridged_pins {
        let pin = &circuit.pins[pin_id.0];
        let pos = board.hole(hole_id).position;
        let rail_id = board.rail_id_of(hole_id);
        buf.holes.push((pos.x, pos.y, rail_id));
        buf.nets.push(pin.net);
        buf.is_virtual.push(false);
    }

    // 1b'. 注入 SA Toggle 后的 bridged 元件的 pin
    for idx in 0..n_comps {
        if !state.bridged[idx] {
            continue;
        }
        let pair = state
            .active_bridge_pair(idx)
            .expect("bridged=true 必有 pin pair (sa::simulate 保证 is_bridgeable[idx] = true)");
        for &(h, pin_id) in &pair {
            let pin = &circuit.pins[pin_id.0];
            let pos = board.hole(h).position;
            let rail_id = board.rail_id_of(h);
            buf.holes.push((pos.x, pos.y, rail_id));
            buf.nets.push(pin.net);
            buf.is_virtual.push(false);
        }
    }

    // 1c. 注入 power rail 虚拟 pin
    if let Some(binding) = board.power_rail_binding() {
        for (polarity, net_id) in [
            (Polarity::Negative, binding.negative),
            (Polarity::Positive, binding.positive),
        ] {
            if let Some(anchor) = board.power_rail_anchor(polarity) {
                let pos = board.hole(anchor).position;
                let rail_id = board.rail_id_of(anchor);
                buf.holes.push((pos.x, pos.y, rail_id));
                buf.nets.push(Some(net_id));
                buf.is_virtual.push(true);
            }
        }
    }

    // 2. OOB: rail_id == u32::MAX 即越界 / blocked row / 电源轨 gap
    let mut oob_count = 0u32;
    for &(_, _, rail_id) in &buf.holes {
        if rail_id == u32::MAX {
            oob_count += 1;
        }
    }

    // 3. Pin 碰撞: 每个被多个 pin 占用的孔, 算 N-1 次
    let mut coll_count = 0u32;
    for (i, &hole) in buf.holes.iter().enumerate() {
        if hole.2 == u32::MAX || buf.is_virtual[i] {
            continue;
        }
        if !buf.pin_seen.insert(hole) {
            coll_count += 1;
        }
    }

    // 4. bbox 碰撞
    let mut bbox_overlap_count = 0u32;
    for i in 0..buf.bboxes.len() {
        let Some(bi) = buf.bboxes[i] else { continue };
        for j in (i + 1)..buf.bboxes.len() {
            let Some(bj) = buf.bboxes[j] else { continue };
            if !bi.overlaps(&bj) {
                continue;
            }
            for pos in bi.iter_cells() {
                if pos.x >= bj.min_x && pos.x <= bj.max_x && pos.y >= bj.min_y && pos.y <= bj.max_y
                {
                    bbox_overlap_count += 1;
                }
            }
        }
    }

    // 5. MST 走线估算: 用 net_buckets (Vec<Vec<usize>>) 代替 HashMap
    for (i, &net_opt) in buf.nets.iter().enumerate() {
        let hole = buf.holes[i];
        if hole.2 == u32::MAX {
            continue;
        }
        if let Some(net) = net_opt {
            buf.net_buckets[net.0].push(i);
        }
    }
    let mut mst_sum = 0.0f64;
    for bucket in &buf.net_buckets {
        if bucket.len() < 2 {
            continue;
        }
        mst_sum += mst_wire_length_fast(bucket, &buf.holes);
    }

    // 6. 列冲突 (rail 冲突): 按 rail_id 聚合
    for (i, &net_opt) in buf.nets.iter().enumerate() {
        let (_, _, rail_id) = buf.holes[i];
        if rail_id == u32::MAX {
            continue;
        }
        buf.rail_map
            .entry(rail_id)
            .or_insert_with(|| Vec::with_capacity(4))
            .push(net_opt);
    }
    let mut col_conflict_pairs = 0usize;
    for rail_owners in buf.rail_map.values() {
        if rail_owners.len() < 2 {
            continue;
        }
        let base = rail_owners[0];
        for owner in &rail_owners[1..] {
            if *owner != base {
                col_conflict_pairs += 1;
            }
        }
    }

    // 7. 紧凑度: 按 rail 分组
    for bbox_opt in buf.bboxes.iter() {
        let Some(bbox) = bbox_opt else { continue };
        if bbox.min_x < 0
            || bbox.max_x >= cols_i
            || bbox.min_y < 0
            || bbox.min_y >= board.main_rows() as i32
            || board.is_blocked(bbox.min_y as usize)
        {
            continue;
        }
        let rail_top = board
            .rail_rows(bbox.min_y)
            .first()
            .copied()
            .unwrap_or(bbox.min_y);
        buf.compact_map
            .entry(rail_top)
            .or_insert_with(|| Vec::with_capacity(4))
            .push(*bbox);
    }
    let mut area_sum = 0.0f64;
    let mut row_squash_penalty = 0.0f64;
    for cells in buf.compact_map.values() {
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut seen_y: HashSet<i32> = HashSet::with_capacity(4);
        for b in cells {
            min_x = min_x.min(b.min_x);
            max_x = max_x.max(b.max_x);
            min_y = min_y.min(b.min_y);
            max_y = max_y.max(b.max_y);
            seen_y.insert(b.min_y);
        }
        if min_x <= max_x && min_y <= max_y {
            let width = (max_x - min_x + 1) as f64;
            let height = (max_y - min_y + 1) as f64;
            area_sum += width * height;
        }
        // 纵向挤压: 元件数 n vs 实际占用行数 ny
        let ny = seen_y.len();
        let n = cells.len();
        if n > ny {
            row_squash_penalty += (n - ny) as f64;
        }
    }
    let rail_cross = if buf.compact_map.len() >= 2 {
        w.rail_crossing
    } else {
        0.0
    };

    w.mst * mst_sum
        + w.pin_overlap * coll_count as f64
        + w.b_box_overlap * bbox_overlap_count as f64
        + w.column_conflict * col_conflict_pairs as f64
        + w.out_of_bounds * oob_count as f64
        + w.compactness * area_sum
        + w.row_squash * row_squash_penalty
        + rail_cross
}

/// 给一个 net 的 pin 位置算 MST (Kruskal) 总长度。
///
/// breadboard 物理距离 (短路抽象为 `rail_id`):
/// - 同 `rail_id`: **0** (面包板内部短接, 无论是 vertical rail 还是 power rail)
/// - 不同 `rail_id`: Manhattan |Δcol| + |Δrow|
///
/// 这是 wire 长度的下界 — 实际走线可能更长 (绕障碍), 但 SA 用它做优化目标。
#[cfg(test)]
fn mst_wire_length(pins: &[(i32, i32, u32)]) -> f64 {
    mst_wire_length_fast(&(0..pins.len()).collect::<Vec<_>>(), pins)
}

/// 快速版本: 使用 index 引用 buf.holes 而不是复制数据。
/// 对于 ≤3 pins 用直接公式, ≥4 用 Kruskal (本地向量, 不借用 buf)。
fn mst_wire_length_fast(indices: &[usize], holes: &[(i32, i32, u32)]) -> f64 {
    let n = indices.len();
    match n {
        0..=1 => 0.0,
        2 => {
            let a = holes[indices[0]];
            let b = holes[indices[1]];
            if a.2 == b.2 {
                0.0
            } else {
                ((a.0 - b.0).abs() + (a.1 - b.1).abs()) as f64
            }
        }
        3 => {
            // 3 pins: 3 种可能的 spanning tree (选 2 条边), 取 min
            let p0 = holes[indices[0]];
            let p1 = holes[indices[1]];
            let p2 = holes[indices[2]];
            let d01 = if p0.2 == p1.2 {
                0
            } else {
                (p0.0 - p1.0).abs() + (p0.1 - p1.1).abs()
            };
            let d02 = if p0.2 == p2.2 {
                0
            } else {
                (p0.0 - p2.0).abs() + (p0.1 - p2.1).abs()
            };
            let d12 = if p1.2 == p2.2 {
                0
            } else {
                (p1.0 - p2.0).abs() + (p1.1 - p2.1).abs()
            };
            let min_d = (d01 + d02).min(d01 + d12).min(d02 + d12);
            min_d as f64
        }
        _ => {
            // 4+ pins: Kruskal with local allocations
            mst_wire_length_fast_kruskal(indices, holes)
        }
    }
}

/// Kruskal MST for ≥4 pin nets. Allocates locally (nets with ≥4 pins are rare).
fn mst_wire_length_fast_kruskal(indices: &[usize], holes: &[(i32, i32, u32)]) -> f64 {
    let n = indices.len();

    // Generate edges
    let mut edges: Vec<(i32, usize, usize)> = Vec::with_capacity(n * (n - 1) / 2);
    for a in 0..n {
        for b in (a + 1)..n {
            let ha = holes[indices[a]];
            let hb = holes[indices[b]];
            let d = if ha.2 == hb.2 {
                0
            } else {
                (ha.0 - hb.0).abs() + (ha.1 - hb.1).abs()
            };
            edges.push((d, a, b));
        }
    }
    edges.sort_by_key(|e| e.0);

    // Union-find
    let mut parent: Vec<usize> = (0..n).collect();

    let find = |parent: &mut Vec<usize>, mut x: usize| -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    };

    let mut total: i32 = 0;
    let mut edges_used = 0;
    for &(d, i, j) in &edges {
        let ri = find(&mut parent, i);
        let rj = find(&mut parent, j);
        if ri != rj {
            parent[ri] = rj;
            total += d;
            edges_used += 1;
            if edges_used == n - 1 {
                break;
            }
        }
    }
    total as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::Breadboard;
    use crate::layout::breadboard::PowerRailBinding;

    /// 1 列宽, 1 个 pin, R0/R180 等价的 footprint (没有第二个 pin 区分方向)。
    fn one_pin_fp() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "single".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        }
    }

    /// 2 列宽, 2 个 pin 紧挨着 (典型 LED 封装)。
    fn two_pin_fp() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "two".into(),
            pins: (1..=2)
                .map(|n| PhysicalPin {
                    name: n.to_string(),
                    offset: Position { x: n - 1, y: 0 },
                })
                .collect(),
        }
    }

    /// 2-pin 元件 + footprint + 1 个 net (pin1 和 pin2 都在 net A)
    fn two_pin_in_net() -> (Circuit, ComponentId) {
        let footprint = two_pin_fp();
        let comp = Component {
            id: ComponentId(0),
            ref_: "D1".into(),
            kind: "LED".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pins = vec![
            Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: Some("K".into()),
                net: Some(NetId(0)),
            },
            Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: Some("A".into()),
                net: Some(NetId(0)),
            },
        ];
        let nets = vec![Net {
            id: NetId(0),
            name: "net-a".into(),
            pins: vec![PinId(0), PinId(1)],
        }];
        let circuit = Circuit {
            components: vec![comp],
            pins,
            nets,
            footprints: vec![footprint],
        };
        (circuit, ComponentId(0))
    }

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    /// 只关心 MST / pin / bbox / column 各项的测试用, 屏蔽新加的紧凑度和跨 rail 惩罚。
    /// 不想让"layout 跨几行" 之类的全局性质混入到孤立某项成本的断言里。
    /// 显式 mst=1.0 让 "1 cell MST → cost 1.0" 这种简单算术在测试里成立
    /// (默认 mst=5.0 是给 SA 跑的; 测试要看的不是权重而是公式结构)。
    fn weights_legacy() -> Weights {
        Weights {
            mst: 1.0,
            compactness: 0.0,
            rail_crossing: 0.0,
            row_squash: 0.0,
            ..Weights::default()
        }
    }

    #[test]
    fn empty_state_costs_zero() {
        let (circuit, _) = two_pin_in_net();
        let state = SAState::from_order(vec![], 2, &[]);
        let c = cost(&state, &circuit, &board(), &[], &Weights::default());
        assert_eq!(c, 0.0);
    }

    #[test]
    fn one_component_same_net_mst_is_one() {
        // 2 pin 紧挨着 (0, 2) 和 (1, 2), 都在同一 net → MST = |1-0| = 1
        let (circuit, cid) = two_pin_in_net();
        let state = SAState::from_order(vec![cid], 2, &[2]);
        let c = cost(&state, &circuit, &board(), &[], &weights_legacy());
        assert!((c - 1.0).abs() < 1e-9, "expected 1.0, got {}", c);
    }

    #[test]
    fn pin_collision_adds_penalty() {
        // 两个 1-pin footprint, 显式 x = [0, 0] 制造 pin 撞。
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[1, 1]);
        // 不撞: x = [0, 2]
        state.x = vec![0, 2];
        let c_clean = cost(&state, &circuit, &board(), &[], &weights_legacy());
        assert_eq!(c_clean, 0.0);

        // 撞: x = [0, 0]
        state.x = vec![0, 0];
        let c_coll = cost(&state, &circuit, &board(), &[], &weights_legacy());
        let expected = weights_legacy().pin_overlap + weights_legacy().b_box_overlap;
        assert!(
            (c_coll - expected).abs() < 1e-9,
            "expected pin_overlap + b_box_overlap = {}, got {}",
            expected,
            c_coll
        );
    }

    /// 列冲突: 同列不同 net 的 pin 对会多扣 cost
    #[test]
    fn column_conflict_adds_penalty() {
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(i)),
            })
            .collect();
        let nets = (0..2)
            .map(|i| Net {
                id: NetId(i),
                name: format!("n{i}"),
                pins: vec![PinId(i)],
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[1, 1]);

        // 不冲突: x = [0, 2]
        let mut s = state.clone();
        s.x = vec![0, 2];
        let c_clean = cost(&s, &circuit, &board(), &[], &weights_legacy());
        assert_eq!(c_clean, 0.0);

        // 冲突: x = [0, 0] (同列, 同孔 → pin_collision + bbox_collision + column_conflict)
        let mut s = state.clone();
        s.x = vec![0, 0];
        let c_coll = cost(&s, &circuit, &board(), &[], &weights_legacy());
        let expected = weights_legacy().pin_overlap
            + weights_legacy().b_box_overlap
            + weights_legacy().column_conflict;
        assert!(
            (c_coll - expected).abs() < 1e-9,
            "expected pin_overlap + b_box_overlap + column_conflict = {}, got {}",
            expected,
            c_coll
        );

        // 只 column_conflict: 同列不同行
        let mut s = state;
        s.x = vec![0, 0];
        s.y = vec![2, 3];
        let c_col_only = cost(&s, &circuit, &board(), &[], &weights_legacy());
        assert!(
            (c_col_only - weights_legacy().column_conflict).abs() < 1e-9,
            "expected only column_conflict penalty, got {}",
            c_col_only
        );
    }

    /// 标准板上, 同列不同 rail 的不同 net pin 不该被记为列冲突。
    #[test]
    fn column_conflict_ignores_different_rails_in_cost() {
        let board = crate::layout::Breadboard::standard();
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(i)),
            })
            .collect();
        let nets = (0..2)
            .map(|i| Net {
                id: NetId(i),
                name: format!("n{i}"),
                pins: vec![PinId(i)],
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 0, &[1, 1]);
        // 同 col 0, C0 在上 rail (y=2), C1 在下 rail (y=10) — 物理不连通
        state.x = vec![0, 0];
        state.y = vec![2, 10];
        let c = cost(&state, &circuit, &board, &[], &weights_legacy());
        assert_eq!(
            c, 0.0,
            "上下 rail 同列不同 net 不该被 cost 记为冲突, got {c}"
        );
    }

    #[test]
    fn oob_adds_huge_penalty() {
        let fp = one_pin_fp();
        let comp = Component {
            id: ComponentId(0),
            ref_: "X1".into(),
            kind: "X".into(),
            value: None,
            pins: vec![PinId(0)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pins = vec![Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: None,
        }];
        let circuit = Circuit {
            components: vec![comp],
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        state.y[0] = -5;
        let c = cost(&state, &circuit, &board(), &[], &Weights::default());
        assert!(c >= Weights::default().out_of_bounds);
    }

    #[test]
    fn from_greedy_fits_2d() {
        // 5 个 2-pin footprint, 贪心应该能放下 (5*3 = 15 cols, 5 rows = 150 cells)
        let fp = two_pin_fp();
        let comps = (0..5)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..10)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..5).map(ComponentId).collect();
        let state = SAState::from_greedy(placeable, &circuit, &board());
        assert_eq!(state.n(), 5);
        // 所有 y 都在 [0, 4]
        for &y in &state.y {
            assert!((0..5).contains(&y), "y={} not in board", y);
        }
        // 所有 x + 1 (footprint 宽 2) < 30
        for &x in &state.x {
            assert!(x + 1 < 30, "x={} 越界", x);
        }
    }

    #[test]
    fn from_greedy_spills_to_next_row() {
        // 4 个 11-col footprint (实际只 1 pin 在用), 30 col 板 → 4*11=44 > 30, 第 4 个应
        // 溢出到 row 1
        let fp = Footprint {
            id: FootprintId(0),
            name: "wide".into(),
            pins: (0..11)
                .map(|i| PhysicalPin {
                    name: i.to_string(),
                    offset: Position { x: i, y: 0 },
                })
                .collect(),
        };
        let comps = (0..4)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("W{i}"),
                kind: "W".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..4)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "0".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..4).map(ComponentId).collect();
        let state = SAState::from_greedy(placeable, &circuit, &board());
        // 3 个 11-col 放 row 0 占 0..33 (实际放 0, 1, 12, 3 个 footprint 总跨度)
        // 第 4 个放不下 row 0 → 走 row 1
        assert_eq!(
            state.y[3], 1,
            "第 4 个应去 row 1, 实际在 row {}",
            state.y[3]
        );
    }

    /// FD 基本性质: 全连通的 3 个元件, 应该聚在一起 (距离 ≤ 3)
    #[test]
    fn from_force_directed_clusters_connected() {
        let fp = one_pin_fp();
        // 3 个元件全连同一个网
        let comps = (0..3)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..3)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(0)),
            })
            .collect();
        let nets = vec![Net {
            id: NetId(0),
            name: "all".into(),
            pins: (0..3).map(PinId).collect(),
        }];
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..3).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());
        // 3 个连通的 1-pin 元件, FD 应该把它们聚在一起 (col 间距 ≤ 3)
        let xs: Vec<i32> = state.x.to_vec();
        let x_min = *xs.iter().min().unwrap();
        let x_max = *xs.iter().max().unwrap();
        assert!(
            x_max - x_min <= 3,
            "3 个全连通的元件应聚簇, 实际 x 范围: {x_min}..{x_max}"
        );
    }

    /// FD 对无连接的元件, 应该把它们推开 (距离 ≥ 2*k)
    #[test]
    fn from_force_directed_spreads_unconnected() {
        let fp = one_pin_fp();
        // 3 个元件, 都不连
        let comps = (0..3)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..3)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None, // 无 net
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..3).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());
        // 3 个 1-pin 元件无连接, 应散开 (最远 col 间距 ≥ 2)
        let xs: Vec<i32> = state.x.to_vec();
        let x_min = *xs.iter().min().unwrap();
        let x_max = *xs.iter().max().unwrap();
        assert!(
            x_max - x_min >= 2,
            "3 个无连接元件应散开, 实际 x 范围: {x_min}..{x_max}"
        );
    }

    /// FD 输出所有元件在板内、无 pin 撞
    #[test]
    fn from_force_directed_no_oob_or_collision() {
        let fp = two_pin_fp();
        let comps = (0..5)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..10)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                net: if i % 2 == 0 {
                    Some(NetId(0))
                } else {
                    Some(NetId(1))
                },
            })
            .collect();
        let nets = vec![
            Net {
                id: NetId(0),
                name: "a".into(),
                pins: (0..5).map(|i| PinId(i * 2)).collect(),
            },
            Net {
                id: NetId(1),
                name: "b".into(),
                pins: (1..5).map(|i| PinId(i * 2 + 1)).collect(),
            },
        ];
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..5).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());
        // 所有 pin 在板内
        for idx in 0..5 {
            let footprint = &circuit.footprints[0];
            for p in footprint.pins() {
                let abs_x = state.x[idx] + p.offset.x;
                let abs_y = state.y[idx] + p.offset.y;
                assert!(
                    (0..30).contains(&abs_x),
                    "x OOB: {} from {}",
                    abs_x,
                    state.x[idx]
                );
                assert!(
                    (0..5).contains(&abs_y),
                    "y OOB: {} from {}",
                    abs_y,
                    state.y[idx]
                );
            }
        }
        // pin 不撞
        let mut holes: HashSet<(i32, i32)> = HashSet::new();
        for idx in 0..5 {
            let footprint = &circuit.footprints[0];
            for p in footprint.pins() {
                let abs = (state.x[idx] + p.offset.x, state.y[idx] + p.offset.y);
                assert!(holes.insert(abs), "pin 撞了: {:?}", abs);
            }
        }
    }

    /// FD 输出同列同 rail 不应存在不同 net 的 pin (避免造成 1M cost 跌入难跳出)。
    /// 5 个 2-pin 元件,  pin 1 连 net A,  pin 2 连 net B。
    fn two_pin_two_net_fp() -> Footprint {
        Footprint {
            id: FootprintId(0),
            name: "two".into(),
            pins: (1..=2)
                .map(|n| PhysicalPin {
                    name: n.to_string(),
                    offset: Position { x: n - 1, y: 0 },
                })
                .collect(),
        }
    }

    #[test]
    fn from_force_directed_no_column_conflict() {
        let fp = two_pin_two_net_fp();
        // 5 个 2-pin 元件, 偶数下标的 pin 连 net 0, 奇数连 net 1
        let comps: Vec<Component> = (0..5)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins: Vec<Pin> = (0..10)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                net: if i % 2 == 0 {
                    Some(NetId(0))
                } else {
                    Some(NetId(1))
                },
            })
            .collect();
        let nets = vec![
            Net {
                id: NetId(0),
                name: "a".into(),
                pins: (0..5).map(|i| PinId(i * 2)).collect(),
            },
            Net {
                id: NetId(1),
                name: "b".into(),
                pins: (0..5).map(|i| PinId(i * 2 + 1)).collect(),
            },
        ];
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let placeable: Vec<ComponentId> = (0..5).map(ComponentId).collect();
        let state =
            SAState::from_force_directed(placeable, &circuit, &board(), &FDConfig::default());

        // 重新算列冲突, 期望为 0
        let mut by_col: HashMap<(i32, i32), Vec<Option<NetId>>> = HashMap::new();
        for idx in 0..5 {
            let footprint = &circuit.footprints[0];
            for pin_id in 0..2 {
                let p = &circuit.pins[idx * 2 + pin_id];
                let physical = footprint
                    .pins()
                    .iter()
                    .find(|pp| pp.name == ((pin_id + 1).to_string()))
                    .unwrap();
                let abs_x = state.x[idx] + physical.offset.x;
                let abs_y = state.y[idx] + physical.offset.y;
                if !(0..30).contains(&abs_x) || !(0..5).contains(&abs_y) {
                    continue;
                }
                let rail_top = board().rail_rows(abs_y).first().copied().unwrap_or(abs_y);
                by_col.entry((abs_x, rail_top)).or_default().push(p.net);
            }
        }
        let mut col_conflict_pairs = 0;
        for col_owners in by_col.values() {
            if col_owners.len() < 2 {
                continue;
            }
            let base = col_owners[0];
            for owner in &col_owners[1..] {
                if *owner != base {
                    col_conflict_pairs += 1;
                }
            }
        }
        assert_eq!(
            col_conflict_pairs, 0,
            "FD 输出含 {col_conflict_pairs} 个列冲突, by_col = {by_col:?}"
        );
    }

    // ============================================================
    //  MST cost 测试
    // ============================================================

    /// MST 边距: 同 rail_id = 0 (不管是 vertical 还是 power rail)
    #[test]
    fn mst_same_col_same_rail_is_zero() {
        let b = Breadboard::new(30, 5);
        let len = mst_wire_length(&[pin(&b, 0, 0), pin(&b, 0, 2)]);
        assert_eq!(len, 0.0, "同列同 rail 应该 rail 短接, MST = 0");
    }

    /// MST 边距: 同 rail 不同 col = |Δcol| (jumper)
    #[test]
    fn mst_same_rail_different_col_is_abs_col_delta() {
        let b = Breadboard::new(30, 5);
        let len = mst_wire_length(&[pin(&b, 0, 2), pin(&b, 3, 2)]);
        assert_eq!(len, 3.0, "同 rail 不同 col = |Δcol| = 3");
    }

    /// MST 边距: 不同 rail (跨中央通道) = Manhattan
    #[test]
    fn mst_same_col_different_rail_is_abs_row_delta() {
        let b = Breadboard::standard(); // rows 5, 6 blocked
        let len = mst_wire_length(&[pin(&b, 5, 0), pin(&b, 5, 8)]);
        assert_eq!(len, 8.0, "同 col 跨 rail = |Δrow| = 8");
    }

    /// MST 边距: 不同 col 不同 rail = Manhattan
    #[test]
    fn mst_different_col_different_rail_is_manhattan() {
        // row 2 blocked → (0, 0) 在 rail 0 (row 0..1), (3, 4) 在 rail 1 (row 3..11)
        let b = Breadboard::with_blocked_rows(30, 12, [2]);
        let len = mst_wire_length(&[pin(&b, 0, 0), pin(&b, 3, 4)]);
        assert_eq!(len, 7.0, "不同 col 不同 rail = 3 + 4 = 7");
    }

    /// MST: 3 pin 的 net, 三角形走最短 (2 条边)
    #[test]
    fn mst_three_pins_picks_two_shortest_edges() {
        let b = Breadboard::new(30, 5);
        // 3 pin: (0,0), (1,0), (5,0) — 都在同一 rail
        // 边: 0-1 (1), 0-5 (5), 1-5 (4) → MST 取 0-1 + 1-5 = 5
        let len = mst_wire_length(&[pin(&b, 0, 0), pin(&b, 1, 0), pin(&b, 5, 0)]);
        assert_eq!(len, 5.0);
    }

    /// helper: 从 board 反查 (x, y) 对应的 (x, y, rail_id) 测试输入
    fn pin(b: &Breadboard, x: i32, y: i32) -> (i32, i32, u32) {
        let id = b
            .at(x, y)
            .unwrap_or_else(|| panic!("测试 pin ({x},{y}) 不在板上"));
        (x, y, b.rail_id_of(id))
    }

    // ============================================================
    //  Power rail 短路 测试
    // ============================================================

    /// 同 power rail 行内 (不同 col) → MST = 0 (横向短接)
    #[test]
    fn mst_same_power_rail_row_is_zero() {
        let b = Breadboard::standard();
        // top negative y=-4, col 0 和 col 10
        let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 10, -4)]);
        assert_eq!(len, 0.0, "同 power rail 行内应该 shorted, MST = 0");
    }

    /// 同极性 top + bottom (用户约定: 短接 + 同一 net) → MST = 0
    #[test]
    fn mst_top_and_bottom_same_polarity_is_zero() {
        let b = Breadboard::standard();
        let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 0, 14)]);
        assert_eq!(len, 0.0, "上下两条同极性应该 shorted, MST = 0");
    }

    /// 正负极 → MST = Manhattan (不短接)
    #[test]
    fn mst_positive_and_negative_is_manhattan() {
        let b = Breadboard::standard();
        // (0, -4) negative, (6, -3) positive → |6| + |1| = 7
        // (6 是 group 第二个的开始: cols 6..10)
        let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 6, -3)]);
        assert_eq!(len, 7.0, "正负极不短接, MST = Manhattan");
    }

    /// Power rail 跟 main board → MST = Manhattan (rail_id 不同)
    #[test]
    fn mst_power_rail_to_main_is_manhattan() {
        let b = Breadboard::standard();
        // top negative (0, -4) 跟 main upper (0, 0): |0| + |4| = 4
        let len = mst_wire_length(&[pin(&b, 0, -4), pin(&b, 0, 0)]);
        assert_eq!(len, 4.0);
    }

    // ============================================================
    //  PowerRailBinding 虚拟 pin
    // ============================================================

    /// 绑定 GND 到负极: 1 个 GND pin 在 (10, 0), 加上虚拟 pin 在 (0, -4)。
    /// MST 距离 = |10| + |4| = 14 (主区到 rail 的 jumper 长度)。
    /// 不绑定时, 那个 pin 单独一个节点, MST = 0。
    /// 用 delta 检验: cost(绑定) - cost(不绑定) 应该 = 14 * mst_weight。
    #[test]
    fn cost_with_binding_reflects_rail_jumper() {
        use crate::circuit::{ComponentId, FootprintId, NetId, PinId};
        use crate::layout::cost::{SAState, Weights};

        let footprint = crate::circuit::Footprint {
            id: FootprintId(0),
            name: "1p".into(),
            pins: vec![crate::circuit::PhysicalPin {
                name: "1".into(),
                offset: crate::circuit::Position { x: 0, y: 0 },
            }],
        };
        let component = crate::circuit::Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pin = crate::circuit::Pin {
            id: PinId(0),
            component: ComponentId(0),
            num: "1".into(),
            pinfunction: None,
            net: Some(NetId(0)),
        };
        let net = crate::circuit::Net {
            id: NetId(0),
            name: "GND".into(),
            pins: vec![PinId(0)],
        };
        let circuit = crate::circuit::Circuit {
            components: vec![component],
            pins: vec![pin],
            nets: vec![net],
            footprints: vec![footprint],
        };
        let mut state = SAState::from_order(vec![ComponentId(0)], 1, &[1]);
        state.x[0] = 10;
        state.y[0] = 0;
        let w = Weights::default();

        // 不绑定
        let board_no_bind = Breadboard::standard();
        let cost_no = cost(&state, &circuit, &board_no_bind, &[], &w);

        // 绑定: 虚拟 pin (0, -4) 加入 net, MST = |10| + |4| = 14
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(0),
        });
        let cost_with = cost(&state, &circuit, &board, &[], &w);

        let delta = cost_with - cost_no;
        let expected_delta = 14.0 * w.mst; // 纯 MST 增量
        assert!(
            (delta - expected_delta).abs() < 0.01,
            "绑定后 cost 增量 = MST 14, 实际 delta = {delta}, 期望 = {expected_delta}"
        );
    }

    /// 不绑定时, 成本跟以前完全一样 (虚拟 pin 0 个)。
    /// 上面那个测试的不绑定部分已覆盖, 这里再加个明显不动的检查: 0 元件 0 pin。
    #[test]
    fn cost_no_binding_no_rail_pins() {
        use crate::circuit::{ComponentId, FootprintId, NetId, PinId};

        // 2-pin 元件, 2 个 pin 都在同一 rail, 同 net → MST = 0
        // 不绑定: 0 虚拟 pin, 跟以前一样
        let footprint = crate::circuit::Footprint {
            id: FootprintId(0),
            name: "2p".into(),
            pins: vec![
                crate::circuit::PhysicalPin {
                    name: "1".into(),
                    offset: crate::circuit::Position { x: 0, y: 0 },
                },
                crate::circuit::PhysicalPin {
                    name: "2".into(),
                    offset: crate::circuit::Position { x: 1, y: 0 },
                },
            ],
        };
        let component = crate::circuit::Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let pins = vec![
            crate::circuit::Pin {
                id: PinId(0),
                component: ComponentId(0),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(0)),
            },
            crate::circuit::Pin {
                id: PinId(1),
                component: ComponentId(0),
                num: "2".into(),
                pinfunction: None,
                net: Some(NetId(0)),
            },
        ];
        let net = crate::circuit::Net {
            id: NetId(0),
            name: "n".into(),
            pins: vec![PinId(0), PinId(1)],
        };
        let circuit = crate::circuit::Circuit {
            components: vec![component],
            pins,
            nets: vec![net],
            footprints: vec![footprint],
        };
        let state = crate::layout::cost::SAState::from_order(vec![ComponentId(0)], 1, &[2]);
        let board = Breadboard::standard();
        let c = cost(
            &state,
            &circuit,
            &board,
            &[],
            &crate::layout::cost::Weights::default(),
        );
        // cost = MST 1 (同 rail 不同 col, |Δcol|=1) × 5.0 (默认 mst 权重)
        //     + compactness 1.0 (2×1×0.5) = 6.0
        // 验证不绑定时, 没注入虚拟 pin 进去 (否则 cost 会更高)
        assert_eq!(
            c, 6.0,
            "不绑定, 同 rail 同 net, cost = MST 1 × mst 5.0 + compactness 1.0 = 6.0"
        );
    }

    /// 成本函数走 MST 而非 HPWL:
    /// 同列不同 row (同 rail) → cost = 0 (零跳线); 而 2D HPWL 会算 = Δrow
    #[test]
    fn cost_zero_jumper_layout_costs_zero() {
        // 2 个 1-pin 元件, 都在 col 0, 不同 row, 同 net
        // → MST 距离 = 0 (rail 短接)
        let fp = one_pin_fp();
        let comps: Vec<Component> = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins: Vec<Pin> = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: Some(NetId(0)),
            })
            .collect();
        let nets = vec![Net {
            id: NetId(0),
            name: "n".into(),
            pins: vec![PinId(0), PinId(1)],
        }];
        let circuit = Circuit {
            components: comps,
            pins,
            nets,
            footprints: vec![fp],
        };
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 0],
            y: vec![0, 1],
            rotation: vec![Rotation::R0, Rotation::R0],
            ..SAState::no_bridging(2)
        };
        let c = cost(&state, &circuit, &board(), &[], &weights_legacy());
        assert!(c.abs() < 1e-9, "零跳线布局应该 cost = 0, got {}", c);
    }

    // ============================================================
    //  紧凑度 + 跨 rail 惩罚
    // ============================================================

    /// 同样 2 个 1-pin 元件, 都同 rail 单行: cost 应随水平跨度线性增长, 垂直 y 不变不增加
    /// (x 和 y 等同计入, 但仅以 1 个 dimension 变化时只有那一项 +1)。
    #[test]
    fn compactness_penalizes_horizontal_spread() {
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        // 屏蔽 MST / pin / bbox / column / row_squash, 只看 compactness
        let w = Weights {
            mst: 0.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            row_squash: 0.0,
            ..Weights::default()
        };
        // 都同 row 2, x 贴在一起 (但不同 col, 不撞 pin)
        let s_tight = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 1],
            y: vec![2, 2],
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };
        let c_tight = cost(&s_tight, &circuit, &board(), &[], &w);
        // 同 row 2, x 拉开 (0, 5) → bbox 6 × 1 = 6 → cost 3.0
        let s_wide = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 5],
            y: vec![2, 2],
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };
        let c_wide = cost(&s_wide, &circuit, &board(), &[], &w);
        // 贴一起: bbox 2×1 = 2 → 1.0
        // 拉开 5 列: bbox 6×1 = 6 → 3.0
        assert!(
            (c_tight - 1.0).abs() < 1e-9,
            "贴一起 (x 0..1) 应 cost = 0.5 * 2 = 1.0, got {c_tight}"
        );
        assert!(
            (c_wide - 3.0).abs() < 1e-9,
            "拉开 5 列 (x 0..5) 应 cost = 0.5 * 6 = 3.0, got {c_wide}"
        );
        assert!(c_wide > c_tight);
    }

    /// 同样 2 个 1-pin 元件, 同列: cost 随垂直跨度增长, 跟水平等价 (x / y 平等)。
    #[test]
    fn compactness_treats_xy_equally() {
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let w = Weights {
            mst: 0.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            row_squash: 0.0,
            ..Weights::default()
        };

        // 拉开 5 cells (x 0..4, width 5) → cost = 0.5 * 5 * 1 = 2.5 (同 row)
        let s_horiz = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 4],
            y: vec![2, 2],
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };
        let c_horiz = cost(&s_horiz, &circuit, &board(), &[], &w);

        // 拉开 5 cells (y 0..4, height 5) → cost = 0.5 * 1 * 5 = 2.5 (同 col)
        let s_vert = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 0],
            y: vec![0, 4],
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };
        let c_vert = cost(&s_vert, &circuit, &board(), &[], &w);

        assert!(
            (c_horiz - c_vert).abs() < 1e-9,
            "x / y 应同代价: 水平={c_horiz}, 垂直={c_vert}"
        );
    }

    /// 跨 rail (中央通道上下都放) 应该加一个 rail_crossing 固定项。
    #[test]
    fn compactness_rail_crossing_penalty() {
        let board = crate::layout::Breadboard::standard();
        let fp = one_pin_fp();
        let comps = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..2)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let w = Weights {
            mst: 0.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            ..Weights::default()
        };

        // 同 rail: 无 rail_crossing
        let s_same = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 0],
            y: vec![0, 1], // 都是上 rail
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };
        let c_same = cost(&s_same, &circuit, &board, &[], &w);

        // 跨 rail (中央通道两侧): 加 rail_crossing
        let s_cross = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 0],
            y: vec![0, 10], // 上 + 下
            rotation: vec![Rotation::R0; 2],
            ..SAState::no_bridging(2)
        };
        let c_cross = cost(&s_cross, &circuit, &board, &[], &w);

        let delta = c_cross - c_same;
        assert!(
            (delta - w.rail_crossing).abs() < 1e-9,
            "跨 rail 多出的 cost 应 = rail_crossing ({}) , 实际多 {delta}",
            w.rail_crossing
        );
    }

    /// 按 rail 分组: 跨中央通道不应被算成"垂直跨度 7 行"让 area 虚胖。
    /// 也就是说, 上 rail 内 bbox 和下 rail 内 bbox 各自算, 不拼接。
    #[test]
    fn compactness_rail_split_avoids_central_channel_inflation() {
        let board = crate::layout::Breadboard::standard();
        let fp = one_pin_fp();
        // 3 个 1-pin 元件: 2 个上 rail (y=0, 1), 1 个下 rail (y=10)
        let comps = (0..3)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("X{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            })
            .collect();
        let pins = (0..3)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i),
                num: "1".into(),
                pinfunction: None,
                net: None,
            })
            .collect();
        let circuit = Circuit {
            components: comps,
            pins,
            nets: vec![],
            footprints: vec![fp],
        };
        let w = Weights {
            mst: 0.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            ..Weights::default()
        };

        // 同样 3 个元件, 都堆在上 rail, x 拉开避免 pin 撞
        let s_all_upper = SAState {
            placeable: vec![ComponentId(0), ComponentId(1), ComponentId(2)],
            x: vec![0, 1, 2],
            y: vec![0, 1, 2],
            rotation: vec![Rotation::R0; 3],
            ..SAState::no_bridging(3)
        };
        let c_all_upper = cost(&s_all_upper, &circuit, &board, &[], &w);

        // 1 个下 rail (y=10), 2 个上 rail (y=0, 1)
        let s_split = SAState {
            placeable: vec![ComponentId(0), ComponentId(1), ComponentId(2)],
            x: vec![0, 1, 2],
            y: vec![0, 1, 10],
            rotation: vec![Rotation::R0; 3],
            ..SAState::no_bridging(3)
        };
        let c_split = cost(&s_split, &circuit, &board, &[], &w);

        // 都上 rail: x=0..2, y=0..2, bbox = 3×3 = 9 → cost 4.5
        assert!(
            (c_all_upper - 4.5).abs() < 1e-9,
            "全上 rail 应 cost = 0.5 * 9 = 4.5, got {c_all_upper}"
        );
        // split: 上 rail bbox 0..1 × 0..1 = 2×2 = 4; 下 rail bbox 2..2 × 10..10 = 1×1 = 1;
        //        总 area = 5, cost = 2.5; 加 rail_crossing 5 = 7.5
        assert!(
            (c_split - (0.5 * (2.0 * 2.0 + 1.0 * 1.0) + w.rail_crossing)).abs() < 1e-9,
            "split 布局应 cost = 0.5 * 5 + 5.0 = 7.5, got {c_split}"
        );
    }

    // ============================================================
    //  桥接路径
    // ============================================================

    /// 1 个 2-pin 水平电阻 (pin offset Δ=(3,0)) + power net 绑定到 top positive rail。
    /// 启发式应该: power pin 落 top positive rail 第一个 hole, signal pin 经 R90
    /// 旋转后落 (col, row 0) main rail。返回 Some 且结构合法。
    #[test]
    fn propose_bridged_pair_uses_r90_for_horizontal_resistor() {
        use crate::circuit::{Footprint, PhysicalPin};
        use crate::layout::breadboard::PowerRailBinding;

        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 3, y: 0 },
                },
            ],
        };
        // pin 0 走 +12V (power), pin 1 走 SIG (signal)
        let circuit = crate::circuit::Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "R1".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0), PinId(1)],
                footprint: Some(FootprintId(0)),
                bridgeable: true,
            }],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
            ],
            nets: vec![
                crate::circuit::Net {
                    id: NetId(0),
                    name: "+12V".into(),
                    pins: vec![PinId(0)],
                },
                crate::circuit::Net {
                    id: NetId(1),
                    name: "SIG".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        };
        // 绑定: top/bottom positive rail  ← +12V, top/bottom negative rail ← SIG
        // (信号名号无所谓, 只曹 power_net_ids 包含 NetId(0) 即可)
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(1),
        });
        let comp = &circuit.components[0];
        let pair = propose_bridged_pair(comp, &circuit, &board, &[NetId(0), NetId(1)]);
        let pair = pair.expect("启发式应该能找一对合法桥接");
        let (h_power, pin_power) = pair[0];
        let (h_signal, pin_signal) = pair[1];
        // power 必须是 power rail
        assert_eq!(
            board.region_of(h_power),
            Region::PowerRail,
            "power 腿应落 power rail"
        );
        // signal 必须是 main rail
        assert_eq!(
            board.region_of(h_signal),
            Region::MainRail,
            "signal 腿应落 main rail"
        );
        // pin 标识要反映 power / signal 分工
        assert_eq!(pin_power, PinId(0), "pin 0 (net=+12V) 应是 power");
        assert_eq!(pin_signal, PinId(1), "pin 1 (net=SIG) 应是 signal");
        // body 方向: 两孔 x 差 == 0 (R90 后), y 差 == 3。证实是 R90 不是 R0 / R180。
        let p_p = board.hole(h_power).position;
        let p_s = board.hole(h_signal).position;
        assert_eq!(
            p_s.x - p_p.x,
            0,
            "R90 后 Δx 应 = 0 (body 竖直), got Δx = {}",
            p_s.x - p_p.x
        );
        assert_eq!(
            p_s.y - p_p.y,
            3,
            "R90 后 Δy 应 = 3 (footprint 跨度), got Δy = {}",
            p_s.y - p_p.y
        );
    }

    /// 没绑 power rail → power_net_ids 为空 → 启发式返 None。
    #[test]
    fn propose_bridged_pair_returns_none_without_power_rail_binding() {
        use crate::circuit::{Footprint, PhysicalPin};
        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 3, y: 0 },
                },
            ],
        };
        let circuit = crate::circuit::Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "R1".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0), PinId(1)],
                footprint: Some(FootprintId(0)),
                bridgeable: true,
            }],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
            ],
            nets: vec![
                crate::circuit::Net {
                    id: NetId(0),
                    name: "P".into(),
                    pins: vec![PinId(0)],
                },
                crate::circuit::Net {
                    id: NetId(1),
                    name: "S".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        };
        let board = Breadboard::standard(); // 不绑 power rail
        let comp = &circuit.components[0];
        // power_net_ids 为空 → power 腿找不到匹配 → 返 None
        let pair = propose_bridged_pair(comp, &circuit, &board, &[]);
        assert!(pair.is_none(), "无 power rail 时启发式应返 None");
    }

    /// bridged 状态的 2-pin 元件: 算 cost 时 pin 走 bridged_pin_pair, 不走 (x, y, rotation),
    /// 不计 bbox, 不计 OOB (因为启发式保证两个孔都是合法的)。
    /// 对比: 同一 bridgeable 元件, OnBoard 走 OOB 区域 vs Bridged 走启发式合法位,
    /// 后者的 cost 远低于前者 (无 OOB 巨罚, 也无越界让 cost 龲起)。
    #[test]
    fn cost_bridged_uses_heuristic_pair_and_skips_bbox() {
        use crate::circuit::{Footprint, PhysicalPin};
        use crate::layout::breadboard::PowerRailBinding;

        let fp = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 3, y: 0 },
                },
            ],
        };
        let circuit = crate::circuit::Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "R1".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0), PinId(1)],
                footprint: Some(FootprintId(0)),
                bridgeable: true,
            }],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
            ],
            nets: vec![
                crate::circuit::Net {
                    id: NetId(0),
                    name: "P".into(),
                    pins: vec![PinId(0)],
                },
                crate::circuit::Net {
                    id: NetId(1),
                    name: "S".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        };
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(1),
        });

        // OnBoard 状态: (0, 0) 放, 令 pin 1 跨进 gap (y=-1) → OOB 龲高 cost
        let on_board = SAState {
            placeable: vec![ComponentId(0)],
            x: vec![0],
            y: vec![0],
            rotation: vec![Rotation::R0],
            ..SAState::no_bridging(1)
        };
        let w = Weights::default();
        let c_on = cost(&on_board, &circuit, &board, &[], &w);

        // Bridged 状态: 走启发式合法对
        let pair = propose_bridged_pair(
            &circuit.components[0],
            &circuit,
            &board,
            &[NetId(0), NetId(1)],
        )
        .unwrap();
        let mut bridged = on_board.clone();
        bridged.is_bridgeable = vec![true];
        bridged.bridged = vec![true];
        bridged.bridged_pin_pairs = vec![vec![pair]];
        bridged.active_bridge_idx = vec![0];
        let c_bridge = cost(&bridged, &circuit, &board, &[], &w);

        // Bridged cost 应远小于 OnBoard (启发式选中两孔都在板上 + 有 rail, MST = 0)
        assert!(
            c_bridge < c_on,
            "Bridged 走启发式合法位应比 OnBoard 跨 gap OOB 便宜: on={c_on} bridge={c_bridge}"
        );
    }

    /// 验证 bridged 元件的 body bbox 参与碰撞检查:
    /// 一个 2-pin 电阻的启发式把 power 落在 top positive rail (col=0, y=-3),
    /// signal 落在 main rail (col=0, y=0) (R90 旋转)。body 走 col 0 rows -3..0。
    /// 另外一个 1-pin 元件放在 col 0, row 0 (在 bridged body 上), 成本应包含 bbox 碰撞。
    /// 同一个 1-pin 元件放在 col 5, row 0 (避开 body), 成本应不含 bbox 碰撞。
    #[test]
    fn cost_bridged_body_bbox_blocks_on_board_components() {
        use crate::circuit::{Footprint, PhysicalPin};
        use crate::layout::breadboard::PowerRailBinding;

        // 1 个 2-pin 水平电阻 (Δ=4)
        let fp_r = Footprint {
            id: FootprintId(0),
            name: "R".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 4, y: 0 },
                },
            ],
        };
        // 1 个 1-pin 元件 (后面 跟 resistor 独立)
        let fp_x = Footprint {
            id: FootprintId(1),
            name: "X".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        };
        let circuit = crate::circuit::Circuit {
            components: vec![
                Component {
                    id: ComponentId(0), // R, bridgeable
                    ref_: "R1".into(),
                    kind: "R".into(),
                    value: None,
                    pins: vec![PinId(0), PinId(1)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: true,
                },
                Component {
                    id: ComponentId(1), // X, 非 bridgeable
                    ref_: "X1".into(),
                    kind: "X".into(),
                    value: None,
                    pins: vec![PinId(2)],
                    footprint: Some(FootprintId(1)),
                    bridgeable: false,
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
                Pin {
                    id: PinId(2),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(1)),
                },
            ],
            nets: vec![
                crate::circuit::Net {
                    id: NetId(0),
                    name: "P".into(),
                    pins: vec![PinId(0)],
                },
                crate::circuit::Net {
                    id: NetId(1),
                    name: "S".into(),
                    pins: vec![PinId(1), PinId(2)],
                },
            ],
            footprints: vec![fp_r, fp_x],
        };
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(1),
        });

        // 拿启发式生成的 R1 bridged pair
        let pair = propose_bridged_pair(
            &circuit.components[0],
            &circuit,
            &board,
            &[NetId(0), NetId(1)],
        )
        .expect("启发式应返 pair");

        // X1 放在 (0, 0) — 与 bridged body 的 col 0 重叠
        let state_overlap = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            is_bridgeable: vec![true, false],
            bridged: vec![true, false],
            bridged_pin_pairs: vec![vec![pair], Vec::new()],
            active_bridge_idx: vec![0, 0],
            x: vec![0, 0],
            y: vec![0, 0],
            rotation: vec![Rotation::R0, Rotation::R0],
        };

        // X1 放在 (5, 0) — 避开 bridged body
        let state_clear = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            is_bridgeable: vec![true, false],
            bridged: vec![true, false],
            bridged_pin_pairs: vec![vec![pair], Vec::new()],
            active_bridge_idx: vec![0, 0],
            x: vec![0, 5],
            y: vec![0, 0],
            rotation: vec![Rotation::R0, Rotation::R0],
        };

        let w = Weights::default();
        let c_overlap = cost(&state_overlap, &circuit, &board, &[], &w);
        let c_clear = cost(&state_clear, &circuit, &board, &[], &w);

        // 重叠的 cost 应比不重叠的高, 高出部分 ≈ bbox 碰撞 (100 per cell)
        let delta = c_overlap - c_clear;
        assert!(
            delta > 1.0,
            "X1 摆 bridged body 上应比避开贵, 但 delta = {delta} (c_overlap={c_overlap}, c_clear={c_clear})"
        );
    }
}
