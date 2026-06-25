//! 模拟退火布局: 顺序 + 旋转 + 行的扰动。
//!
//! 流程: [`simulate`] → [`compact`] → 写回 `Layout.placements` → `validate`。
//!
//! 扰动集 (概率):
//! - 50% `Swap`  —— 交换两个元件的左右顺序
//! - 25% `Flip`  —— 翻转单个元件的方向 (R0 ↔ R180)
//! - 20% `ShiftY` —— 单个元件上下微调 ±1 行
//! -  5% `Teleport` —— 单个元件跳到任意其他行 (5 行板上用得着, 解决"前排满后排空"僵局)
//!
//! **不**用 R90/R270 (会改变 footprint 的水平宽度, 破坏"一维顺序"假设)。
//!
//! Rng: 自己写的 [`Lcg`] (SplitMix64), 不引外部依赖。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, ComponentId};
use crate::layout::breadboard::Breadboard;
use crate::layout::cost::{SAState, Weights, cost};
use crate::layout::placement::{Rotation, rotate};

/// SA 总配置。`Default` 给出 5 元件级别的合理起点。
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
}

impl Default for SAConfig {
    fn default() -> Self {
        Self {
            max_iters: 5000,
            t0: 10.0,
            cool_rate: 0.95,
            weights: Weights::default(),
            seed: 0xCAFE_F00D,
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
    /// 交换 order[i] 和 order[j]
    Swap(usize, usize),
    /// 翻转 order[i] 的旋转 (R0 ↔ R180)
    Flip(usize),
    /// order[i] 的 row 加上 dy (±1)
    ShiftY(usize, i32),
    /// order[i] 跳到任意行
    Teleport(usize, i32),
}

fn random_move(state: &SAState, rng: &mut Lcg, board: &Breadboard) -> Move {
    let n = state.n();
    if n == 0 {
        // 没有元件时不该被调用, 但返回个无害的
        return Move::Flip(0);
    }
    let p = rng.gen_range(0, n);
    let r = rng.gen_unit();

    if n < 2 {
        // 没法 swap, 三个动作平分概率
        if r < 1.0 / 3.0 {
            return Move::Flip(p);
        }
        if r < 2.0 / 3.0 {
            return Move::ShiftY(p, if rng.gen_bool_p(0.5) { -1 } else { 1 });
        }
        return Move::Teleport(p, rng.gen_range(0, board.rows()) as i32);
    }

    if r < 0.50 {
        // Swap; 保证 q != p
        let q = loop {
            let q = rng.gen_range(0, n);
            if q != p {
                break q;
            }
        };
        Move::Swap(p, q)
    } else if r < 0.75 {
        Move::Flip(p)
    } else if r < 0.95 {
        Move::ShiftY(p, if rng.gen_bool_p(0.5) { -1 } else { 1 })
    } else {
        Move::Teleport(p, rng.gen_range(0, board.rows()) as i32)
    }
}

fn apply_move(state: &mut SAState, m: Move) {
    match m {
        Move::Swap(i, j) => state.placeable.swap(i, j),
        Move::Flip(i) => {
            state.rotation[i] = match state.rotation[i] {
                Rotation::R0 => Rotation::R180,
                Rotation::R180 => Rotation::R0,
                other => panic!("SA 只用 R0/R180, 不该出现 {:?}", other),
            };
        }
        Move::ShiftY(i, dy) => state.row[i] += dy,
        Move::Teleport(i, r) => state.row[i] = r,
    }
}

// ============================================================
//  SA 主循环
// ============================================================

/// 跑模拟退火, 返回最佳 [`SAState`]。
pub(super) fn simulate(
    placeable: Vec<ComponentId>,
    circuit: &Circuit,
    board: &Breadboard,
    config: &SAConfig,
) -> SAState {
    let mut rng = Lcg::new(config.seed);
    // 初始: 元件按输入顺序, 全部 R0, 全部同一行
    let mut state = SAState::from_order(placeable, 2);
    let mut current_cost = cost(&state, circuit, board, &config.weights);
    let mut best_state = state.clone();
    let mut best_cost = current_cost;
    let mut t = config.t0;

    for _ in 0..config.max_iters {
        let m = random_move(&state, &mut rng, board);
        // 复制一份做实验, 失败就丢弃
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

/// 把 SA 结果里每个元件的 x 推到"再左就会撞"为止。
///
/// **规则**: v1 只用 R0/R180, footprint 不跨行, 所以"同行上, 下一个元件的最左 pin
/// 必须比上一个最右 pin 远 ≥ 1 列" 就够。**不**扫整行找缝——留 TOD O, 收益小。
pub(super) fn compact(state: &SAState, circuit: &Circuit, _board: &Breadboard) -> Vec<i32> {
    let n = state.n();
    let mut x_positions = vec![0i32; n];
    // 同行 row 上, 下一个元件 "最左 pin col" 的下限
    // (初始 0, 每放一个元件就更新成"其最右 pin col + 1 + 1 间隙")
    let mut next_leftmost: HashMap<i32, i32> = HashMap::new();

    for idx in 0..n {
        let comp_id = state.placeable[idx];
        let component = &circuit.components[comp_id.0];
        let fid = component.footprint.expect("placeable 必有 footprint");
        let footprint = &circuit.footprints[fid.0];
        let rotation = state.rotation[idx];
        let row_y = state.row[idx];

        let pin_offsets: Vec<(i32, i32)> = footprint
            .pins()
            .iter()
            .map(|p| {
                let r = rotate(p.offset, rotation);
                (r.x, r.y)
            })
            .collect();

        let min_rdx = pin_offsets.iter().map(|&(rdx, _)| rdx).min().unwrap_or(0);
        let max_rdx = pin_offsets.iter().map(|&(rdx, _)| rdx).max().unwrap_or(0);

        // 板内约束: x + min(rdx) >= 0
        let board_min = -min_rdx;
        // 同行约束: x + min(rdx) >= next_leftmost[row]
        let row_min = next_leftmost.get(&row_y).copied().unwrap_or(0) - min_rdx;
        let x = board_min.max(row_min);

        x_positions[idx] = x;
        // 下一个元件最左 pin col 下限 = 当前最右 pin col + 1 (贴邻) + 1 (gap)
        next_leftmost.insert(row_y, x + max_rdx + 2);
    }
    x_positions
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
    use crate::layout::cost::{derive_x, footprint_horizontal_width};

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
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2);
        let mut rng = Lcg::new(0);
        for _ in 0..100 {
            let m = random_move(&state, &mut rng, &board());
            match m {
                Move::Swap(i, j) => {
                    assert!(i < state.n() && j < state.n());
                    assert_ne!(i, j);
                }
                Move::Flip(i) | Move::ShiftY(i, _) | Move::Teleport(i, _) => {
                    assert!(i < state.n());
                }
            }
        }
    }

    #[test]
    fn apply_swap_changes_order() {
        let mut state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2);
        apply_move(&mut state, Move::Swap(0, 1));
        assert_eq!(state.placeable, vec![ComponentId(1), ComponentId(0)]);
    }

    #[test]
    fn apply_flip_toggles_rotation() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2);
        apply_move(&mut state, Move::Flip(0));
        assert_eq!(state.rotation[0], Rotation::R180);
        apply_move(&mut state, Move::Flip(0));
        assert_eq!(state.rotation[0], Rotation::R0);
    }

    #[test]
    fn apply_shift_y_increments_row() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2);
        apply_move(&mut state, Move::ShiftY(0, 1));
        assert_eq!(state.row[0], 3);
        apply_move(&mut state, Move::ShiftY(0, -2));
        assert_eq!(state.row[0], 1);
    }

    #[test]
    fn apply_teleport_sets_row() {
        let mut state = SAState::from_order(vec![ComponentId(0)], 2);
        apply_move(&mut state, Move::Teleport(0, 4));
        assert_eq!(state.row[0], 4);
    }

    #[test]
    fn derive_x_includes_gaps() {
        let circuit = simple_circuit(); // 每个 footprint 2 列宽
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2);
        let x = derive_x(&state, &circuit);
        // C0 @ x=0, C1 @ x=0+2+1=3
        assert_eq!(x, vec![0, 3]);
    }

    #[test]
    fn footprint_width_simple() {
        let circuit = simple_circuit();
        let fp = &circuit.footprints[0];
        assert_eq!(footprint_horizontal_width(fp), 2);
    }

    #[test]
    fn compact_pushes_left() {
        // 2 个 footprint 2 列宽, 中间隔 1 列 → 期望 (0, 3)
        let circuit = simple_circuit();
        let state = SAState::from_order(vec![ComponentId(0), ComponentId(1)], 2);
        let xs = compact(&state, &circuit, &board());
        assert_eq!(xs, vec![0, 3]);
    }

    #[test]
    fn compact_avoids_overlap() {
        // 2 个 footprint 2 列宽, 挤到 row 不同, 但要保证 x 不撞
        let circuit = simple_circuit();
        // 强制让两个元件 "争夺" 同一个最左位置
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            rotation: vec![Rotation::R0; 2],
            row: vec![0, 0],
        };
        let xs = compact(&state, &circuit, &board());
        // C0 占了 (0,0) (1,0); C1 必须 >= 3
        assert_eq!(xs[0], 0);
        assert!(xs[1] >= 3, "expected >= 3, got {}", xs[1]);
    }

    #[test]
    fn compact_squeeze_through_gap() {
        // 2 个 1-pin footprint 放不同行, 应该可以挤到同一列
        let fp = Footprint {
            id: FootprintId(0),
            name: "single".into(),
            pins: vec![PhysicalPin {
                name: "1".into(),
                offset: Position { x: 0, y: 0 },
            }],
        };
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
        // 两个不同行, 期望都能在 x=0
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            rotation: vec![Rotation::R0; 2],
            row: vec![0, 1],
        };
        let xs = compact(&state, &circuit, &board());
        assert_eq!(xs, vec![0, 0]);
    }

    #[test]
    fn sa_converges_below_initial() {
        let circuit = simple_circuit();
        // 故意构造一个较差初始: 两元件都在不同行 → 退火应能找到 (0, 3) 共享行 2
        let state = SAState {
            placeable: vec![ComponentId(0), ComponentId(1)],
            rotation: vec![Rotation::R0; 2],
            row: vec![0, 4],
        };
        let initial_cost = cost(&state, &circuit, &board(), &Weights::default());
        let config = SAConfig {
            max_iters: 2000,
            t0: 5.0,
            cool_rate: 0.95,
            weights: Weights::default(),
            seed: 1,
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
        // 共享 net 的两个元件最终应在同一行 (让 HPWL = 0 或很小)
        assert_eq!(best.row[0], best.row[1], "应合并到同一行");
    }

    #[test]
    fn sa_finds_valid_layout_on_simple_circuit() {
        // 退火 + 压缩之后, validate 应过 (无 pin 碰撞)
        let circuit = simple_circuit();
        let config = SAConfig {
            max_iters: 1000,
            seed: 7,
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
                let hole = (xs[idx] + r.x, best.row[idx] + r.y);
                assert!(holes.insert(hole), "pin 撞了: {:?}", hole);
            }
        }
    }

    // 暴露给外层 mod.rs 的测试用
}
