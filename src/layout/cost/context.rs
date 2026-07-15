//! 预计算数据 (CompInfo / SAContext / CostBuf), 避免 cost 热路径里重复查 footprint / 算 bbox。
//!
//! - `CompInfo`: 每个 placeable 元件的 footprint / pin 信息 (只依赖 circuit, 不随 SA 状态变)
//! - `SAContext`: 把 CompInfo 集合 + 外部桥接位 + 电源 anchor 打包, 一次构造
//! - `CostBuf`: SA 热循环里复用的 buffer (per-net scratch)

use super::state::SAState;
use crate::circuit::{Circuit, ComponentId, NetId, Position};
use crate::layout::breadboard::Breadboard;
use crate::layout::placement::{BBox, Rotation};

/// Rotation → 预计算偏移数组的下标。
#[inline]
pub(crate) fn rot_index(rot: Rotation) -> usize {
    match rot {
        Rotation::R0 => 0,
        Rotation::R180 => 1,
        Rotation::R90 => 2,
        Rotation::R270 => 3,
    }
}

/// 一个 bridged pin pair 的世界坐标 + rail_id + net (两个 pin 各一份)。
/// 在 `CompInfo::bridged_pair_world` 里按候选 pair 顺序存放; cost_fast 热路径
/// 直接索引, 不再查 board。
pub(super) type BridgedPair = [(i32, i32, u32, Option<NetId>); 2];

/// 每个 placeable 元件的预计算信息 (只依赖 circuit/footprint, 不随 SA 状态变)。
#[derive(Debug, Clone)]
pub struct CompInfo {
    /// 每个 pin 的预计算数据, 按 component.pins 顺序。
    /// `[R0, R180, R90, R270]` 四个旋转方向的局部偏移 + net。
    pub pins: Vec<([Position; 4], Option<NetId>)>,
    /// 该元件 footprint 在 R0 旋转下的局部坐标 bbox
    pub bbox_r0: BBox,
    /// 仅 bridgeable 元件有: 每个候选 pin pair 对应的 bbox
    /// (board 孔位固定, 这些 bbox 是常量, 不随 SA 状态变)。
    /// None = 非 bridgeable。
    pub bridged_bboxes: Option<Vec<BBox>>,
    /// 仅 bridgeable 元件有: 每个候选 pin pair 对应的世界坐标 (x, y, rail_id, net) × 2。
    /// cost_fast 热路径里不需再查 board。
    pub bridged_pair_world: Option<Vec<BridgedPair>>,
}

/// SA 上下文: 预计算数据 + reusable buffers。
/// 在 simulate() 入口构造一次, 所有 cost 调用复用。
pub struct SAContext {
    pub comp_infos: Vec<CompInfo>,
    /// 用户预摆 bridged_pins 的世界坐标 + rail_id + net (常量)
    pub external_bridged_world: Vec<(i32, i32, u32, Option<NetId>)>,
    /// 电源轨 anchor (negative, positive) 的世界坐标 + rail_id
    pub power_anchor_world: Vec<(i32, i32, u32)>,
    /// 电源轨 anchor 的 net ids (顺序与 power_anchor_world 对应)
    pub power_anchor_nets: Vec<Option<NetId>>,
}

impl SAContext {
    /// 从 circuit 和 placeable 列表预计算所有组件的 footprint 信息。
    /// bridged_bboxes 需要 board 才能算, 所以在 simulate() 里面 extra 一步填充。
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
                    .physical_pin_for(pin)
                    .expect("footprint 缺 pin (解析阶段就该爆)");
                let offset_r0 = physical.offset;
                // R180: negate
                let offset_r180 = Position {
                    x: -offset_r0.x,
                    y: -offset_r0.y,
                };
                pins.push((
                    [
                        offset_r0,
                        offset_r180,
                        Position {
                            x: -offset_r0.y,
                            y: offset_r0.x,
                        },
                        Position {
                            x: offset_r0.y,
                            y: -offset_r0.x,
                        },
                    ],
                    pin.net,
                ));
                world_positions.push(offset_r0);
            }

            let bbox_r0 = BBox::from_points(world_positions).unwrap_or(BBox {
                min_x: 0,
                max_x: 0,
                min_y: 0,
                max_y: 0,
            });

            comp_infos.push(CompInfo {
                pins,
                bbox_r0,
                bridged_bboxes: None,
                bridged_pair_world: None,
            });
        }

        SAContext {
            comp_infos,
            external_bridged_world: Vec::new(),
            power_anchor_world: Vec::new(),
            power_anchor_nets: Vec::new(),
        }
    }

    /// 给 bridgeable 元件填 bridged_bboxes + bridged_pair_world 预计算。
    /// 同时填 external_bridged_world (用户预摆) 和 power_anchor_* (电源轨 anchor)。
    /// 调用时机: populate_bridgeable_info 之后, simulate 的 cost 调用之前。
    pub fn fill_bridged_bboxes(
        &mut self,
        state: &SAState,
        circuit: &Circuit,
        board: &Breadboard,
        bridged_pins: &[(crate::circuit::PinId, crate::layout::breadboard::HoleId)],
    ) {
        for (idx, info) in self.comp_infos.iter_mut().enumerate() {
            if !state.is_bridgeable[idx] {
                continue;
            }
            let mut bboxes = Vec::with_capacity(state.bridged_pin_pairs[idx].len());
            let mut worlds = Vec::with_capacity(state.bridged_pin_pairs[idx].len());
            for pair in &state.bridged_pin_pairs[idx] {
                // bbox
                let p0 = board.hole(pair[0].0).position;
                let p1 = board.hole(pair[1].0).position;
                bboxes.push(BBox {
                    min_x: p0.x.min(p1.x),
                    max_x: p0.x.max(p1.x),
                    min_y: p0.y.min(p1.y),
                    max_y: p0.y.max(p1.y),
                });
                // pin world (x, y, rail_id, net) × 2
                let r0 = board.rail_id_of(pair[0].0);
                let r1 = board.rail_id_of(pair[1].0);
                let n0 = circuit.pins[pair[0].1.0].net;
                let n1 = circuit.pins[pair[1].1.0].net;
                worlds.push([(p0.x, p0.y, r0, n0), (p1.x, p1.y, r1, n1)]);
            }
            info.bridged_bboxes = Some(bboxes);
            info.bridged_pair_world = Some(worlds);
        }

        // 用户预摆 bridged_pins
        self.external_bridged_world.clear();
        for &(pin_id, hole_id) in bridged_pins {
            let pin = &circuit.pins[pin_id.0];
            let pos = board.hole(hole_id).position;
            let rail_id = board.rail_id_of(hole_id);
            self.external_bridged_world
                .push((pos.x, pos.y, rail_id, pin.net));
        }

        // 电源轨 anchor
        self.power_anchor_world.clear();
        self.power_anchor_nets.clear();
        if let Some(binding) = board.power_rail_binding() {
            for (polarity, net_id) in binding.iter() {
                if let Some(anchor) = board.power_rail_anchor(polarity) {
                    let pos = board.hole(anchor).position;
                    let rail_id = board.rail_id_of(anchor);
                    self.power_anchor_world.push((pos.x, pos.y, rail_id));
                    self.power_anchor_nets.push(Some(net_id));
                }
            }
        }
    }
}

// ============================================================
//  Reusable Buffers: 避免 cost() 内部重复分配
// ============================================================

/// 所有 cost 计算复用的缓冲区。
/// 在 simulate() 里创建一次, 每次 cost 计算前 clear 后重用。
///
/// rail_map / compact_map 都以 u32/i32 为索引的 flat `Vec<Vec<...>>`。
/// HashMap 的 hash + Eq + bucket 跳转在这个热点上比直接数组索引慢几倍。
pub(crate) struct CostBuf {
    pub holes: Vec<(i32, i32, u32)>,
    pub nets: Vec<Option<NetId>>,
    pub is_virtual: Vec<bool>,
    pub bboxes: Vec<Option<BBox>>,
    /// net_id → pin 在 holes/nets 里的 index 列表 (按 net.0 索引)
    pub net_buckets: Vec<Vec<usize>>,
    /// rail_id → net 列表 (按 rail_id 索引; rail_id < num_rails)
    pub rail_map: Vec<Vec<Option<NetId>>>,
    /// rail_top → bbox 列表 (按 rail_top 索引; rail_top < main_rows)
    pub compact_map: Vec<Vec<BBox>>,
    /// pin 碰撞检测的 reusable 排序索引缓冲 (避免每次 alloc)
    pub pin_idx_sorted: Vec<usize>,
}

impl CostBuf {
    pub fn new(num_nets: usize, num_rails: usize, main_rows: usize) -> Self {
        Self {
            holes: Vec::new(),
            nets: Vec::new(),
            is_virtual: Vec::new(),
            bboxes: Vec::new(),
            net_buckets: vec![Vec::new(); num_nets],
            rail_map: vec![Vec::new(); num_rails],
            compact_map: vec![Vec::new(); main_rows],
            pin_idx_sorted: Vec::new(),
        }
    }

    /// 清理所有 buffer 以便下一轮 cost 计算复用
    pub(super) fn clear(&mut self) {
        self.holes.clear();
        self.nets.clear();
        self.is_virtual.clear();
        self.bboxes.clear();
        for bucket in &mut self.net_buckets {
            bucket.clear();
        }
        for v in &mut self.rail_map {
            v.clear();
        }
        for v in &mut self.compact_map {
            v.clear();
        }
        self.pin_idx_sorted.clear();
    }
}
