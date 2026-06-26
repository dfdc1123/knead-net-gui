//! 模拟退火用的成本函数: MST 走线估算 + pin 碰撞 + bbox 碰撞 + 越界 + 列冲突 + 紧凑度。
//!
//! 设计要点:
//! - **MST (Minimum Spanning Tree) on pin positions**: 每个 net 算一次 Kruskal
//!   MST, 边长按 breadboard 物理距离计算:
//!   - **同列同 rail: 0** (rail 短接, 无需 wire)
//!   - **同 rail 不同 col: |Δcol|** (走 jumper wire 在同一行)
//!   - **同 col 不同 rail: |Δrow|** (跨中央通道)
//!   - **不同 col 不同 rail: |Δcol| + |Δrow|** (Manhattan)
//!   比 2D HPWL 准 — HPWL 把 "同列不同 row 同 rail" 算成 Δrow, MST 直接 0, 推动
//!   SA 主动寻找 rail 短接的零跳线布局。
//! - **紧凑度**: 按 rail 分组算 union bbox 面积加和, 阻止 SA 停在"零冲突但留白大"的状态。
//!   按 rail 切分避免中央通道把"实际占 1 行"的布局算成跨 2 行。
//! - 成本是各项**加权和**, 权在 [`Weights`] 里调。
//! - `SAState` 是 SA 内部状态, 只在 layout 子模块内共享; v2 起每个元件显式
//!   持有 `(x, y, rotation)`, 不再由 order 推 x。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;
use crate::layout::placement::{BBox, Rotation, rotate};

/// SA 成本函数的六项权重。
///
/// 成本 = `hpwl * HPWL + pin_overlap * pin_pin_碰撞 + b_box_overlap * bbox_重叠格数
///       + column_conflict * 列短路对数 + out_of_bounds * 越界 pin 数
///       + compactness * (按 rail 分组的 union bbox 面积之和) + rail_crossing (用 2+ rail 时)`
///
/// 默认值见 [`Weights::default`], 经验起点; 真用时按板子拥挤程度调。
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub hpwl: f64,
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
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            // 一根 5 孔 wire 省下 ~5 成本
            hpwl: 1.0,
            // 一次 pin 碰撞 = 让 SA 宁愿多绕 50-100 孔也不撞
            pin_overlap: 100.0,
            // bbox 重叠基本也当硬约束, 跟 pin 碰撞同量级 (一个孔算 1)。
            b_box_overlap: 100.0,
            // 同列不同 net 的 pin 会被面包板竖向 rail 短接, 这是物理电气短路,
            // 不能让走线"治愈"。惩罚拉到 out_of_bounds 同级, 让 SA 当作硬约束。
            column_conflict: 1_000_000.0,
            // 越界基本不允许; 巨大惩罚让 SA 直接拒绝
            out_of_bounds: 1_000_000.0,
            // 紧凑度: 1 cell² ≈ 0.5 MST cell 的代价, 让 HPWL 仍有空间优化跨列 net,
            // 但空隙会被这股力挤掉。
            compactness: 0.5,
            // 跨 rail = 多一根 jumper + 视觉割裂, 取约 5 cell MST, 比单 cell 紧凑贵
            // 但比 column_conflict 软得多, 不会让 SA 为了"必须跨 rail 的电路"去撞列冲突。
            rail_crossing: 5.0,
        }
    }
}

/// SA 内部状态: 每个元件显式持有 `(x, y, rotation)`。
///
/// v2 起 (显式 2D 布局), x 不再由 order 推导, SA 可以把元件放到板子任意位置。
/// order 还在, 但只用于标识; `placeable[i]` 的 i 索引对应 `x[i] / y[i] / rotation[i]`。
#[derive(Debug, Clone)]
pub struct SAState {
    pub placeable: Vec<ComponentId>,
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub rotation: Vec<Rotation>,
}

impl SAState {
    pub fn n(&self) -> usize {
        self.placeable.len()
    }

    /// 简单构造: 给定元件顺序, 全部 R0, 全部同一行, x 按顺序累加 (gap=1)。
    /// 主要给测试用——真实初始状态用 [`SAState::from_greedy`]。
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
                x: vec![],
                y: vec![],
                rotation: vec![],
            };
        }

        // 1. Build adjacency: weights[i][j] = 同一网的连接数 (高 = 强耦合)
        let mut weights = vec![vec![0.0f64; n]; n];
        for net in circuit.nets() {
            let mut comps: Vec<usize> = net
                .pins()
                .iter()
                .map(|&pid| circuit.pins[pid.0].component.0)
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
            x,
            y,
            rotation,
        }
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

/// 评估当前状态的 cost。
pub fn cost(state: &SAState, circuit: &Circuit, board: &Breadboard, w: &Weights) -> f64 {
    let cols_i = board.cols() as i32;

    // 1. 收集所有 pin 的 (col, row, rail_id) 和所属 net, 以及每个元件的 bbox。
    //    rail_id = u32::MAX 表示"该位置没有 HoleId" (越界 / blocked / 电源轨 gap)
    //    — 这些 pin 不参与 MST / 列冲突, 但算 OOB。
    let mut holes: Vec<(i32, i32, u32)> = Vec::new();
    let mut nets: Vec<Option<NetId>> = Vec::new();
    let mut bboxes: Vec<Option<BBox>> = Vec::with_capacity(state.placeable.len());
    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.unwrap();
        let footprint = &circuit.footprints[fid.0];
        let rotation = state.rotation[idx];
        let row_y = state.y[idx];
        let px = state.x[idx];

        let mut world_positions: Vec<crate::circuit::Position> =
            Vec::with_capacity(component.pins.len());
        for &pin_id in &component.pins {
            let pin = &circuit.pins[pin_id.0];
            let physical = footprint
                .pins()
                .iter()
                .find(|pp| pp.name() == pin.num())
                .expect("footprint 缺 pin (解析阶段就该爆)");
            let r = rotate(physical.offset, rotation);
            let x = px + r.x;
            let y = row_y + r.y;
            // 用 board.at 反查: 返回 None 意味着 OOB / blocked row / 电源轨 gap,
            // 全部归为"不能放" (u32::MAX 是 sentinel, 不参与任何短路/冲突)
            let rail_id = board
                .at(x, y)
                .map(|h| board.rail_id_of(h))
                .unwrap_or(u32::MAX);
            holes.push((x, y, rail_id));
            nets.push(pin.net);
            world_positions.push(crate::circuit::Position { x, y });
        }
        bboxes.push(BBox::from_points(world_positions));
    }

    // 2. OOB: rail_id == u32::MAX 即"该位置没有 HoleId" — 越界 / blocked row / 电源轨 gap
    let mut oob_count = 0;
    for &(_, _, rail_id) in &holes {
        if rail_id == u32::MAX {
            oob_count += 1;
        }
    }

    // 3. Pin 碰撞: 每个被多个 pin 占用的孔, 算 N-1 次 (SA 关心 Δcost, 系数 1 vs 系数 N 等价)
    let mut coll_count = 0;
    let mut seen: HashMap<(i32, i32, u32), ()> = HashMap::new();
    for &hole in &holes {
        if hole.2 == u32::MAX {
            continue; // OOB pin 不参与碰撞检查
        }
        if seen.insert(hole, ()).is_some() {
            coll_count += 1;
        }
    }

    // 4. bbox 碰撞: 任意两个元件的 bbox 重叠的孔数 (含 pin-pin 重叠, 会和上面
    //    pin 碰撞重叠计入, 但这是两个独立的成本来源, 让 SA 同时压低两者)。
    let mut bbox_overlap_count = 0;
    for i in 0..bboxes.len() {
        let Some(bi) = bboxes[i] else { continue };
        for j in (i + 1)..bboxes.len() {
            let Some(bj) = bboxes[j] else { continue };
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

    // 5. MST 走线估算: 按 net 聚合, 每个 net 算一次 Kruskal 最小生成树。
    //    短路判定改用 rail_id: 同 rail_id = 0 (不管这是 vertical 还是 power rail)。
    //    不同 rail_id = Manhattan 距离。
    let mut by_net: HashMap<NetId, Vec<(i32, i32, u32)>> = HashMap::new();
    for (i, &net_opt) in nets.iter().enumerate() {
        let hole = holes[i];
        if hole.2 == u32::MAX {
            continue; // OOB pin 不参与 MST
        }
        if let Some(net) = net_opt {
            by_net.entry(net).or_default().push(hole);
        }
    }
    let mut mst_sum = 0.0;
    for pins in by_net.values() {
        mst_sum += mst_wire_length(pins);
    }

    // 6. 列冲突 (更准确叫 "rail 冲突"): 按 rail_id 聚合, 计数"不同 net" 的对数。
    //    之前用 (col, rail_top) 作为 key, 引入电源轨后不够 — 电源轨里两个不同 col 的
    //    孔在同一 rail_id (横向短接), 也会被这块板短路。统一用 rail_id。
    let mut by_rail: HashMap<u32, Vec<Option<NetId>>> = HashMap::new();
    for (i, &net_opt) in nets.iter().enumerate() {
        let (_, _, rail_id) = holes[i];
        if rail_id == u32::MAX {
            continue;
        }
        by_rail.entry(rail_id).or_default().push(net_opt);
    }
    let mut col_conflict_pairs = 0usize;
    for rail_owners in by_rail.values() {
        if rail_owners.len() < 2 {
            continue;
        }
        let base = rail_owners[0];
        for i in 1..rail_owners.len() {
            if rail_owners[i] != base {
                col_conflict_pairs += 1;
            }
        }
    }

    // 7. 紧凑度: 按 rail 分组, 每组算 union bbox 面积, 加和; 跨 rail 再加一个固定惩罚。
    //    阻止 SA 停在"cost=0 但留白大"的状态。
    //    跳过 OOB / blocked row 的 bbox: 那些是被 OOB / state_y_valid 硬卡掉的, 进了 union
    //    反而把面积算虚了; 反正 cost 会被 OOB 惩罚主导, 不影响 SA 走向。
    //    bbox 在 main board 的 vertical rail 上: 用 rail_top 做 key (同 rail 的所有 row 汇到一处)
    let mut by_rail: HashMap<i32, Vec<BBox>> = HashMap::new();
    for bbox in bboxes.iter().flatten() {
        if bbox.min_x < 0
            || bbox.max_x >= cols_i
            || bbox.min_y < 0
            || bbox.min_y >= board.main_rows() as i32
            || board.is_blocked(bbox.min_y as usize)
        {
            continue;
        }
        // 拿 bbox 所在 rail 的"顶行"做 bucket key, 同 rail 的所有 row 都汇到一处。
        let rail_top = board
            .rail_rows(bbox.min_y)
            .first()
            .copied()
            .unwrap_or(bbox.min_y);
        by_rail.entry(rail_top).or_default().push(*bbox);
    }
    let mut area_sum = 0.0;
    for cells in by_rail.values() {
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for b in cells {
            min_x = min_x.min(b.min_x);
            max_x = max_x.max(b.max_x);
            min_y = min_y.min(b.min_y);
            max_y = max_y.max(b.max_y);
        }
        if min_x <= max_x && min_y <= max_y {
            // width × height 自然对待 x / y: 1 cell 宽 = 1 cell 高的"成本贡献"。
            // (单 rail 内所有元件 1 行高时, height = 1, area 退化为 width。)
            let width = (max_x - min_x + 1) as f64;
            let height = (max_y - min_y + 1) as f64;
            area_sum += width * height;
        }
    }
    // 用 2+ rail: 固定惩罚一项, 不乘以 rail 数 (不是"跨得越多越贵", 是"跨就有成本")。
    let rail_cross = if by_rail.len() >= 2 {
        w.rail_crossing
    } else {
        0.0
    };

    w.hpwl * mst_sum
        + w.pin_overlap * coll_count as f64
        + w.b_box_overlap * bbox_overlap_count as f64
        + w.column_conflict * col_conflict_pairs as f64
        + w.out_of_bounds * oob_count as f64
        + w.compactness * area_sum
        + rail_cross
}

/// 给一个 net 的 pin 位置算 MST (Kruskal) 总长度。
///
/// breadboard 物理距离 (短路抽象为 `rail_id`):
/// - 同 `rail_id`: **0** (面包板内部短接, 无论是 vertical rail 还是 power rail)
/// - 不同 `rail_id`: Manhattan |Δcol| + |Δrow|
///
/// 这是 wire 长度的下界 — 实际走线可能更长 (绕障碍), 但 SA 用它做优化目标。
fn mst_wire_length(pins: &[(i32, i32, u32)]) -> f64 {
    let n = pins.len();
    if n < 2 {
        return 0.0;
    }

    let dist = |a: (i32, i32, u32), b: (i32, i32, u32)| -> i32 {
        if a.2 == b.2 {
            0 // 同 rail (vertical 或 power) 短接
        } else {
            (a.0 - b.0).abs() + (a.1 - b.1).abs()
        }
    };

    // Kruskal: 列所有候选边 → 排序 → 贪心加边 (union-find 判环)
    let mut edges: Vec<(i32, usize, usize)> = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in (i + 1)..n {
            edges.push((dist(pins[i], pins[j]), i, j));
        }
    }
    edges.sort_by_key(|e| e.0);

    let mut parent: Vec<usize> = (0..n).collect();
    let mut find = |parent: &mut Vec<usize>, mut x: usize| -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    };

    let mut total: i32 = 0;
    let mut edges_used = 0;
    for (d, i, j) in edges {
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

    /// 只关心 HPWL / pin / bbox / column 各项的测试用, 屏蔽新加的紧凑度和跨 rail 惩罚。
    /// 不想让"layout 跨几行" 之类的全局性质混入到孤立某项成本的断言里。
    fn weights_legacy() -> Weights {
        Weights {
            compactness: 0.0,
            rail_crossing: 0.0,
            ..Weights::default()
        }
    }

    #[test]
    fn empty_state_costs_zero() {
        let (circuit, _) = two_pin_in_net();
        let state = SAState::from_order(vec![], 2, &[]);
        let c = cost(&state, &circuit, &board(), &Weights::default());
        assert_eq!(c, 0.0);
    }

    #[test]
    fn one_component_same_net_hpwl_is_one() {
        // 2 pin 紧挨着 (0, 2) 和 (1, 2), 都在同一 net → HPWL = 1 - 0 = 1
        let (circuit, cid) = two_pin_in_net();
        let state = SAState::from_order(vec![cid], 2, &[2]);
        let c = cost(&state, &circuit, &board(), &weights_legacy());
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
        let c_clean = cost(&state, &circuit, &board(), &weights_legacy());
        assert_eq!(c_clean, 0.0);

        // 撞: x = [0, 0]
        state.x = vec![0, 0];
        let c_coll = cost(&state, &circuit, &board(), &weights_legacy());
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
        let c_clean = cost(&s, &circuit, &board(), &weights_legacy());
        assert_eq!(c_clean, 0.0);

        // 冲突: x = [0, 0] (同列, 同孔 → pin_collision + bbox_collision + column_conflict)
        let mut s = state.clone();
        s.x = vec![0, 0];
        let c_coll = cost(&s, &circuit, &board(), &weights_legacy());
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
        let c_col_only = cost(&s, &circuit, &board(), &weights_legacy());
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
        let c = cost(&state, &circuit, &board, &weights_legacy());
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
        let c = cost(&state, &circuit, &board(), &Weights::default());
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
        let xs: Vec<i32> = state.x.iter().copied().collect();
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
        let xs: Vec<i32> = state.x.iter().copied().collect();
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
                    abs_x >= 0 && abs_x < 30,
                    "x OOB: {} from {}",
                    abs_x,
                    state.x[idx]
                );
                assert!(
                    abs_y >= 0 && abs_y < 5,
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
                if abs_x < 0 || abs_x >= 30 || abs_y < 0 || abs_y >= 5 {
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
            for i in 1..col_owners.len() {
                if col_owners[i] != base {
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
        // top negative y=-2, col 0 和 col 10
        let len = mst_wire_length(&[pin(&b, 0, -2), pin(&b, 10, -2)]);
        assert_eq!(len, 0.0, "同 power rail 行内应该 shorted, MST = 0");
    }

    /// 同极性 top + bottom (用户约定: 短接 + 同一 net) → MST = 0
    #[test]
    fn mst_top_and_bottom_same_polarity_is_zero() {
        let b = Breadboard::standard();
        let len = mst_wire_length(&[pin(&b, 0, -2), pin(&b, 0, 12)]);
        assert_eq!(len, 0.0, "上下两条同极性应该 shorted, MST = 0");
    }

    /// 正负极 → MST = Manhattan (不短接)
    #[test]
    fn mst_positive_and_negative_is_manhattan() {
        let b = Breadboard::standard();
        // (0, -2) negative, (6, -1) positive → |6| + |1| = 7
        // (6 是 group 第二个的开始: cols 6..10)
        let len = mst_wire_length(&[pin(&b, 0, -2), pin(&b, 6, -1)]);
        assert_eq!(len, 7.0, "正负极不短接, MST = Manhattan");
    }

    /// Power rail 跟 main board → MST = Manhattan (rail_id 不同)
    #[test]
    fn mst_power_rail_to_main_is_manhattan() {
        let b = Breadboard::standard();
        // top negative (0, -2) 跟 main upper (0, 0): |0| + |2| = 2
        let len = mst_wire_length(&[pin(&b, 0, -2), pin(&b, 0, 0)]);
        assert_eq!(len, 2.0);
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
        };
        let c = cost(&state, &circuit, &board(), &weights_legacy());
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
        // 屏蔽 HPWL / pin / bbox / column, 只看 compactness
        let w = Weights {
            hpwl: 0.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            ..Weights::default()
        };
        // 都同 row 2, x 贴在一起 (但不同 col, 不撞 pin)
        let s_tight = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 1],
            y: vec![2, 2],
            rotation: vec![Rotation::R0; 2],
        };
        let c_tight = cost(&s_tight, &circuit, &board(), &w);
        // 同 row 2, x 拉开 (0, 5) → bbox 6 × 1 = 6 → cost 3.0
        let s_wide = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 5],
            y: vec![2, 2],
            rotation: vec![Rotation::R0; 2],
        };
        let c_wide = cost(&s_wide, &circuit, &board(), &w);
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
            hpwl: 0.0,
            pin_overlap: 0.0,
            b_box_overlap: 0.0,
            column_conflict: 0.0,
            ..Weights::default()
        };

        // 拉开 5 cells (x 0..4, width 5) → cost = 0.5 * 5 * 1 = 2.5 (同 row)
        let s_horiz = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 4],
            y: vec![2, 2],
            rotation: vec![Rotation::R0; 2],
        };
        let c_horiz = cost(&s_horiz, &circuit, &board(), &w);

        // 拉开 5 cells (y 0..4, height 5) → cost = 0.5 * 1 * 5 = 2.5 (同 col)
        let s_vert = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 0],
            y: vec![0, 4],
            rotation: vec![Rotation::R0; 2],
        };
        let c_vert = cost(&s_vert, &circuit, &board(), &w);

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
            hpwl: 0.0,
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
        };
        let c_same = cost(&s_same, &circuit, &board, &w);

        // 跨 rail (中央通道两侧): 加 rail_crossing
        let s_cross = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 0],
            y: vec![0, 10], // 上 + 下
            rotation: vec![Rotation::R0; 2],
        };
        let c_cross = cost(&s_cross, &circuit, &board, &w);

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
            hpwl: 0.0,
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
        };
        let c_all_upper = cost(&s_all_upper, &circuit, &board, &w);

        // 1 个下 rail (y=10), 2 个上 rail (y=0, 1)
        let s_split = SAState {
            placeable: vec![ComponentId(0), ComponentId(1), ComponentId(2)],
            x: vec![0, 1, 2],
            y: vec![0, 1, 10],
            rotation: vec![Rotation::R0; 3],
        };
        let c_split = cost(&s_split, &circuit, &board, &w);

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
}
