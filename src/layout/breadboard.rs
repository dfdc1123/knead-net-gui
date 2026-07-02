//! 面包板的物理结构。
//!
//! 一个 [`Breadboard`] 由两部分组成:
//! 1. **main board** — 中央插元件区, 5 (cols) × N (rows) 的网格, 中间可能有
//!    物理占位的 blocked row (面包板中央通道的简化)。同列内一段连续的非 blocked
//!    行是**纵向 rail** (面包板内部纵向短接)。
//! 2. **power rails** — 板子上下两端各一组横条。每组两条 (上面负极, 下面正极),
//!    每条由若干个 group 组成, 每个 group 内的孔横向短接 (面包板内部短接)。
//!
//! 短路关系用 `rail_id` 统一表达:
//! - main board 内的纵向 rail: 每个 (col, vertical_rail_top) 一个 rail_id
//! - power rail 行: 一行一个 rail_id
//!
//! 两个孔 `rail_id` 相同就内部短接 (距离 0), 不同就走 Manhattan。
//!
//! 坐标空间 (默认 30×12 main + 上下各一组电源轨, 主区到 rail 间各 2 行 gap):
//! ```text
//!   y=-4  [top negative]  横向短路, 5 组 5 孔
//!   y=-3  [top positive]  横向短路
//!   y=-2  ⨯ external gap (主区到 top rail 之间的 2 行间隔, 不可访问)
//!   y=-1  ⨯ external gap
//!   y= 0  ┐
//!   ...   ├ main upper rail (5 行, 内部纵向短路)
//!   y= 4  ┘
//!   y= 5  ⨯ blocked (中央通道, 不可访问)
//!   y= 6  ⨯ blocked
//!   y= 7  ┐
//!   ...   ├ main lower rail (5 行, 内部纵向短路)
//!   y=11  ┘
//!   y=12  ⨯ external gap (主区到 bottom rail 之间的 2 行间隔, 不可访问)
//!   y=13  ⨯ external gap
//!   y=14  [bottom negative] 横向短路
//!   y=15  [bottom positive] 横向短路
//! ```

use std::collections::{BTreeSet, HashMap};
use std::ops::RangeInclusive;

use crate::circuit::{NetId, Position};

/// 板上一个孔的标识, 范围 0..board.len()。
///
/// 编号规则: 在构造时按 (power rails → main board) 的顺序枚举所有孔, `HoleId` 是
/// 该枚举里的索引。`at(x, y)` 通过反向查表拿回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HoleId(pub(crate) usize);

impl HoleId {
    pub fn raw(self) -> usize {
        self.0
    }
}

/// 孔所属的区域类型。
///
/// `at(x, y)` 对 `Region::Gap` 处的 y 返回 `None` — 那些孔在 `holes` 里不存在
/// (跟原 `is_blocked` 语义一致, 中央通道 y 不参与布局)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    /// main board 内非 blocked 的孔; 同列同 vertical rail 短接
    MainRail,
    /// 电源轨上的孔; 同行短接
    PowerRail,
}

/// 电源轨极性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Polarity {
    Positive,
    Negative,
}

/// 板上一个孔的全部元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hole {
    pub id: HoleId,
    pub position: Position,
    pub region: Region,
    /// 短路集合 id; `Hole::rail_id` 相同的孔在面包板内部被短接在一起。
    /// main board 内的纵向短路和 power rail 内的横向短路用同一套 id 表达。
    pub rail_id: u32,
}

/// 一条电源轨 (一行 5+5+5+5+5 横向短接的孔)。
#[derive(Debug, Clone)]
pub struct PowerRail {
    /// 这条轨在板坐标空间里的 y 值 (例如 -1, -2, 12, 13)
    pub y: i32,
    pub polarity: Polarity,
    /// 短接的列范围列表, 通常 5 个 group, 每个 group 5 孔。
    /// `groups[0].end() + 1 == groups[1].start()` 中间是 1 个空孔断开。
    pub groups: Vec<RangeInclusive<i32>>,
}

impl PowerRail {
    /// 列出这条轨上所有 (短路) 孔的 x 坐标 (按 x 升序)。
    pub fn columns(&self) -> impl Iterator<Item = i32> + '_ {
        let mut xs: Vec<i32> = self.groups.iter().flat_map(|g| g.clone()).collect();
        xs.sort();
        xs.into_iter()
    }

    /// x 是否在这条轨上 (属于某个 group)。
    pub fn contains(&self, x: i32) -> bool {
        self.groups.iter().any(|g| g.contains(&x))
    }
}

/// 一组电源轨 (一条负极 + 一条正极, 上下叠在一起)。
#[derive(Debug, Clone)]
pub struct PowerStrip {
    /// `rows[0].y < rows[1].y`, 用户自行决定哪个是正哪个是负
    /// (约定俗成: rows[0] 是远离 main board 的那条, rows[1] 是靠近的)。
    pub rows: [PowerRail; 2],
}

/// 板子两端的电源轨配置 + 允许绑定的 net 名字列表。
///
/// "允许绑定的 net 名字" 是 UI/验证层的提示: 调用方拿这个名字在 netlist 里查
/// `NetId`。最终绑定 ([`NetId`] 级别的) 不放在这里, 由 layout 主流程传进来。
#[derive(Debug, Clone)]
pub struct PowerRails {
    pub top: PowerStrip,
    pub bottom: PowerStrip,
    /// 正极允许绑定的 net 名字 (eg `["VCC", "5V", "12V", "3V3"]`)
    pub positive_names: Vec<String>,
    /// 负极允许绑定的 net 名字 (eg `["GND"]`)
    pub negative_names: Vec<String>,
}

/// 电源轨到 net 的绑定: 哪条 rail 电气上等同于哪个 net。
///
/// 设置后, cost / 路由 会**自动注入一个虚拟 pin** 到该 rail 的 anchor 位置,
/// 并把它挂到绑定 net 上。这样:
/// - net 的 MST 必然包含 rail, 强制算上从主区到 rail 的 jumper 长度
/// - 路由器必会生成一根 wire 把 rail 连到主区最近 pin
/// - 如果同 rail 出现别的 net 的 pin, occupancy 的 rail 冲突检查会逮到
#[derive(Debug, Clone, Copy)]
pub struct PowerRailBinding {
    /// 正极 rail 绑定的 net (例: `+12V` / `5V` / `VCC`)
    pub positive: NetId,
    /// 负极 rail 绑定的 net (例: `GND`)
    pub negative: NetId,
}

#[derive(Debug, Clone)]
pub struct Breadboard {
    cols: usize,
    main_rows: usize,
    main_blocked_rows: BTreeSet<usize>,
    power_rails: Option<PowerRails>,
    /// 电源轨到 net 的绑定 (None = 不绑定, 跟以前一样)
    power_rail_binding: Option<PowerRailBinding>,
    holes: Vec<Hole>,
    /// (x, y) → HoleId 反查, 加速 `at` 调用
    at_map: HashMap<(i32, i32), HoleId>,
}

impl Breadboard {
    /// 创建 `cols × main_rows` 的 main board, 没有 blocked row, 没有 power rail。
    pub fn new(cols: usize, main_rows: usize) -> Self {
        Self::with_blocked_rows_and_power_rails(cols, main_rows, std::iter::empty(), None)
    }

    /// 创建 main board 并标 blocked row; 仍然没有 power rail。
    pub fn with_blocked_rows(
        cols: usize,
        main_rows: usize,
        main_blocked_rows: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self::with_blocked_rows_and_power_rails(cols, main_rows, main_blocked_rows, None)
    }

    /// 创建 main board + 上下两端 power rail 的完整面包板。
    pub fn with_power_rails(
        cols: usize,
        main_rows: usize,
        main_blocked_rows: impl IntoIterator<Item = usize>,
        power_rails: PowerRails,
    ) -> Self {
        Self::with_blocked_rows_and_power_rails(
            cols,
            main_rows,
            main_blocked_rows,
            Some(power_rails),
        )
    }

    fn with_blocked_rows_and_power_rails(
        cols: usize,
        main_rows: usize,
        main_blocked_rows: impl IntoIterator<Item = usize>,
        power_rails: Option<PowerRails>,
    ) -> Self {
        let main_blocked_rows: BTreeSet<usize> = main_blocked_rows.into_iter().collect();
        for &r in &main_blocked_rows {
            assert!(
                r < main_rows,
                "blocked row {} 越界 (main_rows = {})",
                r,
                main_rows
            );
        }

        // 校验 power rail 的 y 跟 main_rows 不冲突 (避免 -1 / 13 跟 main 内部 y 撞)
        if let Some(pr) = &power_rails {
            for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
                assert!(
                    rail.y < 0 || rail.y >= main_rows as i32,
                    "power rail y={} 落在 main board [0, {}) 范围内",
                    rail.y,
                    main_rows
                );
                for g in &rail.groups {
                    assert!(
                        *g.start() >= 0 && *g.end() < cols as i32,
                        "power rail group {:?} 越界 (cols = {})",
                        g,
                        cols
                    );
                }
            }
        }

        let mut holes = Vec::new();
        let mut at_map = HashMap::new();
        let mut next_rail_id: u32 = 0;

        // 1. 电源轨的 rail_id 分配 + 孔枚举
        if let Some(pr) = &power_rails {
            // 同一极性的所有行 (top + bottom) 共享一个 rail_id
            // (用户约定: 上下两条先简化, 短接 + 同一个 net)
            // 我们用 negative / positive 各一个 rail_id
            for &polarity in &[Polarity::Negative, Polarity::Positive] {
                let rail_id = next_rail_id;
                next_rail_id += 1;
                // 遍历所有 4 行, 找到极性匹配的
                for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
                    if rail.polarity != polarity {
                        continue;
                    }
                    for x in rail.columns() {
                        let pos = Position { x, y: rail.y };
                        let id = HoleId(holes.len());
                        holes.push(Hole {
                            id,
                            position: pos,
                            region: Region::PowerRail,
                            rail_id,
                        });
                        at_map.insert((x, rail.y), id);
                    }
                }
            }
        }

        // 2. main board: 收集所有 (col, vertical_rail_top) 对, 给每个分配 rail_id
        // 找出所有 vertical rail 的 top y
        let mut vertical_rails: Vec<usize> = Vec::new();
        for y in 0..main_rows {
            if main_blocked_rows.contains(&y) {
                continue;
            }
            // 检查是不是某个 rail 的 top (上一个 row 是 blocked 或者 y == 0)
            let is_top = y == 0 || main_blocked_rows.contains(&(y - 1));
            if is_top {
                vertical_rails.push(y);
            }
        }

        // 给每个 (col, vertical_rail_index) 一个 rail_id
        for y in 0..main_rows {
            if main_blocked_rows.contains(&y) {
                continue; // blocked row 没有孔
            }
            // 找该 y 所在 vertical rail 的 top
            let rail_top = *vertical_rails
                .iter()
                .find(|&&top| {
                    top <= y && (top + count_rail_rows(&main_blocked_rows, top, main_rows) > y)
                })
                .expect("每个非 blocked y 都属于某个 vertical rail");
            let rail_index = vertical_rails.iter().position(|&t| t == rail_top).unwrap();
            let rail_id = next_rail_id + (rail_index as u32) * (cols as u32); // 后面按 col 加
            for x in 0..cols {
                let id_rail = rail_id + x as u32;
                let pos = Position {
                    x: x as i32,
                    y: y as i32,
                };
                let id = HoleId(holes.len());
                holes.push(Hole {
                    id,
                    position: pos,
                    region: Region::MainRail,
                    rail_id: id_rail,
                });
                at_map.insert((x as i32, y as i32), id);
            }
        }
        let _ = next_rail_id + (vertical_rails.len() as u32) * (cols as u32);

        Self {
            cols,
            main_rows,
            main_blocked_rows,
            power_rails,
            power_rail_binding: None,
            holes,
            at_map,
        }
    }

    // ============================================================
    //  标准板
    // ============================================================

    /// 标准全尺寸面包板: 30 列 × 12 行 main, 中央 2 行 blocked,
    /// 上下各一组 5×5 横向短接的电源轨 (极性按用户约定: 远离 main 是负, 靠近是正)。
    pub fn standard() -> Self {
        let power_rails = standard_power_rails(30);
        Self::with_power_rails(30, 12, [5, 6], power_rails)
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn main_rows(&self) -> usize {
        self.main_rows
    }

    /// 兼容旧 API; 等价于 `main_rows()`。
    pub fn rows(&self) -> usize {
        self.main_rows
    }

    pub fn is_blocked(&self, row: usize) -> bool {
        self.main_blocked_rows.contains(&row)
    }

    pub fn blocked_rows(&self) -> Vec<usize> {
        self.main_blocked_rows.iter().copied().collect()
    }

    pub fn power_rails(&self) -> Option<&PowerRails> {
        self.power_rails.as_ref()
    }

    /// 总孔数 (= `cols * (main_rows - |blocked_rows|) + 电源轨孔数`)。
    pub fn len(&self) -> usize {
        self.holes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.holes.is_empty()
    }

    pub fn hole(&self, id: HoleId) -> &Hole {
        &self.holes[id.0]
    }

    pub fn holes(&self) -> &[Hole] {
        &self.holes
    }

    /// 板内坐标 → HoleId。
    ///
    /// 越界、blocked row、电源轨里不属于任何 group 的位置, 都返回 `None`。
    pub fn at(&self, x: i32, y: i32) -> Option<HoleId> {
        self.at_map.get(&(x, y)).copied()
    }

    /// 给定 y, 返回它所在的 main board vertical rail 的所有 y (含自身)。
    ///
    /// y 在 blocked row / 越界 / 电源轨上时返回空。
    pub fn rail_rows(&self, y: i32) -> Vec<i32> {
        if y < 0 || y >= self.main_rows as i32 {
            return Vec::new();
        }
        let y = y as usize;
        if self.main_blocked_rows.contains(&y) {
            return Vec::new();
        }
        // 找 rail 的 top
        let mut top = y;
        while top > 0 && !self.main_blocked_rows.contains(&(top - 1)) {
            top -= 1;
        }
        // 找 rail 的 bottom
        let mut bottom = y;
        while bottom + 1 < self.main_rows && !self.main_blocked_rows.contains(&(bottom + 1)) {
            bottom += 1;
        }
        (top..=bottom).map(|r| r as i32).collect()
    }

    /// 跟 `id` 内部短接的所有 HoleId (含自身)。
    ///
    /// - MainRail 孔: 同列同 vertical rail 的所有孔
    /// - PowerRail 孔: 同一 rail_id 的所有孔 (即同极性的所有 4 行, 因为我们把
    ///   同一极性的所有行合并到同一个 rail_id 里)
    pub fn connected_to(&self, id: HoleId) -> Vec<HoleId> {
        let target_rail = self.holes[id.0].rail_id;
        self.holes
            .iter()
            .filter(|h| h.rail_id == target_rail)
            .map(|h| h.id)
            .collect()
    }

    /// 给定一个 hole, 如果它在电源轨上, 返回它所属的 PowerRail; 否则 None。
    pub fn power_rail_of(&self, id: HoleId) -> Option<&PowerRail> {
        let hole = &self.holes[id.0];
        if hole.region != Region::PowerRail {
            return None;
        }
        let pr = self.power_rails.as_ref()?;
        let y = hole.position.y;
        pr.top
            .rows
            .iter()
            .chain(pr.bottom.rows.iter())
            .find(|r| r.y == y)
    }

    /// 设置电源轨到 net 的绑定。返回 self 便于链式调用。
    ///
    /// `binding.positive` / `binding.negative` 必须是有效 `NetId` (即
    /// `< circuit.nets().len()`), 否则 cost / 路由 时会静默忽略 (找不到 net)。
    pub fn with_power_rail_binding(mut self, binding: PowerRailBinding) -> Self {
        self.power_rail_binding = Some(binding);
        self
    }

    /// 当前是否设置了电源轨绑定。
    pub fn power_rail_binding(&self) -> Option<&PowerRailBinding> {
        self.power_rail_binding.as_ref()
    }

    /// 给定极性, 返回该 rail 上的一个 anchor `HoleId` (用作虚拟 pin 位置)。
    ///
    /// 选 top strip 里极性匹配那一行的**第一个**孔 (col 0)。因为同 rail 的所有
    /// 孔内部短接, anchor 选哪个孔都对, 这里只是稳定起见。
    /// 返回 `None` 表示: 没装 power rail, 或该极性在配置里不存在。
    pub fn power_rail_anchor(&self, polarity: Polarity) -> Option<HoleId> {
        let pr = self.power_rails.as_ref()?;
        let target_y = pr.top.rows.iter().find(|r| r.polarity == polarity)?.y;
        self.holes
            .iter()
            .find(|h| h.position.y == target_y)
            .map(|h| h.id)
    }

    /// 给定一个 hole, 返回它的 rail_id。
    pub fn rail_id_of(&self, id: HoleId) -> u32 {
        self.holes[id.0].rail_id
    }

    /// 给定一个 hole, 返回它的 region。
    pub fn region_of(&self, id: HoleId) -> Region {
        self.holes[id.0].region
    }

    /// 主区到 power rail 之间的 "external gap" (不能插线的空行) 列表。
    ///
    /// 每个范围是闭区间 `(top, bottom)`, 按 y 升序排列。这些行:
    /// - 在 `at(x, y)` 里返回 `None` (没有 HoleId)
    /// - 跟 main board 的中央 blocked row 一样, 渲染时画成灰色带
    /// - 物理意义: 电源轨跟主区之间必须有物理间隔, 不能放线
    ///
    /// 默认 [`standard_power_rails`] 会产生 2 个 gap: 顶 (top rail 下沿到主区)
    /// 和底 (主区到 bottom rail 上沿), 各 2 行。
    pub fn external_gaps(&self) -> Vec<(i32, i32)> {
        let mut gaps = Vec::new();
        let Some(pr) = &self.power_rails else {
            return gaps;
        };

        // top gap: 在 top rail 的最大 y + 1 到 -1 之间
        let top_rail_max_y = pr.top.rows.iter().map(|r| r.y).max().unwrap_or(-1);
        if top_rail_max_y < -1 {
            gaps.push((top_rail_max_y + 1, -1));
        }
        // bottom gap: 在 main_rows 到 bottom rail 的最小 y - 1 之间
        let bottom_rail_min_y = pr
            .bottom
            .rows
            .iter()
            .map(|r| r.y)
            .min()
            .unwrap_or(self.main_rows as i32);
        if (self.main_rows as i32) < bottom_rail_min_y {
            gaps.push((self.main_rows as i32, bottom_rail_min_y - 1));
        }
        gaps
    }
}

/// 返回从 `top` 开始的 vertical rail 长度 (含 top)。
fn count_rail_rows(blocked: &BTreeSet<usize>, top: usize, rows: usize) -> usize {
    let mut count = 1;
    let mut y = top + 1;
    while y < rows && !blocked.contains(&y) {
        count += 1;
        y += 1;
    }
    count
}

/// 默认电源轨配置: 30 列, 每条 5 组 5 孔, 上下各一组 (4 行)。
///
/// 排布 (相对 main board, y 从小到大):
/// - y=-4: top negative (5 groups: 0..4, 6..10, 12..16, 18..22, 24..28)
/// - y=-3: top positive
/// - y=14: bottom negative
/// - y=15: bottom positive
///
/// 主区 (y=0..11) 到 rail 之间各有 2 行 gap (y=-2,-1 和 y=12,13), 不可插线,
/// 跟中央通道同款。
///
/// 同一极性 (负或正) 的 top + bottom 两条**合并**为同一个 rail_id
/// (用户约定: 上下两组先简化, 短接 + 同一个 net)。
pub fn standard_power_rails(cols: i32) -> PowerRails {
    // 按 6-col 节拍重复 (5 个连续孔 + 1 个空孔断开): 0-4, 6-10, 12-16, ...
    // 最后一组可能被裁短 (e.g. cols=50 时最后一组是 48-49 而不是 48-52)。
    // 原来错误: 这个参数被当成 `_cols` 忽略, groups 硬编码 5 组只覆盖前 30 列。
    let mut groups: Vec<RangeInclusive<i32>> = Vec::new();
    let mut start = 0;
    while start < cols {
        let end = (start + 4).min(cols - 1);
        groups.push(start..=end);
        start += 6;
    }
    PowerRails {
        top: PowerStrip {
            rows: [
                PowerRail {
                    y: -4,
                    polarity: Polarity::Negative,
                    groups: groups.clone(),
                },
                PowerRail {
                    y: -3,
                    polarity: Polarity::Positive,
                    groups: groups.clone(),
                },
            ],
        },
        bottom: PowerStrip {
            rows: [
                PowerRail {
                    y: 14,
                    polarity: Polarity::Negative,
                    groups: groups.clone(),
                },
                PowerRail {
                    y: 15,
                    polarity: Polarity::Positive,
                    groups,
                },
            ],
        },
        positive_names: vec!["VCC".into(), "5V".into(), "12V".into(), "3V3".into()],
        negative_names: vec!["GND".into()],
    }
}

// ============================================================
//  测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // 一块没有 power rail 的纯 main board
    fn board_no_power() -> Breadboard {
        Breadboard::new(30, 5)
    }

    fn board_blocked() -> Breadboard {
        Breadboard::with_blocked_rows(30, 5, [1, 2])
    }

    fn board_full() -> Breadboard {
        Breadboard::standard()
    }

    #[test]
    fn new_30x5_has_150_holes() {
        let b = board_no_power();
        assert_eq!(b.len(), 150);
        assert_eq!(b.cols(), 30);
        assert_eq!(b.main_rows(), 5);
        assert!(b.blocked_rows().is_empty());
        assert!(b.power_rails().is_none());
    }

    #[test]
    fn with_blocked_rows_30x5_with_2_blocked_has_90_holes() {
        let b = board_blocked();
        assert_eq!(b.len(), 30 * 3);
        assert_eq!(b.blocked_rows(), vec![1, 2]);
    }

    #[test]
    fn standard_board_30x12_with_power_rails() {
        let b = board_full();
        // main 30×12 - 2 blocked = 300, + 4 排 × 25 孔 = 100 → 400
        assert_eq!(b.len(), 300 + 100);
        assert_eq!(b.cols(), 30);
        assert_eq!(b.main_rows(), 12);
        assert_eq!(b.blocked_rows(), vec![5, 6]);
        assert!(b.power_rails().is_some());
    }

    #[test]
    fn at_returns_none_for_blocked_rows() {
        let b = board_full();
        assert_eq!(b.at(0, 5), None);
        assert_eq!(b.at(0, 6), None);
        assert_eq!(b.at(15, 5), None);
    }

    #[test]
    fn at_returns_some_for_main_rails() {
        let b = board_full();
        // 上半
        assert!(b.at(0, 0).is_some());
        assert!(b.at(29, 4).is_some());
        // 下半
        assert!(b.at(0, 7).is_some());
        assert!(b.at(29, 11).is_some());
    }

    #[test]
    fn at_returns_some_for_power_rail_holes() {
        let b = board_full();
        // top negative row y=-4 (现在距主区 2 行)
        assert!(b.at(0, -4).is_some());
        assert!(b.at(4, -4).is_some());
        // gap (col 5) in power rail
        assert_eq!(b.at(5, -4), None);
        assert!(b.at(6, -4).is_some());
        // bottom positive row y=15
        assert!(b.at(28, 15).is_some());
        assert_eq!(b.at(29, 15), None); // col 29 是 unused
    }

    #[test]
    fn at_returns_none_for_garbage_y() {
        let b = board_full();
        assert_eq!(b.at(0, -5), None); // 板外
        assert_eq!(b.at(0, 16), None); // 板外
        assert_eq!(b.at(0, -2), None); // external gap (不能插线)
        assert_eq!(b.at(0, 13), None); // external gap
        assert_eq!(b.at(-1, 0), None);
        assert_eq!(b.at(30, 0), None);
    }

    #[test]
    fn power_rail_holes_have_region_power_rail() {
        let b = board_full();
        let id = b.at(0, -4).unwrap();
        assert_eq!(b.region_of(id), Region::PowerRail);
        let id = b.at(0, 15).unwrap();
        assert_eq!(b.region_of(id), Region::PowerRail);
        let id = b.at(0, 0).unwrap();
        assert_eq!(b.region_of(id), Region::MainRail);
    }

    #[test]
    fn same_main_column_same_rail_shorted() {
        let b = board_full();
        let a = b.at(15, 0).unwrap();
        let c = b.at(15, 4).unwrap();
        // 同列上半 rail: rows 0..5
        let connected = b.connected_to(a);
        assert_eq!(connected.len(), 5);
        for h in &connected {
            assert_eq!(b.hole(*h).position.x, 15);
            assert!(b.hole(*h).position.y < 5);
        }
        let _ = c; // unused, just to confirm we can look it up
    }

    #[test]
    fn main_upper_and_lower_are_independent() {
        let b = board_full();
        let upper = b.connected_to(b.at(7, 0).unwrap());
        let lower = b.connected_to(b.at(7, 7).unwrap());
        let upper_ids: std::collections::HashSet<_> = upper.into_iter().collect();
        let lower_ids: std::collections::HashSet<_> = lower.into_iter().collect();
        assert!(upper_ids.is_disjoint(&lower_ids));
    }

    #[test]
    fn power_rail_top_negative_is_shorted_across_columns() {
        let b = board_full();
        // top negative y=-4, 孔在 col 0 和 col 10
        let a = b.at(0, -4).unwrap();
        let c = b.at(10, -4).unwrap();
        let connected = b.connected_to(a);
        let ids: std::collections::HashSet<_> = connected.into_iter().collect();
        assert!(ids.contains(&c), "同 power rail 行的孔应该短接");
        // 用户约定: 同极性的 top + bottom 合并到同一 rail_id,
        // 所以连通集 = 25 (top) + 25 (bottom) = 50
        assert_eq!(ids.len(), 50);
    }

    #[test]
    fn power_rail_top_negative_and_bottom_negative_share_rail() {
        let b = board_full();
        // 用户约定: 上下两条同极性 shorted
        let top_neg = b.at(0, -4).unwrap();
        let bot_neg = b.at(0, 14).unwrap();
        let connected = b.connected_to(top_neg);
        let ids: std::collections::HashSet<_> = connected.into_iter().collect();
        assert!(ids.contains(&bot_neg));
        // 25 + 25 = 50 孔 (top 25 + bottom 25)
        assert_eq!(ids.len(), 50);
    }

    #[test]
    fn positive_and_negative_rails_are_independent() {
        let b = board_full();
        let neg = b.at(0, -4).unwrap();
        let pos = b.at(0, -3).unwrap();
        let neg_ids: std::collections::HashSet<_> = b.connected_to(neg).into_iter().collect();
        let pos_ids: std::collections::HashSet<_> = b.connected_to(pos).into_iter().collect();
        assert!(neg_ids.is_disjoint(&pos_ids));
    }

    #[test]
    fn rail_rows_returns_main_rail_range() {
        let b = board_full();
        assert_eq!(b.rail_rows(0), vec![0, 1, 2, 3, 4]);
        assert_eq!(b.rail_rows(2), vec![0, 1, 2, 3, 4]);
        assert!(b.rail_rows(5).is_empty());
        assert!(b.rail_rows(6).is_empty());
        assert_eq!(b.rail_rows(7), vec![7, 8, 9, 10, 11]);
    }

    #[test]
    fn rail_rows_for_power_rail_y_is_empty() {
        // rail_rows 只对 main board 内的 vertical rail 有意义
        let b = board_full();
        assert!(b.rail_rows(-2).is_empty());
        assert!(b.rail_rows(13).is_empty());
    }

    #[test]
    fn power_rail_of_returns_correct_rail() {
        let b = board_full();
        let id = b.at(0, -4).unwrap();
        let rail = b.power_rail_of(id).unwrap();
        assert_eq!(rail.y, -4);
        assert_eq!(rail.polarity, Polarity::Negative);

        let id = b.at(0, 15).unwrap();
        let rail = b.power_rail_of(id).unwrap();
        assert_eq!(rail.y, 15);
        assert_eq!(rail.polarity, Polarity::Positive);
    }

    #[test]
    fn power_rail_of_returns_none_for_main_holes() {
        let b = board_full();
        assert!(b.power_rail_of(b.at(0, 0).unwrap()).is_none());
    }

    #[test]
    fn standard_power_rails_5_groups_of_5() {
        let pr = standard_power_rails(30);
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            assert_eq!(rail.groups.len(), 5);
            for g in &rail.groups {
                assert_eq!(g.end() - g.start() + 1, 5);
            }
            // 总共 25 孔
            assert_eq!(rail.columns().count(), 25);
        }
    }

    /// cols=50 时多生成 4 组 (30→50 多出 20 列, 加上原 5 共 9 组)。
    /// 最后 1 组被裁短到 2 孔 (48+4=52 越界 → clip 到 49)。
    #[test]
    fn standard_power_rails_scales_with_cols() {
        let pr = standard_power_rails(50);
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            // 50/6 = 8.33, 下一组 start=54 退出循环 → 共 9 组
            assert_eq!(rail.groups.len(), 9);
            // 前 8 组都是 5 孔, 最后 1 组 (48-49) 是 2 孔
            for g in &rail.groups[..8] {
                assert_eq!(g.end() - g.start() + 1, 5, "前 8 组必须是 5 孔");
            }
            assert_eq!(rail.groups[8].end() - rail.groups[8].start() + 1, 2);
            // 总孔数 = 8×5 + 2 = 42
            assert_eq!(rail.columns().count(), 42);
        }
    }

    /// cols=60 时最后一组也能放完整 5 孔 (48+4=52 < 60)。
    #[test]
    fn standard_power_rails_60_cols_full_groups() {
        let pr = standard_power_rails(60);
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            assert_eq!(rail.groups.len(), 10);
            for g in &rail.groups {
                assert_eq!(g.end() - g.start() + 1, 5);
            }
        }
    }

    #[test]
    fn with_blocked_rows_panics_on_out_of_range() {
        let r = std::panic::catch_unwind(|| Breadboard::with_blocked_rows(5, 5, [5]));
        assert!(r.is_err());
    }

    #[test]
    fn any_dimensions_work_no_power() {
        let b = Breadboard::new(5, 30);
        assert_eq!(b.len(), 150);
        assert_eq!(b.connected_to(b.at(2, 5).unwrap()).len(), 30);
    }

    #[test]
    fn blocked_rows_in_middle_split_into_two_rails() {
        let b = Breadboard::with_blocked_rows(1, 4, [1, 2]);
        assert_eq!(b.len(), 2);
        assert_eq!(b.at(0, 0), Some(HoleId(0)));
        assert_eq!(b.at(0, 3), Some(HoleId(1)));
    }

    // ============================================================
    //  PowerRailBinding
    // ============================================================

    #[test]
    fn binding_default_is_none() {
        let b = Breadboard::standard();
        assert!(b.power_rail_binding().is_none());
    }

    #[test]
    fn with_power_rail_binding_sets_it() {
        use crate::circuit::NetId;
        let binding = PowerRailBinding {
            positive: NetId(0),
            negative: NetId(1),
        };
        let b = Breadboard::standard().with_power_rail_binding(binding);
        let got = b.power_rail_binding().unwrap();
        assert_eq!(got.positive, NetId(0));
        assert_eq!(got.negative, NetId(1));
    }

    #[test]
    fn power_rail_anchor_returns_first_hole_in_top_rail() {
        let b = Breadboard::standard();
        // 负极 anchor: top strip 的 y=-4 行, col 0
        let neg = b.power_rail_anchor(Polarity::Negative).unwrap();
        let neg_pos = b.hole(neg).position;
        assert_eq!(neg_pos, Position { x: 0, y: -4 });
        // 正极 anchor: top strip 的 y=-3 行, col 0
        let pos = b.power_rail_anchor(Polarity::Positive).unwrap();
        let pos_pos = b.hole(pos).position;
        assert_eq!(pos_pos, Position { x: 0, y: -3 });
    }

    #[test]
    fn power_rail_anchor_returns_none_without_rails() {
        let b = Breadboard::new(30, 5);
        assert!(b.power_rail_anchor(Polarity::Negative).is_none());
        assert!(b.power_rail_anchor(Polarity::Positive).is_none());
    }

    // ============================================================
    //  External gap (主区到 rail 之间的空行)
    // ============================================================

    #[test]
    fn external_gaps_standard_board() {
        let b = Breadboard::standard();
        // top: y=-4, -3 是 rail, y=-2, -1 是 gap, y=0 是 main
        // bottom: y=11 是 main, y=12, 13 是 gap, y=14, 15 是 rail
        let gaps = b.external_gaps();
        assert_eq!(gaps, vec![(-2, -1), (12, 13)]);
    }

    #[test]
    fn external_gaps_in_at_returns_none() {
        let b = Breadboard::standard();
        // gap 行不能插线: at() 返回 None
        assert_eq!(b.at(0, -2), None);
        assert_eq!(b.at(15, -1), None);
        assert_eq!(b.at(0, 12), None);
        assert_eq!(b.at(28, 13), None);
    }

    #[test]
    fn external_gaps_empty_without_rails() {
        let b = Breadboard::new(30, 5);
        assert!(b.external_gaps().is_empty());
    }
}
