//! 面包板的物理结构。
//!
//! 当前形态: 30 列 × 5 行矩形, 每列纵向连通, 列之间不连通, 不考虑电源轨。

use crate::circuit::Position;

/// 板上的一个孔的标识, 范围 0..cols*rows。
///
/// 索引规则: `id = y * cols + x`, 跟 `at(x, y)` 互逆。
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
    /// 板内坐标: x = 列号, y = 行号
    pub position: Position,
}

#[derive(Debug, Clone)]
pub struct Breadboard {
    cols: usize,
    rows: usize,
    holes: Vec<Hole>,
}

impl Breadboard {
    /// 创建一个 `cols` 列 × `rows` 行的矩形面包板。
    ///
    /// 标准尺寸: `Breadboard::new(30, 5)`。
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut holes = Vec::with_capacity(cols * rows);
        for y in 0..rows {
            for x in 0..cols {
                let id = HoleId(y * cols + x);
                holes.push(Hole {
                    id,
                    position: Position {
                        x: x as i32,
                        y: y as i32,
                    },
                });
            }
        }
        Self { cols, rows, holes }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

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

    /// 板内坐标 → HoleId, 越界返回 `None`。
    pub fn at(&self, x: i32, y: i32) -> Option<HoleId> {
        if x < 0 || y < 0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.cols || y >= self.rows {
            return None;
        }
        Some(HoleId(y * self.cols + x))
    }

    /// 同一列所有 HoleId, 含自身。
    ///
    /// 在当前"每列纵向连通"模型下, 这就是该孔的完整电气等价集
    /// (用于 netlist 验证: 同列的 pin 自动属于同一 net)。
    pub fn connected_to(&self, id: HoleId) -> Vec<HoleId> {
        let col = id.0 % self.cols;
        (0..self.rows)
            .map(|r| HoleId(r * self.cols + col))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Breadboard {
        Breadboard::new(30, 5)
    }

    #[test]
    fn new_30x5_has_150_holes() {
        let b = board();
        assert_eq!(b.len(), 150);
        assert_eq!(b.cols(), 30);
        assert_eq!(b.rows(), 5);
    }

    #[test]
    fn at_returns_correct_id() {
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
    fn connected_to_returns_full_column() {
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
    fn connected_to_is_column_invariant() {
        let b = board();
        let id1 = b.at(7, 0).unwrap();
        let id2 = b.at(7, 3).unwrap();
        let id3 = b.at(7, 4).unwrap();
        assert_eq!(b.connected_to(id1), b.connected_to(id2));
        assert_eq!(b.connected_to(id1), b.connected_to(id3));
    }

    #[test]
    fn different_columns_not_connected() {
        let b = board();
        let a = b.connected_to(b.at(7, 0).unwrap());
        let c = b.connected_to(b.at(8, 0).unwrap());
        assert_ne!(a, c);
    }

    #[test]
    fn any_dimensions_work() {
        // 验证不是硬编码 30×5, 任意 cols × rows 都能工作
        let b = Breadboard::new(5, 30);
        assert_eq!(b.len(), 150);
        assert_eq!(b.connected_to(b.at(2, 5).unwrap()).len(), 30);
    }
}
