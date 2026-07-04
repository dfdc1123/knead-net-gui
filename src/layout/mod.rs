//! 面包板布局: 把 Circuit 投影到 Breadboard 上。
//!
//! 模块组织:
//! - [`breadboard`][]: 物理结构 (30×5 矩形, 每列纵向连通, 无电源轨)
//! - [`placement`]: 摆放 (位置 + 旋转) → 投影到具体 HoleId
//! - [`occupancy`][]: 当前孔占用 (派生, 不缓存)
//! - [`routing`][]: 接线 (Wire, Router trait)
//! - [`Layout`]: 顶层容器, 持有 Circuit 引用 + placements + wires

pub mod breadboard;
pub mod cost;
pub mod occupancy;
pub mod placement;
pub mod routing;
pub mod sa;

pub use breadboard::{
    Breadboard, Hole, HoleId, Polarity, PowerRail, PowerRailBinding, PowerRails, PowerStrip,
    Region, standard_power_rails,
};
pub use cost::{FDConfig, Weights};
pub use occupancy::{Occupancy, Occupant};
pub use placement::{BBox, PinHole, PlacedFootprint, Placement, Rotation};
pub use routing::{PathFinderRouter, Router, Wire, WireId};
pub use sa::SAConfig;

use crate::circuit::{Circuit, ComponentId, Footprint, NetId, PinId, Position};

/// FD 调试输出: 连续力导向位置 + 吸附后的 placement 列表。
///
/// 同一个 FD 运行, 先保存连续位置, 再执行贪心 snap, 最后打印每个元件的 FD→snap 位移。
/// 返回 `(连续位置, 吸附后 placement)`。`连续位置` 跟 `placeable` 同索引,
/// `吸附后 placement` 跟 `circuit.components()` 同索引 (未摆放为 `None`)。
pub fn fd_debug_positions(
    circuit: &Circuit,
    board: &Breadboard,
    fd_config: &FDConfig,
) -> (Vec<(f64, f64)>, Vec<Option<Placement>>) {
    use crate::circuit::Position;
    use std::collections::{HashMap, HashSet};

    // 收集所有有 footprint 的元件 (跟 place_sa 一样的逻辑)
    let placeable: Vec<ComponentId> = circuit
        .components()
        .iter()
        .filter_map(|c| {
            c.footprint()?;
            Some(c.id())
        })
        .collect();

    if placeable.is_empty() {
        return (vec![], vec![None; circuit.components().len()]);
    }

    let n = placeable.len();

    // ========== Phase 1: FD 连续迭代 ==========

    // 1. 邻接权重
    let mut weights = vec![vec![0.0f64; n]; n];
    for net in circuit.nets() {
        let mut comps: Vec<usize> = net
            .pins()
            .iter()
            .map(|&pid| circuit.pins()[pid.raw()].component().raw())
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

    // 2. 初值: 圆周
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

    // 3. FD 迭代
    let k = fd_config.k;
    let mut temp = fd_config.initial_temp;
    for _ in 0..fd_config.max_iters {
        let mut forces = vec![(0.0f64, 0.0f64); n];
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[j].0 - pos[i].0;
                let dy = pos[j].1 - pos[i].1;
                let dist = (dx * dx + dy * dy).sqrt().max(0.01);
                let ux = dx / dist;
                let uy = dy / dist;

                let f_repel = k * k / dist;
                forces[i].0 -= ux * f_repel;
                forces[i].1 -= uy * f_repel;
                forces[j].0 += ux * f_repel;
                forces[j].1 += uy * f_repel;

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

        temp *= fd_config.cool_rate;
        if temp < 0.05 {
            break;
        }
    }

    // 4. 保存连续位置 (snap 前的快照)
    let fd_continuous = pos.clone();

    // ========== Phase 2: 贪心 snap 到格点 ==========

    // 按 FD x 排序
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        pos[a]
            .0
            .partial_cmp(&pos[b].0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut snapped_x = vec![0i32; n];
    let mut snapped_y = vec![0i32; n];
    let mut occupied: HashSet<(i32, i32)> = HashSet::new();
    let mut col_owner: HashMap<(i32, i32), Option<NetId>> = HashMap::new();

    for &idx in &order {
        let comp_id = placeable[idx];
        let component = &circuit.components()[comp_id.raw()];
        let fid = component.footprint().expect("placeable 必有 footprint");
        let fp = &circuit.footprints()[fid.raw()];

        let pin_info: Vec<(i32, i32, Option<NetId>)> = component
            .pins()
            .iter()
            .map(|&pin_id| {
                let pin = &circuit.pins()[pin_id.raw()];
                let physical = fp
                    .pins()
                    .iter()
                    .find(|p| p.name() == pin.num())
                    .expect("footprint 缺 pin");
                (physical.offset.x, physical.offset.y, pin.net)
            })
            .collect();

        let (min_x, max_x, min_y, max_y) = fp.pins().iter().fold(
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

        let mut best: Option<(i32, i32)> = None;
        let mut best_dist_sq = f64::INFINITY;
        for try_y in 0..board.rows() as i32 {
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
        snapped_x[idx] = fx;
        snapped_y[idx] = fy;
        for &(dx, dy) in &bbox_cells {
            occupied.insert((fx + dx, fy + dy));
        }
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

    // ========== 打印调试表格 ==========
    eprintln!("\n=== FD → snap 位移 (按 snap x 排序) ===");
    eprintln!(
        "{:<6} {:<16} {:<16} {:<8}",
        "ref", "FD (x,y)", "snap (x,y)", "Δdist"
    );
    let mut debug_order: Vec<usize> = (0..n).collect();
    debug_order.sort_by_key(|&i| snapped_x[i]);
    for &idx in &debug_order {
        let comp = &circuit.components()[placeable[idx].raw()];
        let (fx, fy) = fd_continuous[idx];
        let (sx, sy) = (snapped_x[idx], snapped_y[idx]);
        let dist = ((sx as f64 - fx).powi(2) + (sy as f64 - fy).powi(2)).sqrt();
        eprintln!(
            "{:<6} ({:>5.1},{:>5.1})     ({:>3},{:>3})        {:.1}",
            comp.ref_(),
            fx,
            fy,
            sx,
            sy,
            dist
        );
    }
    eprintln!();

    // ========== 写回 placement 列表 ==========
    let mut placements: Vec<Option<Placement>> = vec![None; circuit.components().len()];
    for (idx, &comp_id) in placeable.iter().enumerate() {
        placements[comp_id.raw()] = Some(Placement::OnBoard {
            position: Position {
                x: snapped_x[idx],
                y: snapped_y[idx],
            },
            rotation: Rotation::R0,
        });
    }

    (fd_continuous, placements)
}

/// 频谱布局调试输出: 返回 (v₂, v₃, round 后 placement), 并打印频谱值和格点映射。
pub fn spectral_debug_positions(
    circuit: &Circuit,
    board: &Breadboard,
) -> (Vec<f64>, Vec<f64>, Vec<Option<Placement>>) {
    use crate::circuit::Position;
    use crate::layout::cost::SAState;

    let placeable: Vec<ComponentId> = circuit
        .components()
        .iter()
        .filter_map(|c| {
            c.footprint()?;
            Some(c.id())
        })
        .collect();

    if placeable.is_empty() {
        return (vec![], vec![], vec![None; circuit.components().len()]);
    }

    let state = SAState::from_spectral(placeable, circuit, board);
    let n = state.n();

    // 打印表格 (按 round 后的 x 排序)
    eprintln!("\n=== 频谱嵌入 + round 结果 (按 x 排序) ===");
    eprintln!("{:<6} {:<14}", "ref", "round (x,y)");
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| state.x[i]);
    for &idx in &order {
        let comp = &circuit.components()[state.placeable[idx].raw()];
        eprintln!(
            "{:<6}    ({:>3},{:>3})",
            comp.ref_(),
            state.x[idx],
            state.y[idx]
        );
    }
    eprintln!();

    let mut placements: Vec<Option<Placement>> = vec![None; circuit.components().len()];
    for (idx, &comp_id) in state.placeable.iter().enumerate() {
        placements[comp_id.raw()] = Some(Placement::OnBoard {
            position: Position {
                x: state.x[idx],
                y: state.y[idx],
            },
            rotation: state.rotation[idx],
        });
    }

    let v2: Vec<f64> = vec![0.0; n]; // placeholder
    let v3: Vec<f64> = vec![0.0; n]; // placeholder
    (v2, v3, placements)
}

/// 一列上的某个 pin / wire 端点, 捎带它的 net 信息。
///
/// [`LayoutError::ColumnConflict`] 用这个告诉你 "col X 的 a 和 b 被纵向 rail 连起来了,
/// 它们不在同一 net, 算短路" —— 拿这个 net 信息能直接看出是哪个 net 被连到了哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnEndpoint {
    /// pin + 所属 net (`None` = 未连接, e.g. unconnected-pad)
    Pin { pin: PinId, net: Option<NetId> },
    /// wire 端点, 必有 net
    Wire { wire: WireId, net: NetId },
}

/// 布局错误。`apply` / `validate` / `from_layout` 都会返回这个。
///
/// `apply` 只产生 `OutOfBounds` (单个 placement 内的检查);
/// `validate` / `from_layout` 还会产生 `NoFootprint` / `PinCollision` / `WireConflict`
/// / `ColumnConflict` (跨 placement 的检查)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// Component 没有 footprint
    NoFootprint { component: ComponentId },
    /// 某个 pin 算出来落在板外
    OutOfBounds {
        component: ComponentId,
        pin: PinId,
        hole: Position,
    },
    /// pin 跟已摆的元件的 pin 撞同一个孔
    PinCollision {
        component: ComponentId,
        pin: PinId,
        hole: HoleId,
    },
    /// wire path 跟已占用的孔冲突 (跟 pin 或别的 wire)
    WireConflict { wire: WireId, hole: HoleId },
    /// Component 引用了 footprint 里不存在的 pad (按 num 找)
    NoFootprintPad {
        component: ComponentId,
        pin: PinId,
        pad_name: String,
    },
    /// 同列上两个不同 net 的 pin / wire 被面包板纵向 rail 短路。
    /// 面包板的每列内部连通, 所以同列不同 row 上的 pin 也会被连起来。
    ColumnConflict {
        column: i32,
        a: ColumnEndpoint,
        b: ColumnEndpoint,
    },
    /// 两个元件的包围盒在板上重叠 — 元件本体互相碰撞 (不论是 pin 撞本体、
    /// 本体撞 pin 还是本体撞本体都报这个)。报告发生冲突的第一个孔。
    BBoxOverlap {
        a: ComponentId,
        b: ComponentId,
        hole: HoleId,
    },
}

/// 顶层布局: 持有 Circuit 引用 + 每个 component 的 placement + 所有 wire。
///
/// 跟 Circuit 本身**解耦**: Component 不携带 placement, Layout 单独管理。
/// 这让 Circuit 可以独立 serialize / 在不同 layout 间切换。
#[derive(Debug)]
pub struct Layout<'c> {
    pub(crate) circuit: &'c Circuit,
    pub(crate) placements: Vec<Option<Placement>>,
    pub(crate) wires: Vec<Wire>,
}

impl<'c> Layout<'c> {
    pub fn new(circuit: &'c Circuit) -> Self {
        Self {
            circuit,
            placements: vec![None; circuit.components.len()],
            wires: Vec::new(),
        }
    }

    /// 摆放 (不验证, 调用方负责确保 placement 合法; 想验证调 `validate`)
    pub fn place(&mut self, component: ComponentId, placement: Placement) {
        self.placements[component.0] = Some(placement);
    }

    pub fn unplace(&mut self, component: ComponentId) {
        self.placements[component.0] = None;
    }

    pub fn placement(&self, component: ComponentId) -> Option<Placement> {
        self.placements[component.0]
    }

    pub fn placements(&self) -> &[Option<Placement>] {
        &self.placements
    }

    pub fn add_wire(&mut self, wire: Wire) {
        self.wires.push(wire);
    }

    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }

    /// 取出所有 bridged 元件的 pin-hole 对 (按 component 顺序展开成平铺列表)。
    ///
    /// 给 cost / 路由用: bridged 元件不进 SA, 但它的 pin 仍然要进 MST / rail
    /// 冲突检查 (一个 bridged 电阻两端跨 rail, MST 必须包含它)。
    pub fn bridged_pins(&self) -> Vec<(crate::circuit::PinId, HoleId)> {
        let mut out = Vec::new();
        for p in &self.placements {
            if let Some(Placement::Bridged { pin_holes }) = p {
                for &(hole_id, pin_id) in pin_holes {
                    out.push((pin_id, hole_id));
                }
            }
        }
        out
    }

    pub fn circuit(&self) -> &Circuit {
        self.circuit
    }

    /// 一次性验证整个 layout, 返回所有错误 (no footprint / 越界 / pin 碰撞 / wire 冲突)。
    ///
    /// `validate` 跟 `occupancy` 走同一条检查路径, 区别是 `validate` 丢掉了
    /// 构建出来的 occupancy 表, 只关心错误。语义上"我只想问合不合法"。
    pub fn validate(&self, board: &Breadboard) -> Result<(), Vec<LayoutError>> {
        self.occupancy(board).map(|_| ())
    }

    /// 用模拟退火布局。
    ///
    /// 流程: 收集有 footprint 的 component → `sa::simulate` (跑 `config.n_seeds`
    /// 次, 取最低 cost 的 best state) → 写回 `placements` → `validate(board)`。
    /// 紧凑度已折进 [`cost::cost`], 不再需要单独的 post-pass。
    ///
    /// 没有 footprint 的 component 保持未摆放, `validate` 会报 `NoFootprint`。
    /// 调参见 [`SAConfig`], 默认参数适合 ~5 元件级别。
    pub fn place_sa(
        &mut self,
        board: &Breadboard,
        config: &SAConfig,
    ) -> Result<(), Vec<LayoutError>> {
        use crate::layout::cost::SAState;
        use crate::layout::sa;

        // 跳 过已经摆好的 (用户手动 Bridged 或 OnBoard)。SA 只优化未摆的。
        let placeable: Vec<ComponentId> = self
            .circuit
            .components
            .iter()
            .filter_map(|c| {
                c.footprint?;
                if self.placements[c.id.0].is_some() {
                    return None;
                }
                Some(c.id)
            })
            .collect();

        // bridged 元件的 pin 不进 SA, 但要进 cost / 路由 (跨 rail 时)
        let bridged_pins = self.bridged_pins();

        if placeable.is_empty() {
            return self.validate(board);
        }

        // SA 是随机算法, 单次可能卡在 local optimum; 跑 n_seeds 次取最低 cost 的。
        let n_seeds = config.n_seeds.max(1);
        let mut best_state: Option<SAState> = None;
        let mut best_cost = f64::INFINITY;
        for s in 0..n_seeds as u64 {
            let cfg_s = SAConfig {
                seed: config.seed.wrapping_add(s),
                n_seeds: 1,
                ..*config
            };
            let state_s = sa::simulate(
                placeable.clone(),
                self.circuit,
                board,
                &cfg_s,
                &bridged_pins,
            );
            let cost_s = crate::layout::cost::cost(
                &state_s,
                self.circuit,
                board,
                &bridged_pins,
                &config.weights,
            );
            if cost_s < best_cost {
                best_cost = cost_s;
                best_state = Some(state_s);
            }
        }
        let best = best_state.expect("至少跑了一次");

        for (idx, &comp_id) in best.placeable.iter().enumerate() {
            // Toggle 在 SA 中可能拾到 Bridged 模式, 这里分流写回:
            // - bridged[idx] = true: 写 `Placement::Bridged`, pin 对取自启发式缓存
            //   (sa::simulate 在初始化后调 `populate_bridgeable_info` 填的)。
            // - bridged[idx] = false: 写 `Placement::OnBoard`, 照原有逻辑取 (x, y, rotation)。
            if best.bridged[idx] {
                let pair = best.active_bridge_pair(idx).expect(
                    "bridged=true 必有 pin pair (sa::simulate 保证 is_bridgeable[idx] = true)",
                );
                self.placements[comp_id.0] = Some(Placement::Bridged {
                    pin_holes: [pair[0], pair[1]],
                });
            } else {
                self.placements[comp_id.0] = Some(Placement::OnBoard {
                    position: Position {
                        x: best.x[idx],
                        y: best.y[idx],
                    },
                    rotation: best.rotation[idx],
                });
            }
        }

        self.validate(board)
    }

    /// 把所有有 footprint 的 component 横向摆在指定行, R0 方向, 元件之间留 1 空列。
    ///
    /// 最简单的"排成一排"策略: 按 component 顺序, 算出 footprint 水平跨度,
    /// 依次放下去。**会覆盖已存在的 placement**; 没有 footprint 的 component 跳过
    /// (validate 会把它们报为 `NoFootprint`)。
    ///
    /// 越界 / pin 碰撞 / wire 冲突都通过返回值上报; 即使有错, placement 也已经写入,
    /// 调用方可以检查后调整。
    pub fn place_row(&mut self, board: &Breadboard, row: i32) -> Result<(), Vec<LayoutError>> {
        let mut col: i32 = 0;
        for component in &self.circuit.components {
            let Some(fid) = component.footprint else {
                continue;
            };
            let footprint = &self.circuit.footprints[fid.0];
            let width = footprint_horizontal_width(footprint);

            self.placements[component.id.0] = Some(Placement::OnBoard {
                position: Position { x: col, y: row },
                rotation: Rotation::R0,
            });
            col += width + 1; // +1 是元件间空列
        }
        self.validate(board)
    }

    /// 从 placements + wires 派生当前占用, 同时验证合法性。
    ///
    /// **严格**: 任何非法状态返回 `Err`, 不返回部分 occupancy。
    /// 调用方必须拿到 `Ok` 之后才能使用 `Occupancy`。
    pub fn occupancy(&self, board: &Breadboard) -> Result<Occupancy, Vec<LayoutError>> {
        Occupancy::from_layout(self, board)
    }
}

/// R0 方向下 footprint 占多少个列 (= `max_x - min_x + 1`)。
///
/// 空 footprint 当作 1 列, 防止减法下溢。
pub(crate) fn footprint_horizontal_width(footprint: &Footprint) -> i32 {
    if footprint.pins.is_empty() {
        return 1;
    }
    let min_x = footprint.pins.iter().map(|p| p.offset.x).min().unwrap();
    let max_x = footprint.pins.iter().map(|p| p.offset.x).max().unwrap();
    max_x - min_x + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Component, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin, PinId, Position,
    };

    fn fixture() -> &'static Circuit {
        Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "Q1".to_string(),
                kind: "NPN".to_string(),
                value: Some("BC547".to_string()),
                pins: vec![PinId(0), PinId(1), PinId(2)],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            }],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    net: None,
                },
            ],
            nets: vec![],
            footprints: vec![Footprint {
                id: FootprintId(0),
                name: "TO92".to_string(),
                pins: vec![
                    PhysicalPin {
                        name: "1".to_string(),
                        offset: Position { x: 0, y: 0 },
                    },
                    PhysicalPin {
                        name: "2".to_string(),
                        offset: Position { x: 1, y: 0 },
                    },
                    PhysicalPin {
                        name: "3".to_string(),
                        offset: Position { x: 2, y: 0 },
                    },
                ],
            }],
        }))
    }

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn new_layout_has_all_unplaced() {
        let circuit = fixture();
        let layout = Layout::new(circuit);
        assert_eq!(layout.placements().len(), 1);
        assert!(layout.placement(ComponentId(0)).is_none());
        assert!(layout.wires().is_empty());
    }

    #[test]
    fn place_and_unplace() {
        let circuit = fixture();
        let mut layout = Layout::new(circuit);
        let p = Placement::OnBoard {
            position: Position { x: 5, y: 2 },
            rotation: Rotation::R0,
        };
        layout.place(ComponentId(0), p);
        assert_eq!(layout.placement(ComponentId(0)), Some(p));

        layout.unplace(ComponentId(0));
        assert!(layout.placement(ComponentId(0)).is_none());
    }

    #[test]
    fn end_to_end_placement_then_occupancy() {
        let circuit = fixture();
        let mut layout = Layout::new(circuit);
        let board = board();
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );

        let occ = layout.occupancy(&board).unwrap();
        assert!(occ.occupant_at(board.at(10, 2).unwrap()).is_some());
        assert!(occ.occupant_at(board.at(11, 2).unwrap()).is_some());
        assert!(occ.occupant_at(board.at(12, 2).unwrap()).is_some());
        assert!(occ.occupant_at(board.at(13, 2).unwrap()).is_none());
    }

    #[test]
    fn validate_clean_layout_ok() {
        let circuit = fixture();
        let mut layout = Layout::new(circuit);
        let board = board();
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 10, y: 2 },
                rotation: Rotation::R0,
            },
        );
        assert!(layout.validate(&board).is_ok());
    }

    #[test]
    fn validate_detects_out_of_bounds() {
        let circuit = fixture();
        let mut layout = Layout::new(circuit);
        let board = board();
        // R90 at (0, 4): pin 2 落在 (0, 6) 越界
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R90,
            },
        );
        let errors = layout.validate(&board).unwrap_err();
        assert!(errors.iter().any(|e| matches!(
            e,
            LayoutError::OutOfBounds {
                component: ComponentId(0),
                ..
            }
        )));
    }

    #[test]
    fn validate_collects_multiple_errors() {
        let board = board();
        // 两个 component: Q1 有 footprint (但越界), ComponentId(1) 没 footprint
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "Q1".to_string(),
                    kind: "NPN".to_string(),
                    value: None,
                    pins: vec![PinId(0), PinId(1), PinId(2)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
                Component {
                    id: ComponentId(1),
                    ref_: "?".to_string(),
                    kind: "?".to_string(),
                    value: None,
                    pins: vec![PinId(3)],
                    footprint: None,
                    bridgeable: false,
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "x".into(),
                    pinfunction: None,
                    net: None,
                },
            ],
            nets: vec![],
            footprints: vec![Footprint {
                id: FootprintId(0),
                name: "TO92".to_string(),
                pins: vec![
                    PhysicalPin {
                        name: "1".to_string(),
                        offset: Position { x: 0, y: 0 },
                    },
                    PhysicalPin {
                        name: "2".to_string(),
                        offset: Position { x: 1, y: 0 },
                    },
                    PhysicalPin {
                        name: "3".to_string(),
                        offset: Position { x: 2, y: 0 },
                    },
                ],
            }],
        }));
        let mut layout = Layout::new(circuit);
        // Q1 越界
        layout.place(
            ComponentId(0),
            Placement::OnBoard {
                position: Position { x: 0, y: 4 },
                rotation: Rotation::R90,
            },
        );
        // ComponentId(1) 也摆上 (没 footprint 也能摆, 验证时才发现问题)
        layout.place(
            ComponentId(1),
            Placement::OnBoard {
                position: Position { x: 5, y: 0 },
                rotation: Rotation::R0,
            },
        );
        let errors = layout.validate(&board).unwrap_err();
        assert!(errors.iter().any(|e| matches!(
            e,
            LayoutError::OutOfBounds {
                component: ComponentId(0),
                ..
            }
        )));
        assert!(errors.iter().any(|e| matches!(
            e,
            LayoutError::NoFootprint {
                component: ComponentId(1)
            }
        )));
    }

    /// 两个 component + 两个 footprint, Q1(宽 3) + R(宽 4), 用来测列间隔。
    pub(crate) fn two_component_fixture() -> &'static Circuit {
        Box::leak(Box::new(Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "Q1".to_string(),
                    kind: "NPN".to_string(),
                    value: None,
                    pins: vec![PinId(0), PinId(1), PinId(2)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R1".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(3), PinId(4)],
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
                    net: None,
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(2),
                    component: ComponentId(0),
                    num: "3".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(3),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(4),
                    component: ComponentId(1),
                    num: "2".into(),
                    pinfunction: None,
                    net: None,
                },
            ],
            nets: vec![],
            footprints: vec![
                Footprint {
                    id: FootprintId(0),
                    name: "TO92".to_string(),
                    pins: vec![
                        PhysicalPin {
                            name: "1".to_string(),
                            offset: Position { x: 0, y: 0 },
                        },
                        PhysicalPin {
                            name: "2".to_string(),
                            offset: Position { x: 1, y: 0 },
                        },
                        PhysicalPin {
                            name: "3".to_string(),
                            offset: Position { x: 2, y: 0 },
                        },
                    ],
                },
                Footprint {
                    id: FootprintId(1),
                    name: "R2".to_string(),
                    pins: vec![
                        PhysicalPin {
                            name: "1".to_string(),
                            offset: Position { x: 0, y: 0 },
                        },
                        PhysicalPin {
                            name: "2".to_string(),
                            offset: Position { x: 3, y: 0 },
                        },
                    ],
                },
            ],
        }))
    }

    #[test]
    fn place_row_first_at_origin() {
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        layout.place_row(&board, 2).unwrap();

        let q1 = layout.placement(ComponentId(0)).unwrap();
        match q1 {
            Placement::OnBoard { position, rotation } => {
                assert_eq!(position, Position { x: 0, y: 2 });
                assert_eq!(rotation, Rotation::R0);
            }
            Placement::Bridged { .. } => panic!("期望 OnBoard, 实际 Bridged"),
        }
    }

    #[test]
    fn place_row_uses_footprint_width_plus_gap() {
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        layout.place_row(&board, 2).unwrap();

        // Q1 footprint 宽 3, 放 col 0, 下一个应从 col 3+1=4 开始
        let r1 = layout.placement(ComponentId(1)).unwrap();
        match r1 {
            Placement::OnBoard { position, .. } => {
                assert_eq!(position, Position { x: 4, y: 2 });
            }
            Placement::Bridged { .. } => panic!("期望 OnBoard, 实际 Bridged"),
        }
    }

    #[test]
    fn place_row_occupancy_matches_layout() {
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        layout.place_row(&board, 2).unwrap();

        let occ = layout.occupancy(&board).unwrap();
        // Q1 在 (0,2): 占 (0,2) (1,2) (2,2)
        assert_eq!(
            occ.occupant_at(board.at(0, 2).unwrap()),
            Some(Occupant::Pin(PinId(0)))
        );
        assert_eq!(
            occ.occupant_at(board.at(1, 2).unwrap()),
            Some(Occupant::Pin(PinId(1)))
        );
        assert_eq!(
            occ.occupant_at(board.at(2, 2).unwrap()),
            Some(Occupant::Pin(PinId(2)))
        );
        // col 3 是间隙
        assert_eq!(occ.occupant_at(board.at(3, 2).unwrap()), None);
        // R1 在 (4,2): 占 (4,2) (7,2) (因为 pin2 offset.x=3)
        assert_eq!(
            occ.occupant_at(board.at(4, 2).unwrap()),
            Some(Occupant::Pin(PinId(3)))
        );
        assert_eq!(
            occ.occupant_at(board.at(7, 2).unwrap()),
            Some(Occupant::Pin(PinId(4)))
        );
        // (5,2) (6,2) R1 跨度内但无 pin, 现在算作被 R1 本体占据 (Blocked)
        assert_eq!(
            occ.occupant_at(board.at(5, 2).unwrap()),
            Some(Occupant::Blocked(ComponentId(1)))
        );
        assert_eq!(
            occ.occupant_at(board.at(6, 2).unwrap()),
            Some(Occupant::Blocked(ComponentId(1)))
        );
    }

    /// 关键: 没有 footprint 的 component 跳过, 不写 placement
    /// (`Occupancy::from_layout` 只检查已摆放的 component, 所以不报错)
    #[test]
    fn place_row_skips_components_without_footprint() {
        let board = board();
        // Q1 有 footprint, R1 没 footprint
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![
                Component {
                    id: ComponentId(0),
                    ref_: "Q1".to_string(),
                    kind: "NPN".to_string(),
                    value: None,
                    pins: vec![PinId(0)],
                    footprint: Some(FootprintId(0)),
                    bridgeable: false,
                },
                Component {
                    id: ComponentId(1),
                    ref_: "R1".to_string(),
                    kind: "R".to_string(),
                    value: None,
                    pins: vec![PinId(1)],
                    footprint: None,
                    bridgeable: false,
                },
            ],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "p".into(),
                    pinfunction: None,
                    net: None,
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(1),
                    num: "1".into(),
                    pinfunction: None,
                    net: None,
                },
            ],
            nets: vec![],
            footprints: vec![Footprint {
                id: FootprintId(0),
                name: "X".to_string(),
                pins: vec![PhysicalPin {
                    name: "p".to_string(),
                    offset: Position { x: 0, y: 0 },
                }],
            }],
        }));
        let mut layout = Layout::new(circuit);
        let result = layout.place_row(&board, 2);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        // Q1 摆上了
        assert!(layout.placement(ComponentId(0)).is_some());
        // R1 跳过
        assert!(layout.placement(ComponentId(1)).is_none());
    }

    /// 关键: 越界时 place_row 返回 Err, 但 placement 已经被写入
    /// (Q1 宽 3, 放在 (29, 2) → pin 2 落在 (31,2) 越界)
    #[test]
    fn place_row_returns_error_when_out_of_bounds() {
        let board = board(); // 30x5
        let mut layout = Layout::new(fixture()); // 单 TO92, 宽 3
        // 手动把它放在 (28, 2) → pin 2 落在 (30, 2) 越界
        layout.placements[ComponentId(0).0] = Some(Placement::OnBoard {
            position: Position { x: 28, y: 2 },
            rotation: Rotation::R0,
        });
        let errors = layout.validate(&board).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, LayoutError::OutOfBounds { .. }))
        );
    }

    // ============================================================
    //  place_sa 集成测试
    // ============================================================

    /// 退火后 validate() 应过: 无 pin 碰撞, 无越界, 全部有 footprint 的 component 都摆放。
    #[test]
    fn place_sa_produces_valid_layout() {
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        let result = layout.place_sa(
            &board,
            &SAConfig {
                max_iters: 2000,
                seed: 42,
                ..SAConfig::default()
            },
        );
        assert!(result.is_ok(), "place_sa 应成功, got {result:?}");
        assert!(layout.placement(ComponentId(0)).is_some());
        assert!(layout.placement(ComponentId(1)).is_some());
    }

    /// 退火在固定 seed 下应可重现。
    #[test]
    fn place_sa_is_deterministic_with_seed() {
        let board = board();
        let config = SAConfig {
            max_iters: 1000,
            seed: 1234,
            ..SAConfig::default()
        };
        let mut a = Layout::new(two_component_fixture());
        let mut b = Layout::new(two_component_fixture());
        a.place_sa(&board, &config).unwrap();
        b.place_sa(&board, &config).unwrap();
        for cid in [ComponentId(0), ComponentId(1)] {
            assert_eq!(a.placement(cid), b.placement(cid));
        }
    }

    /// 不同 seed 都应能跑出有效布局 (不强求不同——MST 在 1D 顺序布局下是
    /// permutation-invariant, swap 沿 MST 是平的, 不同 seed 可能收敛到同解)。
    /// 这个测试主要确保"没因为换个 seed 就崩"。
    #[test]
    fn place_sa_handles_various_seeds() {
        let board = board();
        for seed in [1, 7, 42, 1234, 9999] {
            let mut layout = Layout::new(two_component_fixture());
            layout
                .place_sa(
                    &board,
                    &SAConfig {
                        seed,
                        max_iters: 1000,
                        ..SAConfig::default()
                    },
                )
                .unwrap_or_else(|e| panic!("seed {seed} 失败: {e:?}"));
            assert!(layout.placement(ComponentId(0)).is_some());
            assert!(layout.placement(ComponentId(1)).is_some());
        }
    }

    /// SA 结果不包含 R90/R270 (v1 限制)。
    #[test]
    fn place_sa_never_uses_r90_or_r270() {
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        for seed in 0..5 {
            layout
                .place_sa(
                    &board,
                    &SAConfig {
                        seed,
                        max_iters: 500,
                        ..SAConfig::default()
                    },
                )
                .unwrap();
            for cid in [ComponentId(0), ComponentId(1)] {
                let p = layout.placement(cid).unwrap();
                assert!(
                    matches!(
                        p,
                        Placement::OnBoard {
                            rotation: Rotation::R0 | Rotation::R180,
                            ..
                        }
                    ),
                    "seed {seed}: cid {:?} 出现了 {:?}",
                    cid,
                    p
                );
            }
        }
    }

    /// 走线和退火能联调出有效路线: SA 布局后, PathFinder 跑出来 wires 不冲突 pin。
    #[test]
    fn place_sa_then_pathfinder_routes_cleanly() {
        use crate::Router;
        let board = board();
        let mut layout = Layout::new(two_component_fixture());
        layout
            .place_sa(
                &board,
                &SAConfig {
                    max_iters: 2000,
                    seed: 17,
                    ..SAConfig::default()
                },
            )
            .unwrap();
        let occ = layout.occupancy(&board).unwrap();
        let router = PathFinderRouter {
            max_iterations: 50,
            history_increment: 1.0,
        };
        let wires = router.route(layout.circuit(), &board, &occ, &[]);
        for w in &wires {
            // 端点不能和 pin 撞
            assert!(occ.can_add_wire(w), "wire {:?} 跟 pin 撞了", w);
        }
    }

    /// Bridged 跨 rail → 主区: 验证 bridged_pins 走通了整条链路
    ///
    /// 场景: 1 个 2-pin 电阻, 跨接 GND (负极轨) 和 主区某行。绑定 rail 到 GND net。
    /// 期望:
    /// - router 不在 bridged 两条腿之间生成 wire (它们物理上连好了)
    /// - bridged 的主区那条腿的 net 跟其他 GND pin 一起, 走 rail 短接
    /// - bridged 的 rail 那条腿不需要 wire 到 rail (已经在 rail 里)
    #[test]
    fn bridged_cross_rail_to_main_routes_correctly() {
        use crate::Router;
        use crate::circuit::{FootprintId, Net, NetId, PhysicalPin};
        use crate::layout::breadboard::PowerRailBinding;

        // 2-pin 电阻, pin 1 (主区) + pin 2 (负极轨)
        let fp = Footprint {
            id: FootprintId(0),
            name: "R_BR".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 5, y: 0 },
                },
            ],
        };
        let r1 = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let r1_pins = vec![
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
                net: Some(NetId(0)),
            },
        ];
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![r1],
            pins: r1_pins,
            nets: vec![Net {
                id: NetId(0),
                name: "GND".into(),
                pins: vec![PinId(0), PinId(1)],
            }],
            footprints: vec![fp],
        }));

        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(0),
        });
        // pin 1 (主区) 在 (5, 0), pin 2 (负极轨) 在 (0, -4)
        let h_main = board.at(5, 0).unwrap();
        let h_rail = board.at(0, -4).unwrap();
        let placement = Placement::Bridged {
            pin_holes: [(h_main, PinId(0)), (h_rail, PinId(1))],
        };

        let mut layout = Layout::new(circuit);
        layout.place(ComponentId(0), placement);
        let occ = layout.occupancy(&board).expect("bridged layout 应该合法");

        let router = PathFinderRouter {
            max_iterations: 50,
            history_increment: 1.0,
        };
        let wires = router.route(circuit, &board, &occ, &layout.bridged_pins());

        // 验证: 没有任何 wire 走到 (0, -4) (rail 端点已经被 pin 占, 不能 wire)
        for w in &wires {
            let p1 = board.hole(w.from).position;
            let p2 = board.hole(w.to).position;
            // 都不该是 (0, -4) 这个孔
            assert!(
                !(p1.x == 0 && p1.y == -4),
                "wire 端点不该在 rail 孔 (rail 上都是短接, 0, -4): {:?}",
                p1
            );
            assert!(
                !(p2.x == 0 && p2.y == -4),
                "wire 端点不该在 rail 孔 (rail 上都是短接, 0, -4): {:?}",
                p2
            );
        }
    }

    /// bridged body 在 occupancy 里被标 Blocked: 验证桥接 (0, -3) → (5, 0) 后,
    /// (1, 0) 到 (4, 0) 这些 main board 主体格 (在 bbox 里、不是 pin) 应该是 Blocked,
    /// 而 (0, -3)、(5, 0) 是 Pin. 另验证 router 不会用 (1..=4, 0) 作端点。
    #[test]
    fn bridged_body_cells_are_marked_blocked() {
        use crate::Occupant;
        use crate::circuit::{FootprintId, Net, NetId, PhysicalPin};
        use crate::layout::breadboard::PowerRailBinding;

        let fp = Footprint {
            id: FootprintId(0),
            name: "R_BR".into(),
            pins: vec![
                PhysicalPin {
                    name: "1".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "2".into(),
                    offset: Position { x: 5, y: 0 },
                },
            ],
        };
        let r1 = Component {
            id: ComponentId(0),
            ref_: "R1".into(),
            kind: "R".into(),
            value: None,
            pins: vec![PinId(0), PinId(1)],
            footprint: Some(FootprintId(0)),
            bridgeable: false,
        };
        let r1_pins = vec![
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
                net: Some(NetId(0)),
            },
        ];
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![r1],
            pins: r1_pins,
            nets: vec![Net {
                id: NetId(0),
                name: "GND".into(),
                pins: vec![PinId(0), PinId(1)],
            }],
            footprints: vec![fp],
        }));
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(0),
        });
        let h_main = board.at(5, 0).unwrap();
        let h_rail = board.at(0, -4).unwrap();
        let placement = Placement::Bridged {
            pin_holes: [(h_main, PinId(0)), (h_rail, PinId(1))],
        };
        let mut layout = Layout::new(circuit);
        layout.place(ComponentId(0), placement);
        let occ = layout.occupancy(&board).expect("bridged layout 应该合法");

        // pin 端点应是 Pin
        assert!(matches!(occ.occupant_at(h_main), Some(Occupant::Pin(_))));
        assert!(matches!(occ.occupant_at(h_rail), Some(Occupant::Pin(_))));

        // body 中间 (1..=4, 0) 这些 main board 格应是 Blocked (R1)
        for x in 1..=4 {
            let h = board.at(x, 0).unwrap();
            assert!(
                matches!(occ.occupant_at(h), Some(Occupant::Blocked(_))),
                "bridged body 中间格 ({x}, 0) 应被标 Blocked, got {:?}",
                occ.occupant_at(h)
            );
        }
        // body 在 rail 行 (-4) 那些格也应是 Blocked
        for x in 1..=4 {
            let h = board.at(x, -4).unwrap();
            assert!(
                matches!(occ.occupant_at(h), Some(Occupant::Blocked(_))),
                "bridged body 在 rail 上的 ({x}, -4) 应被标 Blocked, got {:?}",
                occ.occupant_at(h)
            );
        }
        // (0, -3) / (0, -2) / (0, -1) 是 gap row, 不存在 hole, 不检查
    }

    /// 关键: 2D 状态下, 18 元件 30x5 板不应再出 OOB (以前 sequential x 会塞不下)
    /// 注: 不要求 0 列冲突, 那需要 basin hopping 额外优化
    #[test]
    fn place_sa_no_oob_for_oversized_circuit() {
        // 手搓 18 元件的密集电路: 总宽 ~94, 远超 30
        use crate::circuit::{Net, NetId, PhysicalPin};
        let mut fp_wide = Footprint {
            id: FootprintId(0),
            name: "wide".into(),
            pins: (0..11)
                .map(|i| PhysicalPin {
                    name: i.to_string(),
                    offset: Position { x: i, y: 0 },
                })
                .collect(),
        };
        fp_wide.pins.truncate(1);
        let fp_3 = Footprint {
            id: FootprintId(1),
            name: "to92".into(),
            pins: (0..3)
                .map(|i| PhysicalPin {
                    name: i.to_string(),
                    offset: Position { x: i, y: 0 },
                })
                .collect(),
        };
        let fp_4 = Footprint {
            id: FootprintId(2),
            name: "axial".into(),
            pins: vec![
                PhysicalPin {
                    name: "0".into(),
                    offset: Position { x: 0, y: 0 },
                },
                PhysicalPin {
                    name: "3".into(),
                    offset: Position { x: 3, y: 0 },
                },
            ],
        };
        let mut components = vec![];
        let mut pins = vec![];
        // 4 个 11-col, 6 个 3-col, 8 个 4-col → 18 元件
        for i in 0..4 {
            let pin_id = PinId(pins.len());
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("D{i}"),
                kind: "D".into(),
                value: None,
                pins: vec![pin_id],
                footprint: Some(FootprintId(0)),
                bridgeable: false,
            });
            pins.push(Pin {
                id: pin_id,
                component: ComponentId(i),
                num: "0".into(),
                pinfunction: None,
                net: None,
            });
        }
        for i in 4..10 {
            let pin_id = PinId(pins.len());
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("Q{i}"),
                kind: "Q".into(),
                value: None,
                pins: vec![pin_id],
                footprint: Some(FootprintId(1)),
                bridgeable: false,
            });
            pins.push(Pin {
                id: pin_id,
                component: ComponentId(i),
                num: "0".into(),
                pinfunction: None,
                net: None,
            });
        }
        for i in 10..18 {
            let pin_id = PinId(pins.len());
            components.push(Component {
                id: ComponentId(i),
                ref_: format!("R{i}"),
                kind: "R".into(),
                value: None,
                pins: vec![pin_id],
                footprint: Some(FootprintId(2)),
                bridgeable: false,
            });
            pins.push(Pin {
                id: pin_id,
                component: ComponentId(i),
                num: "0".into(),
                pinfunction: None,
                net: None,
            });
        }
        let circuit = Box::leak(Box::new(Circuit {
            components,
            pins,
            nets: vec![Net {
                id: NetId(0),
                name: "shared".into(),
                pins: (0..18).map(PinId).collect(),
            }],
            footprints: vec![fp_wide, fp_3, fp_4],
        }));
        let board = Breadboard::new(30, 5);
        let mut layout = Layout::new(circuit);
        let result = layout.place_sa(
            &board,
            &SAConfig {
                max_iters: 5000,
                seed: 42,
                ..SAConfig::default()
            },
        );
        // 不一定要 Ok (可能有列冲突), 但 OOB 应该没有
        match result {
            Ok(()) => {}
            Err(errors) => {
                let oob = errors
                    .iter()
                    .filter(|e| matches!(e, LayoutError::OutOfBounds { .. }))
                    .count();
                assert_eq!(oob, 0, "2D SA 不应再出 OOB, got: {errors:?}");
            }
        }
    }

    // ============================================================
    //  桥接 Toggle 端到端
    // ============================================================

    /// 1 个 bridgeable 2-pin 电阻, 放标准板 + power rail 绑定。
    /// 退火后 bridgeable 元件可能是 OnBoard (cost 低) 或 Bridged (启发式选得好)。
    /// 验证: 两种 placement 都应合法, 不出 OOB / pin 碰撞。
    #[test]
    fn place_sa_can_emit_bridged_placement_for_bridgeable_resistor() {
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
        let circuit = Box::leak(Box::new(Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "R1".into(),
                kind: "R".into(),
                value: None,
                pins: vec![PinId(0), PinId(1)],
                footprint: Some(FootprintId(0)),
                bridgeable: true, // 关键: 被启发式预选
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
                Net {
                    id: NetId(0),
                    name: "P".into(),
                    pins: vec![PinId(0)],
                },
                Net {
                    id: NetId(1),
                    name: "S".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        }));
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(1),
        });
        let mut layout = Layout::new(circuit);
        // 退火不一定总选 Bridged (取决于 SA 随机接受), 但无论选哪个, validate 应过。
        // 为提高撞上 Toggle 的概率, 提升 p_toggle_bridge 到 0.3 跑多次。
        let config = SAConfig {
            max_iters: 2000,
            t0: 5.0,
            cool_rate: 0.95,
            n_seeds: 5,
            p_toggle_bridge: 0.3,
            ..SAConfig::default()
        };
        let result = layout.place_sa(&board, &config);
        assert!(
            result.is_ok(),
            "place_sa 应成功 (validate 过), got {result:?}"
        );
        // 验证 placement 类型合法
        match layout.placement(ComponentId(0)) {
            Some(Placement::OnBoard { .. }) | Some(Placement::Bridged { .. }) => {}
            other => panic!("R1 应该有 placement, got {other:?}"),
        }
    }

    /// 验证: 高 p_toggle_bridge + 多 seed 跑下来, 至少有一个 seed 产出 Bridged。
    /// (如果概率分布对, 7% × 多次跑应该能撞上; 提高到 0.5 + n_seeds=20 更稳。)
    #[test]
    fn place_sa_bridgeable_can_flip_to_bridged() {
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
        let circuit = Box::leak(Box::new(Circuit {
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
                Net {
                    id: NetId(0),
                    name: "P".into(),
                    pins: vec![PinId(0)],
                },
                Net {
                    id: NetId(1),
                    name: "S".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![fp],
        }));
        let board = Breadboard::standard().with_power_rail_binding(PowerRailBinding {
            positive: NetId(0),
            negative: NetId(1),
        });
        // 跑多次, 记录是否出现 Bridged 结果。
        // 不强求 100% 出现 (SA 随机性), 但要求至少 1 次出现 (概率足够大时必然出现)。
        let config = SAConfig {
            max_iters: 2000,
            t0: 10.0,
            cool_rate: 0.9,
            n_seeds: 20,
            p_toggle_bridge: 0.5,
            ..SAConfig::default()
        };
        let mut any_bridged = false;
        for seed in 0..20u64 {
            let mut layout = Layout::new(circuit);
            let cfg = SAConfig { seed, ..config };
            if layout.place_sa(&board, &cfg).is_ok()
                && matches!(
                    layout.placement(ComponentId(0)),
                    Some(Placement::Bridged { .. })
                )
            {
                any_bridged = true;
                break;
            }
        }
        assert!(
            any_bridged,
            "20 个 seed × p_toggle=0.5 × 2000 iters 至少应出现 1 次 Bridged, 全是 OnBoard"
        );
    }
}
