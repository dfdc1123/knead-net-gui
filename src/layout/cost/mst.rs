//! 最小生成树 (Kruskal) 走线估算。
//!
//! 三个变体:
//! - `mst_wire_length_fast`: ≤3 pin 闭式公式, ≥4 pin 调 Kruskal
//! - `mst_wire_length_fast_kruskal`: 强制走 Kruskal
//! - `kruskal_union`: 共享给 kruskal 变体用的并查集
//!
//! 私有: 仅在 cost/ 内部使用, 不 re-export。

/// 给一个 net 的 pin 位置算 MST 总长度; 走 `mst_wire_length_fast`: ≤3 pin 用闭式公式, ≥4 pin 才走 Kruskal。
///
/// breadboard 物理距离 (短路抽象为 `rail_id`):
/// - 同 `rail_id`: **0** (面包板内部短接, 无论是 vertical rail 还是 power rail)
/// - 不同 `rail_id`: Manhattan |Δcol| + |Δrow|
///
/// 这是 wire 长度的下界 — 实际走线可能更长 (绕障碍), 但 SA 用它做优化目标。
#[cfg(test)]
pub(crate) fn mst_wire_length(pins: &[(i32, i32, u32)]) -> f64 {
    mst_wire_length_fast(&(0..pins.len()).collect::<Vec<_>>(), pins)
}

/// 快速版本: 使用 index 引用 buf.holes 而不是复制数据。
/// 对于 ≤3 pins 用直接公式, ≥4 用 Kruskal (本地向量, 不借用 buf)。
pub(super) fn mst_wire_length_fast(indices: &[usize], holes: &[(i32, i32, u32)]) -> f64 {
    let n = indices.len();
    match n {
        0..=1 => 0.0,
        2 => {
            let a = holes[indices[0]];
            let b = holes[indices[1]];
            if a.2 == b.2 {
                0.0
            } else {
                ((a.0 - b.0).abs() + (a.1 - b.1).abs()) as f64
            }
        }
        3 => {
            // 3 pins: 3 种可能的 spanning tree (选 2 条边), 取 min
            let p0 = holes[indices[0]];
            let p1 = holes[indices[1]];
            let p2 = holes[indices[2]];
            let d01 = if p0.2 == p1.2 {
                0
            } else {
                (p0.0 - p1.0).abs() + (p0.1 - p1.1).abs()
            };
            let d02 = if p0.2 == p2.2 {
                0
            } else {
                (p0.0 - p2.0).abs() + (p0.1 - p2.1).abs()
            };
            let d12 = if p1.2 == p2.2 {
                0
            } else {
                (p1.0 - p2.0).abs() + (p1.1 - p2.1).abs()
            };
            let min_d = (d01 + d02).min(d01 + d12).min(d02 + d12);
            min_d as f64
        }
        _ => {
            // 4+ pins: Kruskal with local allocations
            mst_wire_length_fast_kruskal(indices, holes)
        }
    }
}

/// Kruskal MST for ≥4 pin nets. 用栈上数组代替堆 Vec, 避免每次 malloc/free。
/// 上限 12 pin = 66 条边, 超过则退到堆路径。
pub(super) fn mst_wire_length_fast_kruskal(indices: &[usize], holes: &[(i32, i32, u32)]) -> f64 {
    const MAX_N: usize = 12;
    const MAX_E: usize = MAX_N * (MAX_N - 1) / 2;
    let n = indices.len();
    debug_assert!(n >= 4);

    let mut stack_edges: [(i32, usize, usize); MAX_E] = [(0, 0, 0); MAX_E];
    let mut edge_count: usize;

    if n <= MAX_N {
        // 快路径: 栈上数组
        edge_count = 0;
        for a in 0..n {
            let ha = holes[indices[a]];
            for b in (a + 1)..n {
                let hb = holes[indices[b]];
                let d = if ha.2 == hb.2 {
                    0
                } else {
                    (ha.0 - hb.0).abs() + (ha.1 - hb.1).abs()
                };
                stack_edges[edge_count] = (d, a, b);
                edge_count += 1;
            }
        }
    } else {
        // 慢路径: 堆 (超 12 pin 的 net 罕见)
        let mut heap_edges: Vec<(i32, usize, usize)> = Vec::with_capacity(n * (n - 1) / 2);
        for a in 0..n {
            let ha = holes[indices[a]];
            for b in (a + 1)..n {
                let hb = holes[indices[b]];
                let d = if ha.2 == hb.2 {
                    0
                } else {
                    (ha.0 - hb.0).abs() + (ha.1 - hb.1).abs()
                };
                heap_edges.push((d, a, b));
            }
        }
        heap_edges.sort_by_key(|e| e.0);
        return kruskal_union(&heap_edges, n);
    }

    // 部分排序: 只 sort 前 edge_count 条
    stack_edges[..edge_count].sort_by_key(|e| e.0);

    // Union-find 也用栈数组
    let mut parent: [usize; MAX_N] = [0; MAX_N];
    for (i, p) in parent.iter_mut().enumerate().take(n) {
        *p = i;
    }

    let mut total: i32 = 0;
    let mut edges_used = 0;
    for &(d, i, j) in &stack_edges[..edge_count] {
        let mut ri = i;
        while parent[ri] != ri {
            parent[ri] = parent[parent[ri]];
            ri = parent[ri];
        }
        let mut rj = j;
        while parent[rj] != rj {
            parent[rj] = parent[parent[rj]];
            rj = parent[rj];
        }
        if ri != rj {
            parent[ri] = rj;
            total += d;
            edges_used += 1;
            if edges_used == n - 1 {
                break;
            }
        }
    }
    total as f64
}

pub(super) fn kruskal_union(edges: &[(i32, usize, usize)], n: usize) -> f64 {
    let mut parent: Vec<usize> = (0..n).collect();
    let find = |parent: &mut Vec<usize>, mut x: usize| -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    };
    let mut total: i32 = 0;
    let mut edges_used = 0;
    for &(d, i, j) in edges {
        let ri = find(&mut parent, i);
        let rj = find(&mut parent, j);
        if ri != rj {
            parent[ri] = rj;
            total += d;
            edges_used += 1;
            if edges_used == n - 1 {
                break;
            }
        }
    }
    total as f64
}

/// 算 MST 并返回每个节点的度数 (用于拥塞惩罚)。
pub(super) fn mst_degrees(indices: &[usize], holes: &[(i32, i32, u32)]) -> Vec<usize> {
    let n = indices.len();
    let mut degree = vec![0; n];
    match n {
        0..=1 => {}
        2 => {
            degree[0] = 1;
            degree[1] = 1;
        }
        3 => {
            let p0 = holes[indices[0]];
            let p1 = holes[indices[1]];
            let p2 = holes[indices[2]];
            let d01 = if p0.2 == p1.2 {
                0
            } else {
                (p0.0 - p1.0).abs() + (p0.1 - p1.1).abs()
            };
            let d02 = if p0.2 == p2.2 {
                0
            } else {
                (p0.0 - p2.0).abs() + (p0.1 - p2.1).abs()
            };
            let d12 = if p1.2 == p2.2 {
                0
            } else {
                (p1.0 - p2.0).abs() + (p1.1 - p2.1).abs()
            };
            let mut edges = [(d01, 0usize, 1usize), (d02, 0, 2), (d12, 1, 2)];
            edges.sort_by_key(|e| e.0);
            degree[edges[0].1] += 1;
            degree[edges[0].2] += 1;
            degree[edges[1].1] += 1;
            degree[edges[1].2] += 1;
        }
        _ => {
            let mut edge_list: Vec<(i32, usize, usize)> = Vec::with_capacity(n * (n - 1) / 2);
            for a in 0..n {
                let ha = holes[indices[a]];
                for b in (a + 1)..n {
                    let hb = holes[indices[b]];
                    let d = if ha.2 == hb.2 {
                        0
                    } else {
                        (ha.0 - hb.0).abs() + (ha.1 - hb.1).abs()
                    };
                    edge_list.push((d, a, b));
                }
            }
            edge_list.sort_by_key(|e| e.0);
            let mut parent: Vec<usize> = (0..n).collect();
            for &(_, i, j) in &edge_list {
                let (mut ri, mut rj) = (i, j);
                while parent[ri] != ri {
                    parent[ri] = parent[parent[ri]];
                    ri = parent[ri];
                }
                while parent[rj] != rj {
                    parent[rj] = parent[parent[rj]];
                    rj = parent[rj];
                }
                if ri != rj {
                    parent[ri] = rj;
                    degree[i] += 1;
                    degree[j] += 1;
                }
            }
        }
    }
    degree
}
