//! 模拟退火布局: 显式 `(x, y, rotation)` 状态。
//!
//! 流程: [`simulate`] → 写回 `Layout.placements` → `validate`。
//! 紧凑度已折进 [`cost::cost`], SA 一次跑完搞定。
//!
//! 扰动集 (v2, 概率):
//! - 30% `Swap`     —— 交换两个元件的 `(x, y, rotation)` 三元组
//! - 15% `Flip`     —— 翻转单个元件的方向 (R0 ↔ R180)
//! - 20% `ShiftX`   —— 单个元件左右微调 ±1 col
//! - 20% `ShiftY`   —— 单个元件上下微调 ±1 row
//! - 15% `Teleport` —— 单个元件跳到任意合法 `(x, y)`
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
    /// 交换两个元件的 (x, y, rotation) 三元组
    Swap(usize, usize),
    /// 翻转单个元件的旋转 (R0 ↔ R180)
    Flip(usize),
    /// 单个元件 x 增 ±1
    ShiftX(usize, i32),
    /// 单个元件 y 增 ±1
    ShiftY(usize, i32),
    /// 单个元件跳到 (x, y)
    Teleport(usize, i32, i32),
}

fn random_move(state: &SAState, rng: &mut fastrand::Rng, board: &Breadboard) -> Move {
    let n = state.n();
    if n == 0 {
        return Move::Flip(0);
    }
    let p = rng.usize(0..n);
    let r = rng.f64();
    let dx = if rng.f64() < 0.5 { -1 } else { 1 };
    let dy = if rng.f64() < 0.5 { -1 } else { 1 };

    if n < 2 {
        if r < 0.20 {
            return Move::Flip(p);
        }
        if r < 0.40 {
            return Move::ShiftX(p, dx);
        }
        if r < 0.60 {
            return Move::ShiftY(p, dy);
        }
        return Move::Teleport(
            p,
            rng.usize(0..board.cols()) as i32,
            rng.usize(0..board.rows()) as i32,
        );
    }

    if r < 0.30 {
        let q = loop {
            let q = rng.usize(0..n);
            if q != p {
                break q;
            }
        };
        Move::Swap(p, q)
    } else if r < 0.45 {
        Move::Flip(p)
    } else if r < 0.65 {
        Move::ShiftX(p, dx)
    } else if r < 0.85 {
        Move::ShiftY(p, dy)
    } else {
        Move::Teleport(
            p,
            rng.usize(0..board.cols()) as i32,
            rng.usize(0..board.rows()) as i32,
        )
    }
}

fn apply_move(state: &mut SAState, m: Move) {
    match m {
        Move::Swap(i, j) => {
            state.placeable.swap(i, j);
            state.x.swap(i, j);
            state.y.swap(i, j);
            state.rotation.swap(i, j);
        }
        Move::Flip(i) => {
            state.rotation[i] = match state.rotation[i] {
                Rotation::R0 => Rotation::R180,
                Rotation::R180 => Rotation::R0,
                other => panic!("SA 只用 R0/R180, 不该出现 {:?}", other),
            };
        }
        Move::ShiftX(i, dx) => state.x[i] += dx,
        Move::ShiftY(i, dy) => state.y[i] += dy,
        Move::Teleport(i, x, y) => {
            state.x[i] = x;
            state.y[i] = y;
        }
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
        let m = random_move(&state, &mut rng, board);
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
        for _ in 0..200 {
            let m = random_move(&state, &mut rng, &board());
            match m {
                Move::Swap(i, j) => {
                    assert!(i < state.n() && j < state.n());
                    assert_ne!(i, j);
                }
                Move::Flip(i)
                | Move::ShiftX(i, _)
                | Move::ShiftY(i, _)
                | Move::Teleport(i, _, _) => {
                    assert!(i < state.n());
                }
            }
        }
    }

    #[test]
    fn apply_swap_swaps_all_fields() {
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        state.x = vec![5, 10];
        state.y = vec![0, 3];
        state.rotation = vec![Rotation::R0, Rotation::R180];
        apply_move(&mut state, Move::Swap(0, 1));
        assert_eq!(state.placeable, vec![ComponentId(1), ComponentId(0)]);
        assert_eq!(state.x, vec![10, 5]);
        assert_eq!(state.y, vec![3, 0]);
        assert_eq!(state.rotation, vec![Rotation::R180, Rotation::R0]);
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

    #[test]
    fn apply_teleport_sets_position() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2, &[1]);
        apply_move(&mut state, Move::Teleport(0, 7, 3));
        assert_eq!(state.x[0], 7);
        assert_eq!(state.y[0], 3);
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
