//! 模拟退火布局: 显式 `(x, y, rotation)` 状态。
//!
//! 流程: [`simulate`] → [`compact`] → 写回 `Layout.placements` → `validate`。
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
//! Rng: 自己写的 [`Lcg`] (SplitMix64), 不引外部依赖。

use std::collections::HashSet;

use crate::circuit::{Circuit, ComponentId};
use crate::layout::breadboard::Breadboard;
use crate::layout::cost::{FDConfig, SAState, Weights, cost};
use crate::layout::placement::{Rotation, rotate};

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
            use_force_directed: false,
            fd_config: FDConfig::default(),
        }
    }
}

// ============================================================
//  随机数 (SplitMix64)
// ============================================================

/// 轻量 PRNG, 不用引外部依赖。统计性质够 SA 用, **不**是密码学安全。
pub(super) struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        // 0 是 SplitMix64 的不动点, 给个最低保证
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[lo, hi)` 上的均匀整数; `hi <= lo` 时返回 `lo`。
    pub fn gen_range(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() as usize) % (hi - lo)
    }

    /// `[0, 1)` 上的均匀浮点。
    pub fn gen_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// 以概率 `p` 返回 true; `p` 被夹到 `[0, 1]`。
    pub fn gen_bool_p(&mut self, p: f64) -> bool {
        self.gen_unit() < p.clamp(0.0, 1.0)
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

fn random_move(state: &SAState, rng: &mut Lcg, board: &Breadboard) -> Move {
    let n = state.n();
    if n == 0 {
        return Move::Flip(0);
    }
    let p = rng.gen_range(0, n);
    let r = rng.gen_unit();
    let dx = if rng.gen_bool_p(0.5) { -1 } else { 1 };
    let dy = if rng.gen_bool_p(0.5) { -1 } else { 1 };

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
            rng.gen_range(0, board.cols()) as i32,
            rng.gen_range(0, board.rows()) as i32,
        );
    }

    if r < 0.30 {
        let q = loop {
            let q = rng.gen_range(0, n);
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
            rng.gen_range(0, board.cols()) as i32,
            rng.gen_range(0, board.rows()) as i32,
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
    let mut rng = Lcg::new(config.seed);
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
        let new_cost = cost(&candidate, circuit, board, &config.weights);
        let delta = new_cost - current_cost;

        let accept = delta <= 0.0 || rng.gen_unit() < (-delta / t).exp();
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

// ============================================================
//  退火后的位置压缩
// ============================================================

/// 把 SA 结果里每个元件的 x 推到"再左就会撞 pin"为止, y / rotation 保持不变。
///
/// **不**扫整行找缝, **不**考虑列短路——SA 的 cost penalty 已经管了。
/// 如果 SA 找到了 0 冲突的布局, compact 应当是几乎无操作 (只把可以左推的推一下)。
pub(super) fn compact(state: &SAState, circuit: &Circuit, board: &Breadboard) -> Vec<i32> {
    let n = state.n();
    let mut new_x = vec![0i32; n];
    let mut occupied: HashSet<(i32, i32)> = HashSet::new();

    for idx in 0..n {
        let comp_id = state.placeable[idx];
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.expect("placeable 必有 footprint");
        let footprint = &circuit.footprints[fid.0];
        let rotation = state.rotation[idx];
        let row_y = state.y[idx];

        let pin_offsets: Vec<(i32, i32)> = footprint
            .pins()
            .iter()
            .map(|p| {
                let r = rotate(p.offset, rotation);
                (r.x, r.y)
            })
            .collect();

        let board_min_x = pin_offsets.iter().map(|&(rdx, _)| -rdx).max().unwrap_or(0);
        let board_max_x =
            board.cols() as i32 - 1 - pin_offsets.iter().map(|&(rdx, _)| rdx).max().unwrap_or(0);

        let mut cur = board_min_x;
        loop {
            if cur > board_max_x {
                break;
            }
            let collides = pin_offsets
                .iter()
                .any(|&(rdx, rdy)| occupied.contains(&(cur + rdx, row_y + rdy)));
            if !collides {
                new_x[idx] = cur;
                break;
            }
            cur += 1;
        }
        if cur > board_max_x {
            new_x[idx] = cur; // 装不下, 让 validate 报越界
        }

        for &(rdx, rdy) in &pin_offsets {
            occupied.insert((new_x[idx] + rdx, row_y + rdy));
        }
    }

    new_x
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
    fn lcg_deterministic() {
        let mut a = Lcg::new(42);
        let mut b = Lcg::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn lcg_different_seeds_differ() {
        let mut a = Lcg::new(1);
        let mut b = Lcg::new(2);
        let mut same = 0;
        for _ in 0..100 {
            if a.next_u64() == b.next_u64() {
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
        let mut rng = Lcg::new(0);
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

    #[test]
    fn compact_pushes_left() {
        // 2 个 2-col footprint 在同一行, compact 推 C0 到 x=0, C1 紧接着放 x=2 (不强制 1-col gap)
        let circuit = simple_circuit();
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2, &[2, 2]);
        let xs = compact(&state, &circuit, &board());
        assert_eq!(xs, vec![0, 2]);
    }

    #[test]
    fn compact_respects_existing_y() {
        // 两个 footprint 在不同 row, 第一个在 row 1, 第二个在 row 3
        // compact 应保持 y 不变, C0 推到 x=0, C1 因为不同 row 也可以压到 x=0
        let circuit = simple_circuit();
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![5, 10],
            y: vec![1, 3],
            rotation: vec![Rotation::R0; 2],
        };
        let xs = compact(&state, &circuit, &board());
        assert_eq!(xs, vec![0, 0]);
    }

    #[test]
    fn compact_avoids_pin_collision_same_row() {
        // 两个 footprint 强制都 row 0, 但 x 已有重叠
        // compact 应当把第二个推到不撞为止
        let circuit = simple_circuit();
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            x: vec![0, 1], // 都会 pin 撞
            y: vec![0, 0],
            rotation: vec![Rotation::R0; 2],
        };
        let xs = compact(&state, &circuit, &board());
        assert_eq!(xs[0], 0);
        // C1 footprint 2 cols 宽, 必须 >= 2 才能避开 C0
        assert!(xs[1] >= 2, "应避开 pin, 期望 >= 2, got {}", xs[1]);
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
        let xs = compact(&best, &circuit, &board());
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
