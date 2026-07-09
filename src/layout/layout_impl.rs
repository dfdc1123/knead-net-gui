//! `Layout` 的方法实现 (与 struct 定义分离, 保持 mod.rs 干净)。

use super::{LayoutError, Occupancy, SAConfig};
use crate::circuit::{Circuit, ComponentId, Footprint, Position};
use crate::layout::breadboard::{Breadboard, HoleId};
use crate::layout::placement::{Placement, Rotation};

use super::debug::{diagnose_expensive_seeds, print_seed_cost_report};
use super::routing::Wire;

impl<'c> super::Layout<'c> {
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

    /// 一次性验证整个 layout, 返回所有错误 (`Vec<LayoutError>`)。
    ///
    /// `validate` 跟 `occupancy` 走同一条检查路径, 区别是 `validate` 丢掉了
    /// 构建出来的 occupancy 表, 只关心错误。语义上"我只想问合不合法"。
    /// 可产生的错误种类见 [`LayoutError`] (7 种)。
    pub fn validate(&self, board: &Breadboard) -> Result<(), Vec<LayoutError>> {
        self.occupancy(board).map(|_| ())
    }

    /// 用模拟退火布局。
    ///
    /// 流程: 收集 **有 footprint 且尚未摆放** 的 component → `sa::simulate`
    /// (跑 `config.n_seeds` 次, 取最低 cost 的 best state) → 写回 `placements`
    /// (SA 的 `ToggleBridging` 可产出 `Placement::Bridged`, 见下方内联注释)
    /// → `validate(board)`。
    ///
    /// 已经手动摆过的 component (OnBoard 或 Bridged 都算) **不会被 SA 覆盖**,
    /// 即 SA 永远只优化未摆的。
    ///
    /// 紧凑度已折进 [`cost::cost`], 不再需要单独的 post-pass。
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
        //
        // 并行: 每个 seed 互相独立 (输入全 &T 只读, 输出是新 SAState, 局部 RNG),
        // 用 rayon 的 `par_iter` 跨核跑。n_seeds = 100 一般远超核数, 池子喂得饱。
        let n_seeds = config.n_seeds.max(1);
        let preprocess = super::preprocess::preprocess_for_breadboard(self.circuit, board);
        if !preprocess.r90_only.is_empty() {
            let names: Vec<&str> = preprocess.r90_only
                .iter()
                .map(|&cid| self.circuit.components()[cid.raw()].ref_())
                .collect();
            eprintln!("R90 预处理: {} 个元件 → {:?}", preprocess.r90_only.len(), names);
        }
        if !preprocess.y_locked.is_empty() {
            for (&cid, &y) in &preprocess.y_locked {
                eprintln!("  y-lock: {} → y={}", self.circuit.components()[cid.raw()].ref_(), y);
            }
        }
        use rayon::prelude::*;
        let results: Vec<(f64, SAState)> = (0..n_seeds as u64)
            .into_par_iter()
            .map(|s| {
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
                    &preprocess,
                );
                let cost_s = crate::layout::cost::cost(
                    &state_s,
                    self.circuit,
                    board,
                    &bridged_pins,
                    &config.weights,
                );
                (cost_s, state_s)
            })
            .collect();
        let per_seed_costs: Vec<f64> = results.iter().map(|(c, _)| *c).collect();
        let per_seed_states: Vec<SAState> = results.iter().map(|(_, s)| s.clone()).collect();
        let (best_cost, best) = results
            .into_iter()
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .expect("至少跑过一次");

        // 报告 30 个 seed 各自的 cost, 帮调试看到 SA 收敛分布。
        // main 以外不是生产路径 (如测试), 调用者不会看到这些输出。
        print_seed_cost_report(&per_seed_costs, best_cost, config.seed);
        diagnose_expensive_seeds(
            &per_seed_states,
            &per_seed_costs,
            self.circuit,
            board,
            &bridged_pins,
            &config.weights,
            config.seed,
        );

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
    /// 返回 `Result<(), Vec<LayoutError>>` 上报所有 7 种 `LayoutError`
    /// (越界 / pin 碰撞 / bbox 重叠 / wire 冲突 / 列冲突 / 无 footprint 等);
    /// 即使有错, placement 也已经写入, 调用方可以检查后调整。
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
