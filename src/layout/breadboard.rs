//! 面包板的物理结构。
//!
//! 一个 [`Breadboard`] 由两部分组成:
//! 1. **main board** — 中央插元件区, 默认 30 × 12 (cols × rows) 的网格 (`standard()`),
//!    中间可能有物理占位的 blocked row (面包板中央通道的简化)。
//!    同列内一段连续的非 blocked 行是**纵向 rail** (面包板内部纵向短接)。
//! 2. **power rails** — 板子上下两端各一组横条。每组两条 (上面负极, 下面正极),
//!    每条由若干个 5 孔 group 组成。group 只是孔位分组; 同一整行由连续导体
//!    天然短接。
//!
//! 短路关系用 `rail_id` 统一表达:
//! - main board 内的纵向 rail: 每个 (col, vertical_rail_top) 一个 rail_id
//! - power rail: 每条完整的 top/bottom 行各有一个 rail_id
//!
//! `rail_id` 只表示板内导体。top/bottom 的外部短接由显式 [`RailTie`] 表示;
//! 算法消费者通过 effective-connectivity 查询同时考虑两者。
//!
//! 坐标空间 (以下图示基于 `Breadboard::standard()` 配置 (30×12 main, blocked_rows=[5,6]);
//! 其它尺寸板的 row / col 数不同但电源轨排布结构一致):
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

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use crate::circuit::{NetId, Position};

/// Default number of unavailable logical columns between adjacent full-size breadboards.
/// Use [`Preset::inter_board_gap_cols`] when the preset is known.
pub const INTER_BOARD_GAP_COLS: usize = 3;

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
/// `Region` 只描述**真实存在的孔**的归属 — 外部 gap / blocked row 上的
/// y 在 `at(x, y)` 里返回 `None`, 是因为这些 y 根本没有对应的 Hole,
/// 跟 `Region` 无关 (这里并没有 `Region::Gap` 这种变体)。
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

/// 电源轨位于主插接区的哪一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerRailSide {
    Top,
    Bottom,
}

/// 面包板内部一片连续导体的稳定标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConductiveIslandId(u32);

impl ConductiveIslandId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// 布局内一条外部电源轨短接线的索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RailTieId(usize);

impl RailTieId {
    pub fn raw(self) -> usize {
        self.0
    }
}

/// RailTie 的来源只影响展示与“恢复默认值”，不影响电气语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RailTieSource {
    Preset,
    User,
}

/// 显式连接两个 conductive islands 的真实外部导体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailTie {
    pub id: RailTieId,
    /// 用于 GUI/serialization 的稳定语义 id。
    pub key: String,
    pub from: HoleId,
    pub to: HoleId,
    pub source: RailTieSource,
    pub label: Option<String>,
}

impl RailTie {
    pub fn contacts(&self) -> [HoleId; 2] {
        [self.from, self.to]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailTieError {
    UnknownHole { hole: HoleId },
    NonPowerRail { hole: HoleId },
    SameIsland { from: HoleId, to: HoleId },
    DuplicateEndpoint { hole: HoleId },
    DuplicateKey { key: String },
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

/// 一条完整电源轨行；所有 group 位于同一片连续导体上。
#[derive(Debug, Clone)]
pub struct PowerRail {
    /// 这条轨在板坐标空间里的 y 值 (例如 -1, -2, 12, 13)
    pub y: i32,
    pub polarity: Polarity,
    /// 有插孔的列范围列表, 通常 5 个 group, 每个 group 5 孔。
    /// group 间的 1 格没有插孔，但底层导体仍连续。
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerRailBinding {
    /// 正极 rail 绑定的 net (例: `+12V` / `5V` / `VCC`)
    pub positive: Option<NetId>,
    /// 负极 rail 绑定的 net (例: `GND`)
    pub negative: Option<NetId>,
}

/// 上下四条物理电源轨各自的网络绑定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerRailBindings {
    pub top: PowerRailBinding,
    pub bottom: PowerRailBinding,
}

impl PowerRailBindings {
    /// 兼容旧行为：同一极性的上下轨使用相同绑定。
    pub fn mirrored(binding: PowerRailBinding) -> Self {
        Self {
            top: binding,
            bottom: binding,
        }
    }

    /// 按上负、上正、下负、下正的稳定顺序遍历实际绑定。
    pub fn iter(&self) -> impl Iterator<Item = (PowerRailSide, Polarity, NetId)> {
        [
            (PowerRailSide::Top, Polarity::Negative, self.top.negative),
            (PowerRailSide::Top, Polarity::Positive, self.top.positive),
            (
                PowerRailSide::Bottom,
                Polarity::Negative,
                self.bottom.negative,
            ),
            (
                PowerRailSide::Bottom,
                Polarity::Positive,
                self.bottom.positive,
            ),
        ]
        .into_iter()
        .filter_map(|(side, polarity, net)| net.map(|net| (side, polarity, net)))
    }

    pub fn is_empty(&self) -> bool {
        self.top.is_empty() && self.bottom.is_empty()
    }
}

impl PowerRailBinding {
    /// 按负极、正极的稳定顺序遍历当前实际绑定的电源轨。
    pub fn iter(&self) -> impl Iterator<Item = (Polarity, NetId)> {
        [
            (Polarity::Negative, self.negative),
            (Polarity::Positive, self.positive),
        ]
        .into_iter()
        .filter_map(|(polarity, net)| net.map(|net| (polarity, net)))
    }

    pub fn is_empty(&self) -> bool {
        self.positive.is_none() && self.negative.is_none()
    }
}

/// 预设板型 (170 / 400 / 830 孔) — main.rs 唯一选择点。
///
/// 选 `Preset` + 传 `cols` 调 [`Preset::make`] 就能拿到 [`Breadboard`]。
/// 电源轨 (`Preset::Hole170` 无, `Preset::Hole400` 是 5×5, `Preset::Hole830` 是 10×5 左右空 2)
/// 全部由 `make` 内部决定, 上层不用拼。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    /// 17×10 main, 无电源轨。默认 cols=17 → 170 孔; cols=N → N×10 孔。
    Hole170,
    /// 30×10 main + 4 条 5×5 电源轨 (无左右留白)。默认 cols=30 → 400 孔; cols=N → N×10 + 4 rail 孔。
    Hole400,
    /// 63×10 main + 4 条 10×5 电源轨 (左右各空 2)。默认 cols=63 → 830 孔; cols=N → N×10 + 4 rail 孔。
    Hole830,
}

impl Preset {
    /// 用这个预设 + `cols` 列宽生成一块 [`Breadboard`]。
    /// `cols` 改了后内部电源轨会自动按 6-col 节拍重排。
    pub fn make(self, cols: usize) -> Breadboard {
        match self {
            Self::Hole170 => Breadboard::preset_170(cols),
            Self::Hole400 => Breadboard::preset_400(cols),
            Self::Hole830 => Breadboard::preset_830(cols),
        }
    }

    /// 生成仅启用上半主插接区的预设板。
    ///
    /// 下半主插接区（含中央通道以下的五行）全部标为 blocked，且下方电源轨
    /// 没有孔位；因而布局、合法性校验和布线都不能使用下半板。带电源轨的
    /// 预设保留上方电源轨可用，同时不创建上下轨短接线。
    pub fn make_upper_half(self, cols: usize) -> Breadboard {
        match self {
            Self::Hole170 => Breadboard::with_blocked_rows(cols, 12, 5..12),
            Self::Hole400 => Breadboard::with_power_rails(
                cols,
                12,
                5..12,
                top_power_rails_only(standard_power_rails(cols as i32)),
            ),
            Self::Hole830 => Breadboard::with_power_rails(
                cols,
                12,
                5..12,
                top_power_rails_only(wide_power_rails_830(cols as i32)),
            ),
        }
    }

    /// Repeat a physical preset along one global x axis while restarting each board's
    /// power-rail hole pattern locally. The repeated strips remain one logical conductive
    /// island per row; physical inter-board connectivity is intentionally not modelled.
    pub fn make_repeated(self, board_count: usize) -> Breadboard {
        assert!(board_count > 0, "board_count must be positive");
        let board_cols = self.default_cols();
        let gap_cols = self.inter_board_gap_cols();
        let cols = repeated_total_cols(board_cols, gap_cols, board_count);
        let blocked_cols = repeated_gap_columns(board_cols, gap_cols, board_count);
        match self {
            Self::Hole170 => Breadboard::with_blocked_rows_and_cols(cols, 12, [5, 6], blocked_cols),
            Self::Hole400 => Breadboard::with_blocked_rows_cols_and_power_rails(
                cols,
                12,
                [5, 6],
                blocked_cols,
                Some(repeat_power_rails(
                    standard_power_rails(board_cols as i32),
                    board_cols,
                    gap_cols,
                    board_count,
                )),
            )
            .with_default_power_rail_ties(),
            Self::Hole830 => Breadboard::with_blocked_rows_cols_and_power_rails(
                cols,
                12,
                [5, 6],
                blocked_cols,
                Some(repeat_power_rails(
                    wide_power_rails_830(board_cols as i32),
                    board_cols,
                    gap_cols,
                    board_count,
                )),
            )
            .with_default_power_rail_ties(),
        }
    }

    /// Upper-half-only counterpart of [`Self::make_repeated`].
    pub fn make_repeated_upper_half(self, board_count: usize) -> Breadboard {
        assert!(board_count > 0, "board_count must be positive");
        let board_cols = self.default_cols();
        let gap_cols = self.inter_board_gap_cols();
        let cols = repeated_total_cols(board_cols, gap_cols, board_count);
        let blocked_cols = repeated_gap_columns(board_cols, gap_cols, board_count);
        match self {
            Self::Hole170 => Breadboard::with_blocked_rows_and_cols(cols, 12, 5..12, blocked_cols),
            Self::Hole400 => Breadboard::with_blocked_rows_cols_and_power_rails(
                cols,
                12,
                5..12,
                blocked_cols,
                Some(top_power_rails_only(repeat_power_rails(
                    standard_power_rails(board_cols as i32),
                    board_cols,
                    gap_cols,
                    board_count,
                ))),
            ),
            Self::Hole830 => Breadboard::with_blocked_rows_cols_and_power_rails(
                cols,
                12,
                5..12,
                blocked_cols,
                Some(top_power_rails_only(repeat_power_rails(
                    wide_power_rails_830(board_cols as i32),
                    board_cols,
                    gap_cols,
                    board_count,
                ))),
            ),
        }
    }

    /// 默认 `cols` 值 (这个预设“典型”的宽度)。
    pub fn default_cols(self) -> usize {
        match self {
            Self::Hole170 => 17,
            Self::Hole400 => 30,
            Self::Hole830 => 63,
        }
    }

    /// Unavailable logical columns inserted between adjacent boards of this preset.
    pub fn inter_board_gap_cols(self) -> usize {
        match self {
            Self::Hole170 => 2,
            Self::Hole400 | Self::Hole830 => INTER_BOARD_GAP_COLS,
        }
    }

    /// 预设名 (“170” / “400” / “830”), 跟文件名 / 日志一起用。
    pub fn name(self) -> &'static str {
        match self {
            Self::Hole170 => "170",
            Self::Hole400 => "400",
            Self::Hole830 => "800",
        }
    }
}

fn repeated_total_cols(board_cols: usize, gap_cols: usize, board_count: usize) -> usize {
    board_cols
        .checked_mul(board_count)
        .and_then(|cols| {
            gap_cols
                .checked_mul(board_count.saturating_sub(1))
                .and_then(|gaps| cols.checked_add(gaps))
        })
        .expect("repeated breadboard width overflow")
}

fn repeated_gap_columns(board_cols: usize, gap_cols: usize, board_count: usize) -> BTreeSet<usize> {
    let stride = board_cols + gap_cols;
    (1..board_count)
        .flat_map(|board_index| {
            let start = board_index * stride - gap_cols;
            start..start + gap_cols
        })
        .collect()
}

fn repeat_power_rails(
    mut rails: PowerRails,
    board_cols: usize,
    gap_cols: usize,
    board_count: usize,
) -> PowerRails {
    let offset_groups = |groups: &[RangeInclusive<i32>]| {
        (0..board_count)
            .flat_map(|board_index| {
                let offset = (board_index * (board_cols + gap_cols)) as i32;
                groups
                    .iter()
                    .map(move |group| (*group.start() + offset)..=(*group.end() + offset))
            })
            .collect()
    };
    for rail in rails
        .top
        .rows
        .iter_mut()
        .chain(rails.bottom.rows.iter_mut())
    {
        rail.groups = offset_groups(&rail.groups);
    }
    rails
}

#[derive(Debug, Clone)]
pub struct Breadboard {
    cols: usize,
    main_rows: usize,
    main_blocked_rows: BTreeSet<usize>,
    main_blocked_cols: BTreeSet<usize>,
    power_rails: Option<PowerRails>,
    power_rail_bindings: Option<PowerRailBindings>,
    /// 仅当上下绑定相同时提供，保留旧的逐极性查询 API。
    power_rail_binding: Option<PowerRailBinding>,
    rail_ties: Vec<RailTie>,
    /// physical island id -> RailTie 闭包的稳定代表 id。
    effective_rail_ids: Vec<u32>,
    holes: Vec<Hole>,
    /// `(y - at_y_min) * cols + x` → HoleId. 预分配的 flat grid,比 HashMap 快几倍,
    /// 拿掉 cost_fast 里的 `board.at(x,y)` 热点的 hash 开销。out-of-bounds / blocked
    /// row / gap 位置都是 `None`。
    at_grid: Vec<Option<HoleId>>,
    at_y_min: i32,
    at_y_max: i32,
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

    fn with_blocked_rows_and_cols(
        cols: usize,
        main_rows: usize,
        main_blocked_rows: impl IntoIterator<Item = usize>,
        main_blocked_cols: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self::with_blocked_rows_cols_and_power_rails(
            cols,
            main_rows,
            main_blocked_rows,
            main_blocked_cols,
            None,
        )
    }

    fn with_blocked_rows_and_power_rails(
        cols: usize,
        main_rows: usize,
        main_blocked_rows: impl IntoIterator<Item = usize>,
        power_rails: Option<PowerRails>,
    ) -> Self {
        Self::with_blocked_rows_cols_and_power_rails(
            cols,
            main_rows,
            main_blocked_rows,
            std::iter::empty(),
            power_rails,
        )
    }

    fn with_blocked_rows_cols_and_power_rails(
        cols: usize,
        main_rows: usize,
        main_blocked_rows: impl IntoIterator<Item = usize>,
        main_blocked_cols: impl IntoIterator<Item = usize>,
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
        let main_blocked_cols: BTreeSet<usize> = main_blocked_cols.into_iter().collect();
        for &col in &main_blocked_cols {
            assert!(col < cols, "blocked column {} 越界 (cols = {})", col, cols);
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
        // 确定 at_grid 范围: 包住所有 main row + power rail y; cols 是 x 轴范围
        let (at_y_min, at_y_max) = if let Some(pr) = &power_rails {
            let lo = pr.top.rows.iter().map(|r| r.y).min().unwrap_or(0);
            let hi = pr
                .bottom
                .rows
                .iter()
                .map(|r| r.y)
                .max()
                .unwrap_or(main_rows as i32 - 1);
            (lo.min(0), hi.max(main_rows as i32 - 1))
        } else {
            (0, main_rows as i32 - 1)
        };
        let grid_rows = (at_y_max - at_y_min + 1) as usize;
        let mut at_grid: Vec<Option<HoleId>> = vec![None; grid_rows * cols];
        let mut next_rail_id: u32 = 0;

        // 1. 电源轨的 physical-island rail_id 分配 + 孔枚举
        if let Some(pr) = &power_rails {
            // 保持旧 HoleId 枚举次序 (negative 再 positive)，但每条完整行分别分配
            // physical island id。同行 group 共享 id；top/bottom 不共享。
            for &polarity in &[Polarity::Negative, Polarity::Positive] {
                for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
                    if rail.polarity != polarity {
                        continue;
                    }
                    let rail_id = next_rail_id;
                    next_rail_id += 1;
                    for x in rail.columns() {
                        let pos = Position { x, y: rail.y };
                        let id = HoleId(holes.len());
                        holes.push(Hole {
                            id,
                            position: pos,
                            region: Region::PowerRail,
                            rail_id,
                        });
                        at_grid[((rail.y - at_y_min) as usize) * cols + (x as usize)] = Some(id);
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
                if main_blocked_cols.contains(&x) {
                    continue;
                }
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
                at_grid[((y as i32 - at_y_min) as usize) * cols + x] = Some(id);
            }
        }
        let num_rails = next_rail_id + (vertical_rails.len() as u32) * (cols as u32);

        Self {
            cols,
            main_rows,
            main_blocked_rows,
            main_blocked_cols,
            power_rails,
            power_rail_bindings: None,
            power_rail_binding: None,
            rail_ties: Vec::new(),
            effective_rail_ids: (0..num_rails).collect(),
            holes,
            at_grid,
            at_y_min,
            at_y_max,
        }
    }

    // ============================================================
    //  标准板
    // ============================================================

    /// 标准全尺寸面包板: 30 列 × 12 行 main, 中央 2 行 blocked,
    /// 上下各一组 5×5 横向短接的电源轨 (极性按用户约定: 远离 main 是负, 靠近是正)。
    pub fn standard() -> Self {
        let power_rails = standard_power_rails(30);
        Self::with_power_rails(30, 12, [5, 6], power_rails).with_default_power_rail_ties()
    }

    // ============================================================
    //  预设板 (preset) — 170 / 400 / 830 孔
    // ============================================================

    /// 170 孔预设: `cols` 列 × 12 行 main (中央 2 行 blocked, 上半 5 行 + 下半 5 行 = 10 行),
    /// 没有电源轨。`cols` 是板子横向 col 数, 默认 17 → 170 孔;
    /// 传 20 → 200 孔, 传 30 → 300 孔, 依此类推。
    pub fn preset_170(cols: usize) -> Self {
        Self::with_blocked_rows(cols, 12, [5, 6])
    }

    /// 400 孔预设: `cols` 列 × 12 行 main (10 行可用 + 2 行中央 blocked),
    /// 上下各两组 5×5 横向短接的电源轨 (无左右留白, 轨从 x=0 开始)。
    /// 默认 30 列: 30 × 10 = 300 main + 4 × 25 = 100 rail = 400 孔。
    /// 改 `cols` 后电源轨自动按 6-col 节拍重新生成 (5 孔 + 1 空), 最后一组可能被裁短。
    pub fn preset_400(cols: usize) -> Self {
        Self::with_power_rails(cols, 12, [5, 6], standard_power_rails(cols as i32))
            .with_default_power_rail_ties()
    }

    /// 830 孔预设: `cols` 列 × 12 行 main (10 行可用 + 2 行中央 blocked),
    /// 上下各两组 10×5 横向短接的电源轨 (左右各留 2 格空)。
    /// 默认 63 列: 63 × 10 = 630 main + 4 × 50 = 200 rail = 830 孔。
    /// 改 `cols` 后电源轨在 [2, cols-2) 区间按 6-col 节拍重新生成。
    pub fn preset_830(cols: usize) -> Self {
        Self::with_power_rails(cols, 12, [5, 6], wide_power_rails_830(cols as i32))
            .with_default_power_rail_ties()
    }

    /// 返回正极电源轨允许绑定的 net 名字列表。
    /// 没有电源轨的板子 (e.g. `preset_170`) 返回空切片。
    pub fn positive_names(&self) -> &[String] {
        self.power_rails
            .as_ref()
            .map(|pr| pr.positive_names.as_slice())
            .unwrap_or(&[])
    }

    /// 返回负极电源轨允许绑定的 net 名字列表。
    /// 没有电源轨的板子 (e.g. `preset_170`) 返回空切片。
    pub fn negative_names(&self) -> &[String] {
        self.power_rails
            .as_ref()
            .map(|pr| pr.negative_names.as_slice())
            .unwrap_or(&[])
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

    pub fn is_blocked_col(&self, col: usize) -> bool {
        self.main_blocked_cols.contains(&col)
    }

    pub fn power_rails(&self) -> Option<&PowerRails> {
        self.power_rails.as_ref()
    }

    /// 总孔数 (= 非 blocked 行列交叉点数 + 电源轨孔数)。
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
        if x < 0 || x >= self.cols as i32 {
            return None;
        }
        if y < self.at_y_min || y > self.at_y_max {
            return None;
        }
        let idx = ((y - self.at_y_min) as usize) * self.cols + (x as usize);
        // 初始化为 None 的位置 (blocked row / gap / 板外) 直接返 None
        unsafe { *self.at_grid.get_unchecked(idx) }
    }

    /// 给定 y, 返回它所在的 main board vertical rail 的 top y (只返一个数)。
    /// 不分配 Vec, 热路径首选。y 在 blocked row / 越界 / 电源轨上时返回 None。
    pub fn rail_top(&self, y: i32) -> Option<i32> {
        if y < 0 || y >= self.main_rows as i32 {
            return None;
        }
        let y = y as usize;
        if self.main_blocked_rows.contains(&y) {
            return None;
        }
        let mut top = y;
        while top > 0 && !self.main_blocked_rows.contains(&(top - 1)) {
            top -= 1;
        }
        Some(top as i32)
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

    /// 跟 `id` 由板内铜片天然短接的所有 HoleId (含自身)。
    ///
    /// - MainRail 孔: 同列同 vertical rail 的所有孔
    /// - PowerRail 孔: 同一完整电源轨行的所有孔（跨 group，但不跨 top/bottom）
    pub fn connected_to(&self, id: HoleId) -> Vec<HoleId> {
        let target_rail = self.holes[id.0].rail_id;
        self.holes
            .iter()
            .filter(|h| h.rail_id == target_rail)
            .map(|h| h.id)
            .collect()
    }

    /// 跟 `id` 通过板内铜片和当前全部 RailTie 有效连通的所有 HoleId。
    pub fn effectively_connected_to(&self, id: HoleId) -> Vec<HoleId> {
        let target = self.effective_rail_id_of(id);
        self.holes
            .iter()
            .filter(|hole| self.effective_rail_id_of(hole.id) == target)
            .map(|hole| hole.id)
            .collect()
    }

    /// 当前显式 RailTie。preset tie 与用户 tie 的电气语义相同。
    pub fn rail_ties(&self) -> &[RailTie] {
        &self.rail_ties
    }

    /// 某孔是否被 RailTie 端点占用。
    pub fn rail_tie_at(&self, hole: HoleId) -> Option<RailTieId> {
        self.rail_ties
            .iter()
            .find(|tie| tie.from == hole || tie.to == hole)
            .map(|tie| tie.id)
    }

    /// 新增一条用户 RailTie；成功后立即更新 effective connectivity。
    pub fn add_user_rail_tie(
        &mut self,
        key: impl Into<String>,
        from: HoleId,
        to: HoleId,
        label: Option<String>,
    ) -> Result<RailTieId, RailTieError> {
        self.push_rail_tie(key.into(), from, to, RailTieSource::User, label)
    }

    /// 按稳定 key 删除 RailTie；删除成功后立即更新 effective connectivity。
    pub fn remove_rail_tie(&mut self, key: &str) -> Option<RailTie> {
        let index = self.rail_ties.iter().position(|tie| tie.key == key)?;
        let tie = self.rail_ties.remove(index);
        self.rebuild_effective_rail_ids();
        Some(tie)
    }

    /// 清除所有 preset/user ties，保留裸板 physical islands。
    pub fn without_rail_ties(mut self) -> Self {
        self.rail_ties.clear();
        self.rebuild_effective_rail_ids();
        self
    }

    fn push_rail_tie(
        &mut self,
        key: String,
        from: HoleId,
        to: HoleId,
        source: RailTieSource,
        label: Option<String>,
    ) -> Result<RailTieId, RailTieError> {
        for hole in [from, to] {
            if hole.raw() >= self.holes.len() {
                return Err(RailTieError::UnknownHole { hole });
            }
            if self.region_of(hole) != Region::PowerRail {
                return Err(RailTieError::NonPowerRail { hole });
            }
            if self.rail_tie_at(hole).is_some() {
                return Err(RailTieError::DuplicateEndpoint { hole });
            }
        }
        if self.rail_id_of(from) == self.rail_id_of(to) {
            return Err(RailTieError::SameIsland { from, to });
        }
        if self.rail_ties.iter().any(|tie| tie.key == key) {
            return Err(RailTieError::DuplicateKey { key });
        }
        let id = RailTieId(
            self.rail_ties
                .iter()
                .map(|tie| tie.id.raw())
                .max()
                .map_or(0, |max| max + 1),
        );
        self.rail_ties.push(RailTie {
            id,
            key,
            from,
            to,
            source,
            label,
        });
        self.rebuild_effective_rail_ids();
        Ok(id)
    }

    fn with_default_power_rail_ties(mut self) -> Self {
        for polarity in [Polarity::Negative, Polarity::Positive] {
            let Some([from, to]) = self.rightmost_power_rail_anchors(polarity) else {
                continue;
            };
            let polarity_name = match polarity {
                Polarity::Negative => "negative",
                Polarity::Positive => "positive",
            };
            self.push_rail_tie(
                format!("preset:{polarity_name}:top-bottom"),
                from,
                to,
                RailTieSource::Preset,
                Some("default power-rail tie".to_owned()),
            )
            .expect("preset RailTie geometry must be valid");
        }
        self
    }

    fn rightmost_power_rail_anchors(&self, polarity: Polarity) -> Option<[HoleId; 2]> {
        let pr = self.power_rails.as_ref()?;
        let anchor = |strip: &PowerStrip| {
            let rail = strip.rows.iter().find(|rail| rail.polarity == polarity)?;
            let x = rail.columns().max()?;
            self.at(x, rail.y)
        };
        Some([anchor(&pr.top)?, anchor(&pr.bottom)?])
    }

    fn rebuild_effective_rail_ids(&mut self) {
        let mut parents: Vec<u32> = (0..self.effective_rail_ids.len() as u32).collect();

        fn find(parents: &mut [u32], mut node: u32) -> u32 {
            while parents[node as usize] != node {
                let parent = parents[node as usize];
                parents[node as usize] = parents[parent as usize];
                node = parents[node as usize];
            }
            node
        }

        for tie in &self.rail_ties {
            let from = self.rail_id_of(tie.from);
            let to = self.rail_id_of(tie.to);
            let from_root = find(&mut parents, from);
            let to_root = find(&mut parents, to);
            if from_root != to_root {
                let representative = from_root.min(to_root);
                let other = from_root.max(to_root);
                parents[other as usize] = representative;
            }
        }
        for island in 0..parents.len() {
            self.effective_rail_ids[island] = find(&mut parents, island as u32);
        }
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
    /// `binding` 中存在的 `NetId` 必须有效 (即 `< circuit.nets().len()`),
    /// 否则 cost / 路由时会静默忽略 (找不到 net)。
    pub fn with_power_rail_binding(mut self, binding: PowerRailBinding) -> Self {
        self.power_rail_bindings = Some(PowerRailBindings::mirrored(binding));
        self.power_rail_binding = Some(binding);
        self
    }

    /// 分别设置上、下两组电源轨绑定。
    pub fn with_power_rail_bindings(mut self, bindings: PowerRailBindings) -> Self {
        for (polarity_name, top, bottom) in [
            ("negative", bindings.top.negative, bindings.bottom.negative),
            ("positive", bindings.top.positive, bindings.bottom.positive),
        ] {
            if top != bottom {
                self.remove_rail_tie(&format!("preset:{polarity_name}:top-bottom"));
            }
        }
        self.power_rail_binding = (bindings.top == bindings.bottom).then_some(bindings.top);
        self.power_rail_bindings = Some(bindings);
        self
    }

    /// 当前是否设置了电源轨绑定。
    pub fn power_rail_binding(&self) -> Option<&PowerRailBinding> {
        self.power_rail_binding.as_ref()
    }

    /// 当前逐物理电源轨设置的绑定。
    pub fn power_rail_bindings(&self) -> Option<&PowerRailBindings> {
        self.power_rail_bindings.as_ref()
    }

    /// 返回所有已绑定物理电源轨的 anchor 与网络，并按 effective rail 去重。
    pub fn bound_power_rail_anchors(&self) -> Vec<(HoleId, NetId)> {
        let Some(bindings) = self.power_rail_bindings() else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (side, polarity, net) in bindings.iter() {
            let Some(anchor) = self.power_rail_anchor_on(side, polarity) else {
                continue;
            };
            if seen.insert((self.effective_rail_id_of(anchor), net)) {
                result.push((anchor, net));
            }
        }
        result
    }

    /// 给定极性, 返回该 rail 上的一个 anchor `HoleId` (用作虚拟 pin 位置)。
    ///
    /// 选 top strip 里极性匹配那一行**构造时第一个插入**的孔。此方法只为兼容
    /// 需要单个展示位置的调用者；需要表达逐 island binding 时应使用
    /// [`Self::power_rail_anchors`]。
    /// 返回 `None` 表示: 没装 power rail, 或该极性在配置里不存在。
    pub fn power_rail_anchor(&self, polarity: Polarity) -> Option<HoleId> {
        let pr = self.power_rails.as_ref()?;
        let target_y = pr.top.rows.iter().find(|r| r.polarity == polarity)?.y;
        self.holes
            .iter()
            .find(|h| h.position.y == target_y)
            .map(|h| h.id)
    }

    /// 给定极性，返回 top/bottom 两条独立 power-rail island 的稳定 anchor。
    pub fn power_rail_anchors(&self, polarity: Polarity) -> Option<[HoleId; 2]> {
        Some([
            self.power_rail_anchor_on(PowerRailSide::Top, polarity)?,
            self.power_rail_anchor_on(PowerRailSide::Bottom, polarity)?,
        ])
    }

    /// 返回某一侧、某一极性物理电源轨的稳定 anchor。
    pub fn power_rail_anchor_on(&self, side: PowerRailSide, polarity: Polarity) -> Option<HoleId> {
        let pr = self.power_rails.as_ref()?;
        let strip = match side {
            PowerRailSide::Top => &pr.top,
            PowerRailSide::Bottom => &pr.bottom,
        };
        let target_y = strip.rows.iter().find(|rail| rail.polarity == polarity)?.y;
        self.holes
            .iter()
            .find(|hole| hole.position.y == target_y)
            .map(|hole| hole.id)
    }

    /// 给定一个 hole, 返回它的 rail_id。
    pub fn rail_id_of(&self, id: HoleId) -> u32 {
        self.holes[id.0].rail_id
    }

    /// 给定孔，返回它在当前 RailTie 闭包中的 effective component id。
    pub fn effective_rail_id_of(&self, id: HoleId) -> u32 {
        self.effective_rail_ids[self.rail_id_of(id) as usize]
    }

    pub fn conductive_island_of(&self, id: HoleId) -> ConductiveIslandId {
        ConductiveIslandId(self.rail_id_of(id))
    }

    /// 热路径辅助: 一次 `at + rail_id_of` 合并。越界 / blocked / gap 返 `u32::MAX`。
    /// 避免 cost_fast 里两次数组查找 + 两次分支。
    #[inline]
    pub fn rail_id_at(&self, x: i32, y: i32) -> u32 {
        if x < 0 || x >= self.cols as i32 || y < self.at_y_min || y > self.at_y_max {
            return u32::MAX;
        }
        let idx = ((y - self.at_y_min) as usize) * self.cols + (x as usize);
        // 安全: idx 在 Vec 范围内 (by bounds check above)
        unsafe {
            self.at_grid
                .get_unchecked(idx)
                .map(|h| self.holes.get_unchecked(h.0).rail_id)
                .unwrap_or(u32::MAX)
        }
    }

    /// `rail_id_at` 的 effective-connectivity 版本。
    #[inline]
    pub fn effective_rail_id_at(&self, x: i32, y: i32) -> u32 {
        let island = self.rail_id_at(x, y);
        if island == u32::MAX {
            u32::MAX
        } else {
            self.effective_rail_ids[island as usize]
        }
    }

    /// `rail_id` 总数 (= `max(rail_id) + 1`)。可作 flat `Vec` 索引的容量上限。
    pub fn num_rails(&self) -> usize {
        self.holes
            .iter()
            .map(|h| h.rail_id as usize + 1)
            .max()
            .unwrap_or(0)
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

/// 保留上方两条电源轨的定义，同时移除下方两条轨的所有孔位。
fn top_power_rails_only(mut rails: PowerRails) -> PowerRails {
    for rail in &mut rails.bottom.rows {
        rail.groups.clear();
    }
    rails
}

/// 默认电源轨配置: `cols` 参数化; 按 6-col 节拍生成 (5 连续孔 + 1 个无插孔位置)。
/// `cols=30` 时 5 组 5 孔; `cols=50` 时 9 组 (最后一组 2 孔); `cols=60` 时 10 组。
/// y 坐标固定为 -4 / -3 (top) 和 14 / 15 (bottom)。
///
/// 排布 (相对 main board, y 从小到大):
/// - y=-4: top negative
/// - y=-3: top positive
/// - y=14: bottom negative
/// - y=15: bottom positive
///
/// 主区 (y=0..11) 到 rail 之间各有 2 行 gap (y=-2,-1 和 y=12,13), 不可插线,
/// 跟中央通道同款。
///
/// 每条完整电源轨行是独立 conductive island；preset 构造器另行物化两条
/// top/bottom RailTie。
pub fn standard_power_rails(cols: i32) -> PowerRails {
    // 按 6-col 节拍重复 (5 个连续孔 + 1 个无插孔位置): 0-4, 6-10, 12-16, ...
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
        positive_names: vec![
            "VCC".into(),
            "+5V".into(),
            "5V".into(),
            "+12V".into(),
            "12V".into(),
            "3V3".into(),
        ],
        negative_names: vec!["GND".into()],
    }
}

/// 800 预设的电源轨: 左右各留 2 格空, 中间按 6-col 节拍排 5-孔 group
/// (e.g. cols=63: x=2..60 排 10 个 5-孔 group, 间隔 1 空)。
/// 与 [`standard_power_rails`] 唯一区别: 这里从 `start = 2` 开始, 终点上限是 `cols - 3`。
pub fn wide_power_rails_830(cols: i32) -> PowerRails {
    assert!(cols >= 4, "830 preset 需要 cols >= 4 (左右各留 2 格)");
    let margin: i32 = 2;
    let mut groups: Vec<RangeInclusive<i32>> = Vec::new();
    let mut start = margin;
    while start < cols - margin {
        let end = (start + 4).min(cols - margin - 1);
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
        positive_names: vec![
            "VCC".into(),
            "+5V".into(),
            "5V".into(),
            "+12V".into(),
            "12V".into(),
            "3V3".into(),
        ],
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
        assert_eq!(ids.len(), 25, "physical island 只包含当前完整电源轨行");
    }

    #[test]
    fn power_rows_are_distinct_islands_without_explicit_ties() {
        let b = Breadboard::with_power_rails(30, 12, [5, 6], standard_power_rails(30));
        let top_left = b.at(0, -4).unwrap();
        let top_across_group_gap = b.at(10, -4).unwrap();
        let bottom_same_polarity = b.at(0, 14).unwrap();

        let connected: std::collections::HashSet<_> =
            b.connected_to(top_left).into_iter().collect();

        assert!(
            connected.contains(&top_across_group_gap),
            "同一电源轨行跨 5 孔 group 必须天然导通"
        );
        assert!(
            !connected.contains(&bottom_same_polarity),
            "没有显式 RailTie 时 top/bottom 必须是独立 conductive islands"
        );
        assert_eq!(connected.len(), 25, "单条 400 preset 电源轨行有 25 个孔");
    }

    #[test]
    fn preset_power_rail_ties_connect_top_and_bottom_effectively() {
        let b = board_full();
        let top_neg = b.at(0, -4).unwrap();
        let bot_neg = b.at(0, 14).unwrap();
        let physical: std::collections::HashSet<_> = b.connected_to(top_neg).into_iter().collect();
        let effective: std::collections::HashSet<_> =
            b.effectively_connected_to(top_neg).into_iter().collect();

        assert!(!physical.contains(&bot_neg));
        assert!(effective.contains(&bot_neg));
        assert_eq!(physical.len(), 25);
        assert_eq!(effective.len(), 50);
        assert_eq!(b.rail_ties().len(), 2);
        assert_eq!(b.rail_ties()[0].key, "preset:negative:top-bottom");
        assert_eq!(b.rail_ties()[1].key, "preset:positive:top-bottom");
        assert_eq!(b.rail_ties()[0].source, RailTieSource::Preset);
    }

    #[test]
    fn preset_power_rail_ties_use_the_rightmost_available_column() {
        for board in [Preset::Hole400.make(30), Preset::Hole830.make(63)] {
            let rightmost = board.power_rails().unwrap().top.rows[0]
                .columns()
                .max()
                .unwrap();

            assert_eq!(board.rail_ties().len(), 2);
            for tie in board.rail_ties() {
                assert_eq!(board.hole(tie.from).position.x, rightmost);
                assert_eq!(board.hole(tie.to).position.x, rightmost);
            }
        }
    }

    #[test]
    fn preset_830_has_exactly_two_top_bottom_ties() {
        let b = Breadboard::preset_830(63);
        assert_eq!(b.rail_ties().len(), 2);
        for polarity in [Polarity::Negative, Polarity::Positive] {
            let [top, bottom] = b.power_rail_anchors(polarity).unwrap();
            assert_ne!(b.conductive_island_of(top), b.conductive_island_of(bottom));
            assert_eq!(b.effective_rail_id_of(top), b.effective_rail_id_of(bottom));
        }
    }

    #[test]
    fn removing_and_restoring_a_tie_updates_effective_connectivity() {
        let mut b = Breadboard::standard();
        let [top, bottom] = b.power_rail_anchors(Polarity::Negative).unwrap();
        assert_eq!(b.effective_rail_id_of(top), b.effective_rail_id_of(bottom));

        let removed = b
            .remove_rail_tie("preset:negative:top-bottom")
            .expect("negative preset tie");
        assert_ne!(b.effective_rail_id_of(top), b.effective_rail_id_of(bottom));

        let id = b
            .add_user_rail_tie("user:negative:top-bottom", top, bottom, None)
            .expect("restored user tie");
        assert_eq!(b.effective_rail_id_of(top), b.effective_rail_id_of(bottom));
        assert_eq!(b.rail_tie_at(top), Some(id));
        assert_eq!(removed.source, RailTieSource::Preset);
        assert_eq!(
            b.rail_ties()
                .iter()
                .find(|tie| tie.id == id)
                .unwrap()
                .source,
            RailTieSource::User
        );
    }

    #[test]
    fn rail_tie_rejects_same_island_and_duplicate_endpoints() {
        let mut b = Breadboard::with_power_rails(30, 12, [5, 6], standard_power_rails(30));
        let top_a = b.at(0, -4).unwrap();
        let top_b = b.at(10, -4).unwrap();
        let bottom = b.at(0, 14).unwrap();
        assert!(matches!(
            b.add_user_rail_tie("invalid", top_a, top_b, None),
            Err(RailTieError::SameIsland { .. })
        ));
        b.add_user_rail_tie("valid", top_a, bottom, None).unwrap();
        assert!(matches!(
            b.add_user_rail_tie("duplicate", top_a, b.at(0, -3).unwrap(), None),
            Err(RailTieError::DuplicateEndpoint { hole }) if hole == top_a
        ));
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
            positive: Some(NetId(0)),
            negative: Some(NetId(1)),
        };
        let b = Breadboard::standard().with_power_rail_binding(binding);
        let got = b.power_rail_binding().unwrap();
        assert_eq!(got.positive, Some(NetId(0)));
        assert_eq!(got.negative, Some(NetId(1)));
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

    // ============================================================
    //  预设板 (preset_170 / preset_400 / preset_830)
    // ============================================================

    #[test]
    fn preset_170_default_17_cols_has_170_holes() {
        let b = Breadboard::preset_170(17);
        assert_eq!(b.cols(), 17);
        assert_eq!(b.main_rows(), 12);
        assert_eq!(b.blocked_rows(), vec![5, 6]);
        assert!(b.power_rails().is_none());
        assert_eq!(b.len(), 170); // 17 × (12-2)
    }

    #[test]
    fn preset_170_with_20_cols_has_200_holes() {
        let b = Breadboard::preset_170(20);
        assert_eq!(b.cols(), 20);
        assert_eq!(b.len(), 200); // 20 × 10
    }

    #[test]
    fn preset_400_default_30_cols_has_400_holes() {
        let b = Breadboard::preset_400(30);
        assert_eq!(b.cols(), 30);
        assert_eq!(b.main_rows(), 12);
        assert_eq!(b.blocked_rows(), vec![5, 6]);
        assert!(b.power_rails().is_some());
        assert_eq!(b.len(), 400); // 300 main + 100 rail (5组×5×4)
    }

    #[test]
    fn preset_400_with_50_cols_scales_rails() {
        // 50 cols 上 6-col 节拍能排 8 个完整 5-孔 group + 1 个裁短的 2-孔 group = 42 孔/行
        let b = Breadboard::preset_400(50);
        assert_eq!(b.cols(), 50);
        assert_eq!(b.len(), 500 + 168); // 50×10 main + 4×42 rail
    }

    #[test]
    fn preset_830_default_63_cols_has_2_col_margins() {
        let b = Breadboard::preset_830(63);
        assert_eq!(b.cols(), 63);
        assert_eq!(b.main_rows(), 12);
        assert_eq!(b.blocked_rows(), vec![5, 6]);
        let pr = b.power_rails().unwrap();
        // 左边 2 空 (x=0,1 不在 power rail 里)
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            assert!(!rail.contains(0));
            assert!(!rail.contains(1));
            assert!(!rail.contains(62));
            assert!(!rail.contains(61));
            assert!(rail.contains(2));
            assert!(rail.contains(60));
        }
        // 63 × 10 main + 4 × 50 rail = 830
        assert_eq!(b.len(), 830);
    }

    #[test]
    fn preset_830_with_50_cols_still_has_margins() {
        let b = Breadboard::preset_830(50);
        assert_eq!(b.cols(), 50);
        let pr = b.power_rails().unwrap();
        for rail in pr.top.rows.iter().chain(pr.bottom.rows.iter()) {
            assert!(!rail.contains(0));
            assert!(!rail.contains(1));
        }
    }

    #[test]
    #[should_panic(expected = "830 preset 需要 cols >= 4")]
    fn preset_830_panics_with_cols_too_small() {
        let _ = Breadboard::preset_830(3);
    }

    // ============================================================
    //  Preset 枚举 + board.positive_names / board.negative_names
    // ============================================================

    #[test]
    fn preset_enum_dispatches_to_right_constructor() {
        assert_eq!(Preset::Hole170.make(17).len(), 170);
        assert_eq!(Preset::Hole400.make(30).len(), 400);
        assert_eq!(Preset::Hole830.make(63).len(), 830);
        // 改 cols 走同一预设仍 OK
        assert_eq!(Preset::Hole400.make(50).len(), 668);
        // wide_power_rails_830(50): 7 组 5 孔 + 1 组 4 孔 = 39 孔/行 × 4 行 = 156 rail
        assert_eq!(Preset::Hole830.make(50).len(), 500 + 156);
    }

    #[test]
    fn preset_default_cols_match_naming() {
        assert_eq!(Preset::Hole170.default_cols(), 17);
        assert_eq!(Preset::Hole400.default_cols(), 30);
        assert_eq!(Preset::Hole830.default_cols(), 63);
        assert_eq!(Preset::Hole170.inter_board_gap_cols(), 2);
        assert_eq!(Preset::Hole400.inter_board_gap_cols(), 3);
        assert_eq!(Preset::Hole830.inter_board_gap_cols(), 3);
    }

    #[test]
    fn repeated_830_preset_restarts_physical_rail_margins_on_each_board() {
        let board = Preset::Hole830.make_repeated(2);
        let rail = &board.power_rails().unwrap().top.rows[0];

        assert!(rail.contains(60));
        assert!(!rail.contains(61));
        assert!(!rail.contains(62));
        assert!(!rail.contains(63));
        assert!(!rail.contains(64));
        assert!(!rail.contains(65));
        assert!(!rail.contains(66));
        assert!(!rail.contains(67));
        assert!(rail.contains(68));
        assert!(rail.contains(126));
        assert!(!rail.contains(127));
        assert!(!rail.contains(128));
        assert!(board.at(62, 0).is_some());
        assert!(board.at(63, 0).is_none());
        assert!(board.at(64, 0).is_none());
        assert!(board.at(65, 0).is_none());
        assert!(board.at(66, 0).is_some());
        assert_eq!(board.cols(), 129);
    }

    #[test]
    fn repeated_presets_leave_their_configured_unusable_columns_between_boards() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole830] {
            let board_cols = preset.default_cols();
            let gap_cols = preset.inter_board_gap_cols();
            let board = preset.make_repeated(2);

            assert_eq!(board.cols(), board_cols * 2 + gap_cols);
            for x in board_cols..board_cols + gap_cols {
                for y in 0..board.main_rows() {
                    assert!(board.at(x as i32, y as i32).is_none());
                }
                if let Some(power_rails) = board.power_rails() {
                    for rail in power_rails
                        .top
                        .rows
                        .iter()
                        .chain(power_rails.bottom.rows.iter())
                    {
                        assert!(board.at(x as i32, rail.y).is_none());
                    }
                }
            }
            assert!(board.at((board_cols - 1) as i32, 0).is_some());
            assert!(board.at((board_cols + gap_cols) as i32, 0).is_some());
        }
    }

    #[test]
    fn repeated_upper_half_presets_keep_only_each_top_half() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole830] {
            let board = preset.make_repeated_upper_half(3);
            let gap_cols = preset.inter_board_gap_cols();
            assert_eq!(board.cols(), preset.default_cols() * 3 + gap_cols * 2);
            assert!(board.at(0, 4).is_some());
            assert!(board.at((board.cols() - 1) as i32, 4).is_some());
            assert!(board.at(0, 7).is_none());
            assert!(board.at((board.cols() - 1) as i32, 7).is_none());
            for board_index in 1..3 {
                let gap_start = board_index * preset.default_cols() + (board_index - 1) * gap_cols;
                for x in gap_start..gap_start + gap_cols {
                    assert!(board.at(x as i32, 4).is_none());
                }
            }
            assert!(board.rail_ties().is_empty());
        }
    }

    #[test]
    fn preset_name_is_stable_label() {
        assert_eq!(Preset::Hole170.name(), "170");
        assert_eq!(Preset::Hole400.name(), "400");
        assert_eq!(Preset::Hole830.name(), "800");
    }

    #[test]
    fn board_positive_negative_names_from_400() {
        let b = Breadboard::preset_400(50);
        assert_eq!(
            b.positive_names(),
            &["VCC", "+5V", "5V", "+12V", "12V", "3V3"]
        );
        assert_eq!(b.negative_names(), &["GND"]);
    }

    #[test]
    fn board_170_has_no_power_rail_names() {
        let b = Breadboard::preset_170(17);
        assert!(b.positive_names().is_empty());
        assert!(b.negative_names().is_empty());
    }

    #[test]
    fn upper_half_preset_blocks_the_entire_lower_main_region() {
        for preset in [Preset::Hole170, Preset::Hole400, Preset::Hole830] {
            let board = preset.make_upper_half(30);

            for y in 0..5 {
                assert!(board.at(0, y).is_some(), "{preset:?}: upper row {y}");
            }
            for y in 5..12 {
                assert!(board.at(0, y).is_none(), "{preset:?}: disabled row {y}");
            }
            if preset != Preset::Hole170 {
                assert!(board.at(0, 14).is_none(), "{preset:?}: lower power rail");
            }
            assert!(board.rail_ties().is_empty(), "{preset:?}");
        }
    }

    #[test]
    fn board_830_has_same_rail_names_as_400() {
        let a = Breadboard::preset_400(30);
        let b = Breadboard::preset_830(63);
        assert_eq!(a.positive_names(), b.positive_names());
        assert_eq!(a.negative_names(), b.negative_names());
    }
}
