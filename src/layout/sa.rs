//! 模拟退火布局: 显式 `(x, y, rotation)` 状态。
//!
//! 流程: [`simulate`] → 写回 `Layout.placements` → `validate`。
//! 紧凑度已折进 [`cost::cost`], SA 一次跑完搞定。
//!
//! 扰动集 (v3, 概率):
//! - 60% `ShiftX` —— 单个元件左右微调; 高温期幅度可达 ±3 col, 低温退到 ±1
//! - 30% `Flip`   —— 翻转单个元件的方向 (R0 ↔ R180)
//! - 10% `ShiftY` —— 单个元件上下微调; 高温期幅度可达 ±3 row, 低温退到 ±1
//!
//! v3 相对 v2 的关键变化:
//! - 删 `Swap` / `Teleport`: FD 初排已经给好全局形状, SA 阶段不需要远距离重排;
//!   远距离重排靠高温期 `ShiftX` 的大步长 (`N=2 or 3`) 多次迭代实现。
//! - `ShiftX` 概率从 20% 提到 60%: 18 元件板上 2.5 cell 量级的"局部最优"
//!   (如 R2 卡在 x=1 vs 真实 x=2) 是 SA 的主战场, 需要更多采样。
//! - `ShiftY` 从 20% 降到 10%: 所有 footprint 的 y 跨度 ≤ 1, ShiftY 大部分
//!   move 对最终 layout 没贡献, 抽样成本高。
//! - `Flip` 从 15% 提到 30%: TO-92 / DO-41 等有方向性的元件, 翻转是常见
//!   "把 pin 挪到对面 rail"的手段, 概率太低会让方向修正跟不上。
//!
//! **不**用 R90/R270 (会改变 footprint 的水平宽度, 破坏"显式 2D 状态"假设)。
//!
//! Rng: [`fastrand::Rng`] (WyRand), 不密码学安全但统计性质足够 SA 用。

use crate::circuit::{Circuit, ComponentId};
use crate::layout::breadboard::Breadboard;
use crate::layout::cost::{FDConfig, SAState, Weights, cost};
use crate::layout::placement::Rotation;

// 测试用: HashSet 和 rotate 只在下面的 mod tests 里用, 放到 cfg(test) 块里避免非测试
// 构建时的 unused_imports 警告。
#[cfg(test)]
use crate::layout::placement::rotate;
#[cfg(test)]
use std::collections::HashSet;

/// SA 总配置。`Default` 给出 18 元件级别的合理起点。
#[derive(Debug, Clone, Copy)]
pub struct SAConfig {
    /// 退火总迭代数; 后期 SA 接受率接近 0, 跑也是空转。
    pub max_iters: usize,
    /// 初始温度; 经验上 T0 ≈ 3 * "典型变差 Δcost" 比较稳。
    pub t0: f64,
    /// 每步 T *= cool_rate; 0.95 通用, 0.9 更快但更糙。
    pub cool_rate: f64,
    pub weights: Weights,
    /// 决定随机扰动序列; 改 seed 可重新跑一遍出不同结果。
    pub seed: u64,
    /// 跑多少次取最低 cost 的解。SA 是随机算法, 单次可能卡在 local optimum;
    /// MST cost 下多 seed 跑出来大部分能找到 cost=0 (零跳线)。默认 1。
    pub n_seeds: usize,
    /// `true` 用 [`SAState::from_force_directed`] 做初排 (比 `from_greedy` 慢,
    /// 但对强耦合电路起点好得多); `false` 用贪心 first-fit。
    pub use_force_directed: bool,
    /// 仅在 `use_force_directed = true` 时使用。
    pub fd_config: FDConfig,
}

impl Default for SAConfig {
    fn default() -> Self {
        Self {
            max_iters: 10000,
            t0: 10.0,
            cool_rate: 0.95,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
            n_seeds: 1,
            use_force_directed: false,
            fd_config: FDConfig::default(),
        }
    }
}

// ============================================================
//  扰动
// ============================================================

#[derive(Debug, Clone, Copy)]
enum Move {
    /// 翻转单个元件的旋转 (R0 ↔ R180)
    Flip(usize),
    /// 单个元件 x 增 ±N (N 随温度: 高温 1..=3, 低温 1)
    ShiftX(usize, i32),
    /// 单个元件 y 增 ±N (N 随温度: 高温 1..=3, 低温 1)
    ShiftY(usize, i32),
}

fn random_move(state: &SAState, rng: &mut fastrand::Rng, t: f64, t0: f64) -> Move {
    let n = state.n();
    if n == 0 {
        return Move::Flip(0);
    }
    let p = rng.usize(0..n);
    let r = rng.f64();

    // 步长随温度变: 高温期 N ∈ {1, 2, 3} 均匀, 中温 {1, 2}, 低温恒为 1。
    // 三个区间按 T0 的 0.5 / 0.2 划分; 越冷越精细, 越热越敢跳。
    let max_n = if t > t0 * 0.5 {
        3
    } else if t > t0 * 0.2 {
        2
    } else {
        1
    };
    // 用 max_n=1 时 rng.usize(0..1) 仍消耗一个随机数 (始终返回 0),
    // 保持不同温度下 rng 序列的可比性 — 避免"高温多消费随机数"污染种子复现性。
    let n_amp = 1 + rng.usize(0..max_n) as i32;
    let dx_sign = if rng.f64() < 0.5 { -1 } else { 1 };
    let dy_sign = if rng.f64() < 0.5 { -1 } else { 1 };
    let dx = dx_sign * n_amp;
    let dy = dy_sign * n_amp;

    if r < 0.60 {
        Move::ShiftX(p, dx)
    } else if r < 0.90 {
        Move::Flip(p)
    } else {
        Move::ShiftY(p, dy)
    }
}

fn apply_move(state: &mut SAState, m: Move) {
    match m {
        Move::Flip(i) => {
            state.rotation[i] = match state.rotation[i] {
                Rotation::R0 => Rotation::R180,
                Rotation::R180 => Rotation::R0,
                other => panic!("SA 只用 R0/R180, 不该出现 {:?}", other),
            };
        }
        Move::ShiftX(i, dx) => state.x[i] += dx,
        Move::ShiftY(i, dy) => state.y[i] += dy,
    }
}

// ============================================================
//  SA 主循环
// ============================================================

/// 跑模拟退火, 返回最佳 [`SAState`]。
///
/// 初始状态用 [`SAState::from_greedy`]: 按行从上到下、列从左到右贪心放置,
/// 保证不 OOB、不撞 pin (但**不**保证无列短路——那是 SA 要优化的)。
pub(super) fn simulate(
    placeable: Vec<ComponentId>,
    circuit: &Circuit,
    board: &Breadboard,
    config: &SAConfig,
) -> SAState {
    let mut rng = fastrand::Rng::with_seed(config.seed);
    let mut state = if config.use_force_directed {
        SAState::from_force_directed(placeable, circuit, board, &config.fd_config)
    } else {
        SAState::from_greedy(placeable, circuit, board)
    };
    let mut current_cost = cost(&state, circuit, board, &config.weights);
    let mut best_state = state.clone();
    let mut best_cost = current_cost;
    let mut t = config.t0;

    for _ in 0..config.max_iters {
        let m = random_move(&state, &mut rng, t, config.t0);
        let mut candidate = state.clone();
        apply_move(&mut candidate, m);
        // 拒绝任何产生越界 / blocked row y 的候选 — 物理上不该考虑的状态。
        // (cost 会扣 OOB 惩罚让 SA 远离开, 但放在这里少跑 cost 计算。
        // 另外, 若初始状态本身就合法, SA 也不会被锁定到 "都是 OOB 的同代价态"。)
        if !state_y_valid(&candidate, board) {
            continue;
        }
        let new_cost = cost(&candidate, circuit, board, &config.weights);
        let delta = new_cost - current_cost;

        let accept = delta <= 0.0 || rng.f64() < (-delta / t).exp();
        if accept {
            state = candidate;
            current_cost = new_cost;
            if current_cost < best_cost {
                best_cost = current_cost;
                best_state = state.clone();
            }
        }

        t *= config.cool_rate;
        if t < 1e-6 {
            break;
        }
    }

    best_state
}

/// 检查 SAState 里所有 y 是否在板内且非 blocked row。
/// ShiftY ±1 可能把 y=0 的元件推到 y=-1 (越界), 把 y=4 的推到 y=5 (blocked row),
/// 这些候选从一开始就不该让 SA 考虑。
fn state_y_valid(state: &SAState, board: &Breadboard) -> bool {
    state
        .y
        .iter()
        .all(|&y| y >= 0 && (y as usize) < board.rows() && !board.is_blocked(y as usize))
}

// ============================================================
//  测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{
        Circuit, Component, ComponentId, Footprint, FootprintId, Net, NetId, PhysicalPin, Pin,
        PinId, Position,
    };
    use crate::layout::Breadboard;

    /// 构造一个最简电路: 2 个 2-pin 元件, pin1=net0, pin2=net0 (都连一起)
    fn simple_circuit() -> Circuit {
        let footprint = Footprint {
            id: FootprintId(0),
            name: "two".into(),
            pins: (1..=2)
                .map(|n| PhysicalPin {
                    name: n.to_string(),
                    offset: Position { x: n - 1, y: 0 },
                })
                .collect(),
        };
        let components = (0..2)
            .map(|i| Component {
                id: ComponentId(i),
                ref_: format!("C{i}"),
                kind: "X".into(),
                value: None,
                pins: vec![PinId(i * 2), PinId(i * 2 + 1)],
                footprint: Some(FootprintId(0)),
            })
            .collect();
        let pins = (0..4)
            .map(|i| Pin {
                id: PinId(i),
                component: ComponentId(i / 2),
                num: ((i % 2) + 1).to_string(),
                pinfunction: None,
                net: Some(NetId(0)),
            })
            .collect();
        let nets = vec![Net {
            id: NetId(0),
            name: "shared".into(),
            pins: (0..4).map(PinId).collect(),
        }];
        Circuit {
            components,
            pins,
            nets,
            footprints: vec![footprint],
        }
    }

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn rng_deterministic() {
        let mut a = fastrand::Rng::with_seed(42);
        let mut b = fastrand::Rng::with_seed(42);
        for _ in 0..100 {
            assert_eq!(a.u64(..), b.u64(..));
        }
    }

    #[test]
    fn rng_different_seeds_differ() {
        let mut a = fastrand::Rng::with_seed(1);
        let mut b = fastrand::Rng::with_seed(2);
        let mut same = 0;
        for _ in 0..100 {
            if a.u64(..) == b.u64(..) {
                same += 1;
            }
        }
        assert!(same < 5, "两个不同 seed 不该几乎一样: same={same}");
    }

    #[test]
    fn random_move_returns_valid_index() {
        let _circuit = simple_circuit();
        let state = SAState::from_greedy(
            vec![ComponentId(0), ComponentId(1)],
            &simple_circuit(),
            &board(),
        );
        let mut rng = fastrand::Rng::with_seed(0);
        // T0=30, T 在 [0.3, 30] 区间走过, 涵盖 max_n=3/2/1 三档。
        for k in 0..200 {
            let t = 30.0_f64 * (1.0 - k as f64 / 200.0) + 0.3;
            let m = random_move(&state, &mut rng, t, 30.0);
            match m {
                Move::Flip(i) | Move::ShiftX(i, _) | Move::ShiftY(i, _) => {
                    assert!(i < state.n(), "index {i} out of range {}", state.n());
                }
            }
        }
    }

    #[test]
    fn random_move_high_t_uses_larger_amplitude() {
        // 同一个 state, 同一个 seed, 在不同 t 下生成的 ShiftX 步长分布不同
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        let mut rng_hi = fastrand::Rng::with_seed(0);
        let mut rng_lo = fastrand::Rng::with_seed(0);
        let mut hi_max = 0i32;
        let mut lo_max = 0i32;
        for _ in 0..2000 {
            if let Move::ShiftX(_, dx) = random_move(&state, &mut rng_hi, 30.0, 30.0) {
                hi_max = hi_max.max(dx.abs());
            }
            if let Move::ShiftX(_, dx) = random_move(&state, &mut rng_lo, 0.5, 30.0) {
                lo_max = lo_max.max(dx.abs());
            }
        }
        assert!(hi_max >= 2, "T=30 应该出现 N=2 或 N=3, 最大观测 = {hi_max}");
        assert_eq!(lo_max, 1, "T=0.5 应该恒为 N=1, 最大观测 = {lo_max}");
    }

    #[test]
    fn apply_flip_toggles_rotation() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, Move::Flip(0));
        assert_eq!(state.rotation[0], Rotation::R180);
        apply_move(&mut state, Move::Flip(0));
        assert_eq!(state.rotation[0], Rotation::R0);
    }

    #[test]
    fn apply_shift_x_increments_x() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, Move::ShiftX(0, 1));
        assert_eq!(state.x[0], 1);
        apply_move(&mut state, Move::ShiftX(0, -2));
        assert_eq!(state.x[0], -1);
    }

    #[test]
    fn apply_shift_y_increments_y() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, Move::ShiftY(0, 1));
        assert_eq!(state.y[0], 3);
        apply_move(&mut state, Move::ShiftY(0, -2));
        assert_eq!(state.y[0], 1);
    }

    /// 验证 SAState::from_greedy 在标准板上不会试着把元件放中间 blocked row。
    #[test]
    fn from_greedy_avoids_blocked_rows() {
        let board = crate::layout::Breadboard::standard();
        let circuit = simple_circuit();
        let state = SAState::from_greedy(vec![ComponentId(0), ComponentId(1)], &circuit, &board);
        for &y in &state.y {
            assert!(
                !board.is_blocked(y as usize),
                "from_greedy 把元件放到了 blocked row y={y}"
            );
        }
    }

    #[test]
    fn sa_converges_below_initial() {
        let circuit = simple_circuit();
        // 故意构造差初始: C0 @ (0,0), C1 @ (3,4) — 都在不同 row 远距离
        // 共享 net, HPWL 跨度 3
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        state.x = vec![0, 3];
        state.y = vec![0, 4];
        let initial_cost = cost(&state, &circuit, &board(), &Weights::default());
        let config = SAConfig {
            max_iters: 2000,
            t0: 5.0,
            cool_rate: 0.95,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        };
        let best = simulate(
            vec![ComponentId(0), ComponentId(1)],
            &circuit,
            &board(),
            &config,
        );
        let best_cost = cost(&best, &circuit, &board(), &Weights::default());
        assert!(
            best_cost <= initial_cost,
            "SA 应该不恶化: init={initial_cost} best={best_cost}"
        );
    }

    #[test]
    fn sa_finds_valid_layout_on_simple_circuit() {
        let circuit = simple_circuit();
        let config = SAConfig {
            max_iters: 1000,
            seed: 0xCAFE_F00D,
            ..SAConfig::default()
        };
        let best = simulate(
            vec![ComponentId(0), ComponentId(1)],
            &circuit,
            &board(),
            &config,
        );
        // SA 输出本身就是 final 位置 (compact 删了, cost 里的 compactness 替代)
        let xs = best.x.clone();
        // 检查 pin 不撞
        let mut holes: HashSet<(i32, i32)> = HashSet::new();
        for idx in 0..best.n() {
            let comp = &circuit.components[best.placeable[idx].0];
            let footprint = &circuit.footprints[comp.footprint.unwrap().0];
            let rotation = best.rotation[idx];
            for &pin_id in &comp.pins {
                let pin = &circuit.pins[pin_id.0];
                let pp = footprint
                    .pins
                    .iter()
                    .find(|p| p.name() == pin.num())
                    .unwrap();
                let r = rotate(pp.offset, rotation);
                let hole = (xs[idx] + r.x, best.y[idx] + r.y);
                assert!(holes.insert(hole), "pin 撞了: {:?}", hole);
            }
        }
    }
}
