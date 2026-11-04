//! Spatial acceleration: a uniform point hash and a triangle BVH with closest-point queries.

use crate::common::{D3, add, dot, scale, sub};
use std::collections::HashMap;

/// Uniform grid over points for fixed-radius neighbour queries.
pub(crate) struct PointGrid {
    cell: f64,
    buckets: HashMap<(i64, i64, i64), Vec<u32>>,
}

impl PointGrid {
    pub(crate) fn new(cell: f64) -> Self {
        Self {
            cell,
            buckets: HashMap::new(),
        }
    }

    fn key(&self, p: D3) -> (i64, i64, i64) {
        (
            (p[0] / self.cell).floor() as i64,
            (p[1] / self.cell).floor() as i64,
            (p[2] / self.cell).floor() as i64,
        )
    }

    pub(crate) fn insert(&mut self, index: u32, p: D3) {
        let key = self.key(p);
        self.buckets.entry(key).or_default().push(index);
    }

    /// Visits candidates in the 27 surrounding cells, in a fixed (deterministic) order.
    pub(crate) fn for_neighbours(&self, p: D3, mut visit: impl FnMut(u32)) {
        let (cx, cy, cz) = self.key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(bucket) = self.buckets.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &index in bucket {
                            visit(index);
                        }
                    }
                }
            }
        }
    }
}

/// Closest point on triangle `(a, b, c)` to `p` (Ericson, Real-Time Collision Detection 5.1.5).
pub(crate) fn closest_point_on_triangle(p: D3, a: D3, b: D3, c: D3) -> D3 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), t));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

struct BvhNode {
    low: D3,
    high: D3,
    /// Leaf: `(first triangle slot, count)`. Interior: `(left child, right child)` with count 0.
    first: u32,
    second: u32,
    count: u32,
}

/// Static bounding-volume hierarchy over triangles.
pub(crate) struct TriBvh {
    points: Vec<D3>,
    triangles: Vec<[u32; 3]>,
    order: Vec<u32>,
    nodes: Vec<BvhNode>,
}

const BVH_LEAF_SIZE: usize = 8;

impl TriBvh {
    pub(crate) fn build(points: Vec<D3>, triangles: Vec<[u32; 3]>) -> Self {
        let centroids: Vec<D3> = triangles
            .iter()
            .map(|t| {
                scale(
                    add(
                        add(points[t[0] as usize], points[t[1] as usize]),
                        points[t[2] as usize],
                    ),
                    1.0 / 3.0,
                )
            })
            .collect();
        let mut bvh = Self {
            order: (0..triangles.len() as u32).collect(),
            points,
            triangles,
            nodes: Vec::new(),
        };
        if !bvh.triangles.is_empty() {
            let count = bvh.order.len();
            bvh.build_node(0, count, &centroids);
        }
        bvh
    }

    fn build_node(&mut self, start: usize, end: usize, centroids: &[D3]) -> u32 {
        let (low, high) = self.bounds(start, end);
        let node_index = self.nodes.len() as u32;
        self.nodes.push(BvhNode {
            low,
            high,
            first: start as u32,
            second: 0,
            count: (end - start) as u32,
        });
        if end - start <= BVH_LEAF_SIZE {
            return node_index;
        }
        let extent = sub(high, low);
        let axis = if extent[0] >= extent[1] && extent[0] >= extent[2] {
            0
        } else if extent[1] >= extent[2] {
            1
        } else {
            2
        };
        let middle = (start + end) / 2;
        self.order[start..end].select_nth_unstable_by(middle - start, |&a, &b| {
            centroids[a as usize][axis]
                .total_cmp(&centroids[b as usize][axis])
                .then(a.cmp(&b))
        });
        let left = self.build_node(start, middle, centroids);
        let right = self.build_node(middle, end, centroids);
        let node = &mut self.nodes[node_index as usize];
        node.first = left;
        node.second = right;
        node.count = 0;
        node_index
    }

    fn bounds(&self, start: usize, end: usize) -> (D3, D3) {
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        for &slot in &self.order[start..end] {
            for &vertex in &self.triangles[slot as usize] {
                let p = self.points[vertex as usize];
                for (axis, &value) in p.iter().enumerate() {
                    low[axis] = low[axis].min(value);
                    high[axis] = high[axis].max(value);
                }
            }
        }
        (low, high)
    }

    fn box_distance_squared(node: &BvhNode, p: D3) -> f64 {
        let mut total = 0.0;
        for (axis, &value) in p.iter().enumerate() {
            let gap = (node.low[axis] - value)
                .max(value - node.high[axis])
                .max(0.0);
            total += gap * gap;
        }
        total
    }

    /// Closest surface point to `p`, or `p` itself when the BVH holds no triangles.
    pub(crate) fn closest_point(&self, p: D3) -> D3 {
        let mut best = p;
        let mut best_distance = f64::INFINITY;
        if self.nodes.is_empty() {
            return best;
        }
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index as usize];
            if Self::box_distance_squared(node, p) >= best_distance {
                continue;
            }
            if node.count > 0 {
                let start = node.first as usize;
                for &slot in &self.order[start..start + node.count as usize] {
                    let t = self.triangles[slot as usize];
                    let q = closest_point_on_triangle(
                        p,
                        self.points[t[0] as usize],
                        self.points[t[1] as usize],
                        self.points[t[2] as usize],
                    );
                    let gap = sub(q, p);
                    let squared = dot(gap, gap);
                    if squared < best_distance {
                        best_distance = squared;
                        best = q;
                    }
                }
            } else {
                let left = Self::box_distance_squared(&self.nodes[node.first as usize], p);
                let right = Self::box_distance_squared(&self.nodes[node.second as usize], p);
                // Push the farther child first so that the nearer one is examined next.
                if left < right {
                    stack.push(node.second);
                    stack.push(node.first);
                } else {
                    stack.push(node.first);
                    stack.push(node.second);
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::distance;
    use pcc_math::Philox4x32;

    #[test]
    fn closest_point_classifies_all_seven_voronoi_regions() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
        let cases: [(D3, D3); 7] = [
            ([-1.0, -1.0, 1.0], a),
            ([3.0, -1.0, 1.0], b),
            ([-1.0, 3.0, 1.0], c),
            ([1.0, -1.0, 1.0], [1.0, 0.0, 0.0]),
            ([-1.0, 1.0, 1.0], [0.0, 1.0, 0.0]),
            ([2.0, 2.0, 1.0], [1.0, 1.0, 0.0]),
            ([0.5, 0.5, 1.0], [0.5, 0.5, 0.0]),
        ];
        for (query, expected) in cases {
            let q = closest_point_on_triangle(query, a, b, c);
            assert!(distance(q, expected) < 1e-12, "{query:?} gave {q:?}");
        }
    }

    #[test]
    fn bvh_query_agrees_with_brute_force_on_random_soup() {
        let rng = Philox4x32::new(7);
        let mut points = Vec::new();
        let mut triangles = Vec::new();
        for i in 0..300u32 {
            let base = rng.generate_unit_f32([i, 0, 0, 0]);
            let center = [
                f64::from(base[0]) * 10.0,
                f64::from(base[1]) * 10.0,
                f64::from(base[2]) * 10.0,
            ];
            let first = points.len() as u32;
            for k in 0..3u32 {
                let offset = rng.generate_unit_f32([i, k + 1, 0, 0]);
                points.push(add(
                    center,
                    [
                        f64::from(offset[0]) - 0.5,
                        f64::from(offset[1]) - 0.5,
                        f64::from(offset[2]) - 0.5,
                    ],
                ));
            }
            triangles.push([first, first + 1, first + 2]);
        }
        let bvh = TriBvh::build(points.clone(), triangles.clone());
        for i in 0..200u32 {
            let raw = rng.generate_unit_f32([i, 99, 1, 0]);
            let query = [
                f64::from(raw[0]) * 14.0 - 2.0,
                f64::from(raw[1]) * 14.0 - 2.0,
                f64::from(raw[2]) * 14.0 - 2.0,
            ];
            let brute = triangles
                .iter()
                .map(|t| {
                    distance(
                        query,
                        closest_point_on_triangle(
                            query,
                            points[t[0] as usize],
                            points[t[1] as usize],
                            points[t[2] as usize],
                        ),
                    )
                })
                .fold(f64::INFINITY, f64::min);
            let via_bvh = distance(query, bvh.closest_point(query));
            assert!((brute - via_bvh).abs() < 1e-12, "{brute} vs {via_bvh}");
        }
    }
}
