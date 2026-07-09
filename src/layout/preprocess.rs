//! 面包板预处理: 检测需要 R90 预旋转和/或 y 锁定的元件。

use std::collections::{HashMap, HashSet};

use crate::circuit::{Circuit, ComponentId, NetId};
use crate::layout::breadboard::Breadboard;

/// 预处理结果: 哪些元件需要特殊 SA 约束。
#[derive(Debug, Clone)]
pub struct PreprocessResult {
    /// 只能使用 R90 / R270 的元件
    pub r90_only: HashSet<ComponentId>,
    /// 需要锁定 y 坐标的元件 → 锁定值 (板坐标)
    pub y_locked: HashMap<ComponentId, i32>,
}

/// 检测需要预处理的元件。
///
/// 对称流程:
/// 1. 扫描所有元件的 footprint: 同一列 (local x) 有 ≥2 pin 且 net 不同 → 候选
/// 2. 对候选:
///    a. R0 下能 y-lock 跨通道? → 保持 R0, y-lock (横着进来的宽元件)
///    b. R90 后无同列冲突? → r90_only (竖着进来转横)
///    c. R90 下能 y-lock? → r90_only + y_locked (DIP 类)
///    d. 以上都不行 → panic
pub fn preprocess_for_breadboard(circuit: &Circuit, board: &Breadboard) -> PreprocessResult {
    let mut r90_only = HashSet::new();
    let mut y_locked = HashMap::new();

    let blocked_count = board.blocked_rows().len();

    for comp in circuit.components() {
        let Some(fid) = comp.footprint() else {
            continue;
        };
        let fp = &circuit.footprints()[fid.raw()];

        // Step 1: 检测 R0 下同列多 net
        if !has_column_conflict(comp.pins(), fp, circuit, false) {
            continue;
        }

        // Step 2a: R0 下能 y-lock 吗? (横着进来直接跨通道)
        if blocked_count > 0 {
            if let Some(locked_y) =
                try_y_lock(comp.pins(), fp, circuit, board, blocked_count, false)
            {
                y_locked.insert(comp.id(), locked_y);
                continue;
            }
        }

        // Step 2b: R90 后还有冲突吗?
        if !has_column_conflict(comp.pins(), fp, circuit, true) {
            r90_only.insert(comp.id());
            continue;
        }

        // Step 2c: R90 下能 y-lock 吗?
        if blocked_count == 0 {
            panic!(
                "{} (footprint {}) R90 后仍有列冲突, 且板子没有中央通道可以跨",
                comp.ref_(),
                fp.name(),
            );
        }
        if let Some(locked_y) = try_y_lock(comp.pins(), fp, circuit, board, blocked_count, true) {
            r90_only.insert(comp.id());
            y_locked.insert(comp.id(), locked_y);
            continue;
        }

        // Step 2d: 没办法 → panic
        panic!(
            "{} (footprint {}) R0/R90 后仍有列冲突, 且引脚分布无法跨通道解决",
            comp.ref_(),
            fp.name(),
        );
    }

    PreprocessResult { r90_only, y_locked }
}

/// 检查元件在给定旋转下是否有「同列不同 net」的冲突。
fn has_column_conflict(
    comp_pins: &[crate::circuit::PinId],
    fp: &crate::circuit::Footprint,
    circuit: &Circuit,
    r90: bool,
) -> bool {
    let mut x_groups: HashMap<i32, Vec<Option<NetId>>> = HashMap::new();
    for &pin_id in comp_pins {
        let pin = &circuit.pins()[pin_id.raw()];
        let Some(physical) = fp.physical_pin_for(pin) else {
            continue;
        };
        let x = if r90 {
            -physical.offset().y
        } else {
            physical.offset().x
        };
        x_groups.entry(x).or_default().push(pin.net());
    }
    x_groups.values().any(|nets| {
        let unique: HashSet<Option<NetId>> = nets.iter().copied().collect();
        unique.len() > 1
    })
}

/// 尝试为元件计算 y-lock 位置。
///
/// 条件: 在指定旋转下, 所有冲突 pin 分布在恰好 2 个 y 值上,
/// 且两行之间的空行数 == blocked_rows 数。
///
/// `r90` 控制使用 R0 还是 R90 旋转后的坐标。
/// 返回锁定的 state.y 值 (使上排 pin 落在上半区底部行)。
fn try_y_lock(
    comp_pins: &[crate::circuit::PinId],
    fp: &crate::circuit::Footprint,
    circuit: &Circuit,
    board: &Breadboard,
    blocked_count: usize,
    r90: bool,
) -> Option<i32> {
    // 收集指定旋转后所有冲突列里 pin 的 (x, y, net)
    let mut col_pins: HashMap<i32, Vec<(i32, Option<NetId>)>> = HashMap::new();
    for &pin_id in comp_pins {
        let pin = &circuit.pins()[pin_id.raw()];
        let Some(physical) = fp.physical_pin_for(pin) else {
            continue;
        };
        let (rx, ry) = if r90 {
            // R90: (x, y) → (-y, x)
            (-physical.offset().y, physical.offset().x)
        } else {
            (physical.offset().x, physical.offset().y)
        };
        col_pins.entry(rx).or_default().push((ry, pin.net()));
    }

    // 只保留「同列多 net」的列
    let conflict_cols: Vec<&Vec<(i32, Option<NetId>)>> = col_pins
        .values()
        .filter(|pins| {
            let nets: HashSet<Option<NetId>> = pins.iter().map(|&(_, n)| n).collect();
            nets.len() > 1
        })
        .collect();

    if conflict_cols.is_empty() {
        return None;
    }

    // 收集所有冲突列的 y 坐标集合
    let mut all_y: HashSet<i32> = HashSet::new();
    for pins in &conflict_cols {
        for &(y, _) in *pins {
            all_y.insert(y);
        }
    }

    // 必须恰好 2 行
    if all_y.len() != 2 {
        return None;
    }
    let mut ys: Vec<i32> = all_y.into_iter().collect();
    ys.sort();
    let y_low = ys[0];
    let y_high = ys[1];

    // 两行之间空行数 = y_high - y_low - 1
    let gap = (y_high - y_low - 1) as usize;
    if gap != blocked_count {
        return None;
    }

    // 上排 pin (y_low) 应落在上半区最底行, 下排 (y_high) 落在下半区最顶行。
    // 上半区最底行 = 第一个 blocked row - 1。
    let first_blocked = board.blocked_rows().first().copied()?;
    let upper_bottom = first_blocked as i32 - 1;
    let locked_y = upper_bottom - y_low;

    Some(locked_y)
}
