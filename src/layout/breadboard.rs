//! 面包板的物理结构。
//!
//! 当前形态: 30 列 × 12 行矩形, 中间 2 行 (rows 5..7) 是物理占位 (面包板中央通道
//! 的简化模型)。剩下 10 行分成两段独立 rail:
//! - rail 0: rows 0..5   (上半)
//! - rail 1: rows 7..12  (下半)
//!
//! **同列的两个 rail 互相独立** — 上半和下半之间不连通 (面包板的物理事实)。
//! 之前 `connected_to` 是"同列即连通", 现在变成"同列且同 rail 才连通"。

use std::collections::BTreeSet;

use crate::circuit::Position;

/// 板上的一个孔的标识, 范围 0..non_blocked_holes。
///
/// 索引规则: `id = y_actual * cols + x`, 其中 `y_actual = y - |blocked_rows < y|`。
/// 跟 `at(x, y)` 互逆。**blocked row 没有 HoleId** — 它们不出现在 `holes` 里,
/// `at` 在那里返回 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HoleId(pub(crate) usize);

impl HoleId {
    pub fn raw(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hole {
    pub id: HoleId,
    /// 板内坐标: x = 列号, y = 行号 (含 blocked row 的坐标系, 跟 `at` 一致)
    pub position: Position,
}

#[derive(Debug, Clone)]
pub struct Breadboard {
    cols: usize,
    rows: usize,
    holes: Vec<Hole>,
    /// 物理上不可用的行 (例如面包板中央通道), 视为已被板子本身永久占用。
    /// `at(x, y)` 在这些行上返回 `None`, `holes` 不包含它们。
    blocked_rows: BTreeSet<usize>,
}

impl Breadboard {
    /// 创建一个 `cols` 列 × `rows` 行的矩形面包板, 没有 blocked row。
    ///
    /// 等价于 `Breadboard::with_blocked_rows(cols, rows, [])`。
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_blocked_rows(cols, rows, std::iter::empty())
    }

    /// 创建一个 `cols` 列 × `rows` 行的矩形面包板, 并把 `blocked_rows` 中的行
    /// 标为物理占用 (不参与布局, `at` 在那里返回 `None`)。
    ///
    /// `blocked_rows` 里的每个值必须严格 < `rows`; 否则 panic。
    pub fn with_blocked_rows(
        cols: usize,
        rows: usize,
        blocked_rows: impl IntoIterator<Item = usize>,
    ) -> Self {
        let blocked_rows: BTreeSet<usize> = blocked_rows.into_iter().collect();
        for &r in &blocked_rows {
            assert!(r < rows, "blocked row {} 越界 (rows = {})", r, rows);
        }
        let mut holes = Vec::with_capacity(cols * (rows - blocked_rows.len()));
        for y in 0..rows {
            if blocked_rows.contains(&y) {
                continue;
            }
            for x in 0..cols {
                holes.push(Hole {
                    id: HoleId(holes.len()),
                    position: Position {
                        x: x as i32,
                        y: y as i32,
                    },
                });
            }
        }
        Self {
            cols,
            rows,
            holes,
            blocked_rows,
        }
    }

    /// 标准全尺寸面包板: 30 列 × 12 行, rows 5..7 (中央 2 行) 是物理占位。
    /// 等价于 `Breadboard::with_blocked_rows(30, 12, [5, 6])`。
    pub fn standard() -> Self {
        Self::with_blocked_rows(30, 12, [5, 6])
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 总行数 (含 blocked row)。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 该 row 是否是物理占位 (blocked row)。
    pub fn is_blocked(&self, row: usize) -> bool {
        self.blocked_rows.contains(&row)
    }

    /// 所有 blocked row 的列表 (升序)。
    pub fn blocked_rows(&self) -> Vec<usize> {
        self.blocked_rows.iter().copied().collect()
    }

    /// 非 blocked 的孔数量 (= `cols * (rows - |blocked_rows|)`)。
    pub fn len(&self) -> usize {
        self.holes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.holes.is_empty()
    }

    pub fn hole(&self, id: HoleId) -> &Hole {
        &self.holes[id.0]
    }

    /// 非 blocked 的所有孔 (不包含 blocked row 的孔)。
    pub fn holes(&self) -> &[Hole] {
        &self.holes
    }

    /// 板内坐标 → HoleId, 越界或落在 blocked row 上返回 `None`。
    pub fn at(&self, x: i32, y: i32) -> Option<HoleId> {
        if x < 0 || y < 0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.cols || y >= self.rows {
            return None;
        }
        if self.blocked_rows.contains(&y) {
            return None;
        }
        // holes 里只有非 blocked 的孔, 跳过的行数 = 严格 < y 的 blocked 行数
        let n_blocked_before = self.blocked_rows.range(0..y).count();
        let y_actual = y - n_blocked_before;
        Some(HoleId(y_actual * self.cols + x))
    }

    /// 给定 y, 返回它所在 rail 的所有 y 值 (含自身)。y 在 blocked row 上返回空。
    ///
    /// "rail" = 板上连续的非 blocked 行; 一列上 blocked row 把它切成多段独立 rail。
    /// 同 rail 内的孔在面包板内部被纵向 rail 短接在一起。
    pub fn rail_rows(&self, y: i32) -> Vec<i32> {
        if y < 0 || y >= self.rows as i32 {
            return Vec::new();
        }
        let y = y as usize;
        if self.blocked_rows.contains(&y) {
            return Vec::new();
        }
        // 找该 rail 的最高行
        let mut top = y;
        while top > 0 && !self.blocked_rows.contains(&(top - 1)) {
            top -= 1;
        }
        // 往下直到 blocked row 或板边
        let mut bottom = y;
        while bottom + 1 < self.rows && !self.blocked_rows.contains(&(bottom + 1)) {
            bottom += 1;
        }
        (top..=bottom).map(|r| r as i32).collect()
    }

    /// 同一列同一 rail 的所有 HoleId, 含自身。
    ///
    /// 模型变化: 之前是"同列即连通", 现在是"同列且同 rail 才连通"。
    /// blocked row 把一列切成多段独立的 rail, 互相不连通。
    pub fn connected_to(&self, id: HoleId) -> Vec<HoleId> {
        let pos = self.hole(id).position;
        self.rail_rows(pos.y)
            .into_iter()
            .filter_map(|y| self.at(pos.x, y))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    fn full_board() -> Breadboard {
        Breadboard::standard()
    }

    #[test]
    fn new_30x5_has_150_holes() {
        let b = board();
        assert_eq!(b.len(), 150);
        assert_eq!(b.cols(), 30);
        assert_eq!(b.rows(), 5);
        assert!(b.blocked_rows().is_empty());
    }

    #[test]
    fn standard_30x12_has_300_holes() {
        let b = full_board();
        // 30 cols × 12 rows = 360, blocked 2 行 = 60 → 剩 300
        assert_eq!(b.len(), 300);
        assert_eq!(b.cols(), 30);
        assert_eq!(b.rows(), 12);
        assert_eq!(b.blocked_rows(), vec![5, 6]);
    }

    #[test]
    fn at_returns_correct_id_no_blocked() {
        let b = board();
        // id = y * cols + x
        assert_eq!(b.at(0, 0), Some(HoleId(0)));
        assert_eq!(b.at(1, 0), Some(HoleId(1)));
        assert_eq!(b.at(0, 1), Some(HoleId(30)));
        assert_eq!(b.at(29, 4), Some(HoleId(29 + 4 * 30)));
    }

    #[test]
    fn at_rejects_out_of_bounds() {
        let b = board();
        assert_eq!(b.at(-1, 0), None);
        assert_eq!(b.at(0, -1), None);
        assert_eq!(b.at(30, 0), None);
        assert_eq!(b.at(0, 5), None);
        assert_eq!(b.at(100, 100), None);
    }

    #[test]
    fn at_returns_none_for_blocked_rows() {
        let b = full_board();
        // 中间 2 行物理占位
        assert_eq!(b.at(0, 5), None);
        assert_eq!(b.at(0, 6), None);
        assert_eq!(b.at(15, 5), None);
        assert_eq!(b.at(29, 6), None);
        // 边界: row 4 是上 rail 最底, row 7 是下 rail 最顶
        assert!(b.at(0, 4).is_some());
        assert!(b.at(0, 7).is_some());
        // 越界
        assert_eq!(b.at(0, 12), None);
    }

    #[test]
    fn at_uses_skipped_indexing_through_blocked_rows() {
        let b = full_board();
        // 上 rail: y=0..5, 5 行 × 30 = 150 孔, id 0..150
        assert_eq!(b.at(0, 0), Some(HoleId(0)));
        assert_eq!(b.at(29, 0), Some(HoleId(29)));
        assert_eq!(b.at(0, 4), Some(HoleId(4 * 30)));
        assert_eq!(b.at(29, 4), Some(HoleId(4 * 30 + 29)));
        // 下 rail: 跳过 2 行, y=7..12
        assert_eq!(b.at(0, 7), Some(HoleId(5 * 30)));
        assert_eq!(b.at(29, 7), Some(HoleId(5 * 30 + 29)));
        assert_eq!(b.at(0, 11), Some(HoleId(9 * 30)));
        assert_eq!(b.at(29, 11), Some(HoleId(9 * 30 + 29)));
    }

    #[test]
    fn hole_position_matches_at() {
        let b = board();
        for y in 0..5 {
            for x in 0..30 {
                let id = b.at(x, y).unwrap();
                assert_eq!(b.hole(id).position, Position { x, y });
            }
        }
    }

    #[test]
    fn hole_position_matches_at_on_full_board() {
        let b = full_board();
        // 走遍所有非 blocked (x, y), id 和 position 要对得上
        for y in 0..12 {
            for x in 0..30 {
                if b.is_blocked(y) {
                    assert!(b.at(x, y as i32).is_none());
                    continue;
                }
                let id = b.at(x, y as i32).unwrap();
                assert_eq!(
                    b.hole(id).position,
                    Position {
                        x: x as i32,
                        y: y as i32
                    }
                );
            }
        }
    }

    #[test]
    fn connected_to_returns_full_column_no_blocked() {
        let b = board();
        let id = b.at(15, 2).unwrap();
        let column = b.connected_to(id);
        assert_eq!(column.len(), 5);
        for (i, hole_id) in column.iter().enumerate() {
            let pos = b.hole(*hole_id).position;
            assert_eq!(pos.x, 15);
            assert_eq!(pos.y, i as i32);
        }
    }

    #[test]
    fn connected_to_returns_only_own_rail_on_full_board() {
        let b = full_board();
        // 上半: row 2 → 同 rail 是 rows 0..5
        let upper = b.connected_to(b.at(15, 2).unwrap());
        assert_eq!(upper.len(), 5);
        for h in &upper {
            let pos = b.hole(*h).position;
            assert_eq!(pos.x, 15);
            assert!(
                pos.y >= 0 && pos.y < 5,
                "上 rail 孔应在 y=0..5, got {pos:?}"
            );
        }
        // 下半: row 10 → 同 rail 是 rows 7..12
        let lower = b.connected_to(b.at(15, 10).unwrap());
        assert_eq!(lower.len(), 5);
        for h in &lower {
            let pos = b.hole(*h).position;
            assert_eq!(pos.x, 15);
            assert!(
                pos.y >= 7 && pos.y < 12,
                "下 rail 孔应在 y=7..12, got {pos:?}"
            );
        }
    }

    #[test]
    fn connected_to_upper_and_lower_are_disjoint() {
        let b = full_board();
        let upper: std::collections::HashSet<_> =
            b.connected_to(b.at(7, 0).unwrap()).into_iter().collect();
        let lower: std::collections::HashSet<_> =
            b.connected_to(b.at(7, 11).unwrap()).into_iter().collect();
        assert!(upper.is_disjoint(&lower), "上下 rail 必须互不相连");
    }

    #[test]
    fn connected_to_is_rail_invariant() {
        let b = full_board();
        // 上 rail 内任意两点连通集相同
        let a = b.connected_to(b.at(7, 0).unwrap());
        let b_ = b.connected_to(b.at(7, 3).unwrap());
        assert_eq!(a, b_);
        // 下 rail 同理
        let c = b.connected_to(b.at(7, 7).unwrap());
        let d = b.connected_to(b.at(7, 10).unwrap());
        assert_eq!(c, d);
    }

    #[test]
    fn different_columns_not_connected() {
        let b = board();
        let a = b.connected_to(b.at(7, 0).unwrap());
        let c = b.connected_to(b.at(8, 0).unwrap());
        assert_ne!(a, c);
    }

    #[test]
    fn different_columns_not_connected_on_full_board() {
        let b = full_board();
        // 同 rail 内, 跨列 → 不同连通集
        let a = b.connected_to(b.at(7, 2).unwrap());
        let c = b.connected_to(b.at(8, 2).unwrap());
        assert_ne!(a, c);
    }

    #[test]
    fn rail_rows_returns_correct_range() {
        let b = full_board();
        assert_eq!(b.rail_rows(0), vec![0, 1, 2, 3, 4]);
        assert_eq!(b.rail_rows(2), vec![0, 1, 2, 3, 4]);
        assert_eq!(b.rail_rows(4), vec![0, 1, 2, 3, 4]);
        assert!(b.rail_rows(5).is_empty());
        assert!(b.rail_rows(6).is_empty());
        assert_eq!(b.rail_rows(7), vec![7, 8, 9, 10, 11]);
        assert_eq!(b.rail_rows(11), vec![7, 8, 9, 10, 11]);
    }

    #[test]
    fn rail_rows_rejects_out_of_bounds() {
        let b = full_board();
        assert!(b.rail_rows(-1).is_empty());
        assert!(b.rail_rows(12).is_empty());
    }

    #[test]
    fn with_blocked_rows_panics_on_out_of_range() {
        let r = std::panic::catch_unwind(|| Breadboard::with_blocked_rows(5, 5, [5]));
        assert!(r.is_err());
    }

    #[test]
    fn any_dimensions_work() {
        // 验证不是硬编码 30×5, 任意 cols × rows 都能工作
        let b = Breadboard::new(5, 30);
        assert_eq!(b.len(), 150);
        assert_eq!(b.connected_to(b.at(2, 5).unwrap()).len(), 30);
    }

    #[test]
    fn blocked_rows_in_middle_split_into_two_rails() {
        // 1 列 × 5 行, rows 1,2 blocked → 上下两段各 1 行
        let b = Breadboard::with_blocked_rows(1, 4, [1, 2]);
        assert_eq!(b.len(), 2);
        assert_eq!(b.at(0, 0), Some(HoleId(0)));
        assert_eq!(b.at(0, 3), Some(HoleId(1)));
        assert!(b.connected_to(b.at(0, 0).unwrap())[0] == b.at(0, 0).unwrap());
        assert!(b.connected_to(b.at(0, 3).unwrap())[0] == b.at(0, 3).unwrap());
    }
}
