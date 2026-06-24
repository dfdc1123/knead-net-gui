//! 接线: Wire (一段跳线) + Router trait (接线算法接口)。

use crate::circuit::{Circuit, NetId};

use super::Breadboard;
use super::breadboard::HoleId;
use super::occupancy::Occupancy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(pub(crate) usize);

impl WireId {
    pub fn raw(self) -> usize {
        self.0
    }
}

/// 一段面包板跳线。
///
/// 路径 = `[from, waypoints..., to]`, 全部 HoleId。
/// 物理上整条线插在每个 path 上的孔里 (waypoint 是拐弯点, 也被线穿过)。
#[derive(Debug, Clone)]
pub struct Wire {
    pub id: WireId,
    pub net: NetId,
    pub from: HoleId,
    pub to: HoleId,
    /// 拐弯点, 都必须是 HoleId
    pub waypoints: Vec<HoleId>,
}

impl Wire {
    /// 完整路径: from + waypoints + to, 顺序连接。
    pub fn path(&self) -> impl Iterator<Item = HoleId> + '_ {
        std::iter::once(self.from)
            .chain(self.waypoints.iter().copied())
            .chain(std::iter::once(self.to))
    }
}

/// 接线算法接口。给定一个 circuit + board + 当前占用, 返回一组 wire 满足所有 net。
pub trait Router {
    fn route(&self, circuit: &Circuit, board: &Breadboard, occupancy: &Occupancy) -> Vec<Wire>;
}
