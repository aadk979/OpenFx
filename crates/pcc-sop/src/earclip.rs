//! Ear-clipping triangulation of planar polygons, including holes through bridge edges.

pub(crate) type P2 = [f64; 2];

fn orient(a: P2, b: P2, c: P2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Signed area of a closed loop; positive when counter-clockwise.
pub(crate) fn signed_area(points: &[P2], polygon: &[usize]) -> f64 {
    let mut twice = 0.0;
    for (i, &index) in polygon.iter().enumerate() {
        let a = points[index];
        let b = points[polygon[(i + 1) % polygon.len()]];
        twice += a[0] * b[1] - b[0] * a[1];
    }
    twice * 0.5
}

/// Even-odd point-in-polygon test.
pub(crate) fn contains_point(points: &[P2], polygon: &[usize], query: P2) -> bool {
    let mut inside = false;
    for (i, &index) in polygon.iter().enumerate() {
        let a = points[index];
        let b = points[polygon[(i + 1) % polygon.len()]];
        if (a[1] > query[1]) != (b[1] > query[1]) {
            let x = a[0] + (query[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if query[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

fn inside_triangle(a: P2, b: P2, c: P2, p: P2) -> bool {
    if p == a || p == b || p == c {
        return false;
    }
    orient(a, b, p) >= 0.0 && orient(b, c, p) >= 0.0 && orient(c, a, p) >= 0.0
}

/// Triangulates a counter-clockwise loop (duplicate coordinates from hole bridges are allowed).
/// Every input corner is consumed, so a loop always yields `n - 2` triangles; on numerically
/// degenerate input with no valid ear, the flattest corner is clipped to guarantee progress.
pub(crate) fn ear_clip(points: &[P2], polygon: &[usize]) -> Vec<[usize; 3]> {
    let mut remaining = polygon.to_vec();
    let mut triangles = Vec::with_capacity(polygon.len().saturating_sub(2));
    while remaining.len() > 3 {
        let n = remaining.len();
        let corner = |i: usize| {
            (
                points[remaining[(i + n - 1) % n]],
                points[remaining[i]],
                points[remaining[(i + 1) % n]],
            )
        };
        let ear = (0..n).find(|&i| {
            let (a, b, c) = corner(i);
            orient(a, b, c) > 0.0
                && remaining
                    .iter()
                    .all(|&other| !inside_triangle(a, b, c, points[other]))
        });
        let clip = ear.unwrap_or_else(|| {
            (0..n)
                .min_by(|&x, &y| {
                    let area = |i: usize| {
                        let (a, b, c) = corner(i);
                        orient(a, b, c).abs()
                    };
                    area(x).total_cmp(&area(y))
                })
                .unwrap_or(0)
        });
        triangles.push([
            remaining[(clip + n - 1) % n],
            remaining[clip],
            remaining[(clip + 1) % n],
        ]);
        remaining.remove(clip);
    }
    if let [a, b, c] = remaining[..] {
        triangles.push([a, b, c]);
    }
    triangles
}

/// Splices clockwise `holes` into counter-clockwise `outer` through mutually visible bridges
/// (Eberly's rightmost-vertex construction), yielding one weakly simple loop.
pub(crate) fn bridge_holes(
    points: &[P2],
    outer: Vec<usize>,
    mut holes: Vec<Vec<usize>>,
) -> Vec<usize> {
    let rightmost = |hole: &[usize]| {
        hole.iter()
            .map(|&i| points[i][0])
            .fold(f64::NEG_INFINITY, f64::max)
    };
    holes.sort_by(|a, b| rightmost(b).total_cmp(&rightmost(a)));
    let mut merged = outer;
    for hole in holes {
        let start = (0..hole.len())
            .max_by(|&a, &b| points[hole[a]][0].total_cmp(&points[hole[b]][0]))
            .unwrap_or(0);
        let m = points[hole[start]];
        let bridge = find_bridge(points, &merged, m);
        let mut spliced = Vec::with_capacity(merged.len() + hole.len() + 2);
        spliced.extend_from_slice(&merged[..=bridge]);
        for k in 0..=hole.len() {
            spliced.push(hole[(start + k) % hole.len()]);
        }
        spliced.extend_from_slice(&merged[bridge..]);
        merged = spliced;
    }
    merged
}

/// Finds the index in `polygon` of a vertex visible from `m`, casting a ray toward +X.
fn find_bridge(points: &[P2], polygon: &[usize], m: P2) -> usize {
    let n = polygon.len();
    let mut best_x = f64::INFINITY;
    let mut candidate = None;
    for i in 0..n {
        let a = points[polygon[i]];
        let b = points[polygon[(i + 1) % n]];
        if (a[1] <= m[1] && m[1] <= b[1]) || (b[1] <= m[1] && m[1] <= a[1]) {
            let x = if a[1] == b[1] {
                a[0].min(b[0])
            } else {
                a[0] + (m[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
            };
            if x >= m[0] && x < best_x {
                best_x = x;
                candidate = Some(if a[0] > b[0] { i } else { (i + 1) % n });
            }
        }
    }
    let Some(endpoint) = candidate else {
        return 0;
    };
    let p = points[polygon[endpoint]];
    let hit = [best_x, m[1]];
    // The triangle (m, hit, p) may contain reflex vertices that block the bridge. The one making
    // the smallest angle with the ray is then guaranteed to be visible.
    let mut best = endpoint;
    let mut best_key = (f64::INFINITY, f64::INFINITY);
    for i in 0..n {
        let q = points[polygon[i]];
        let prev = points[polygon[(i + n - 1) % n]];
        let next = points[polygon[(i + 1) % n]];
        let reflex = orient(prev, q, next) <= 0.0;
        let in_triangle = q[0] >= m[0] && {
            let (d1, d2, d3) = (orient(m, hit, q), orient(hit, p, q), orient(p, m, q));
            !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
        };
        if i == endpoint || (reflex && in_triangle) {
            let (dx, dy) = (q[0] - m[0], q[1] - m[1]);
            let slope = if dx > 0.0 {
                dy.abs() / dx
            } else {
                f64::INFINITY
            };
            let key = (slope, dx * dx + dy * dy);
            if key < best_key {
                best_key = key;
                best = i;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_area_sum(points: &[P2], triangles: &[[usize; 3]]) -> f64 {
        triangles
            .iter()
            .map(|t| orient(points[t[0]], points[t[1]], points[t[2]]) * 0.5)
            .sum()
    }

    #[test]
    fn concave_polygon_triangulation_conserves_area_without_inversions() {
        let points = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [2.0, 1.0], [0.0, 4.0]];
        let polygon = [0, 1, 2, 3, 4];
        let triangles = ear_clip(&points, &polygon);
        assert_eq!(triangles.len(), 3);
        let expected = signed_area(&points, &polygon);
        assert!((triangle_area_sum(&points, &triangles) - expected).abs() < 1e-12);
        assert!(
            triangles
                .iter()
                .all(|t| orient(points[t[0]], points[t[1]], points[t[2]]) > 0.0)
        );
    }

    #[test]
    fn hole_bridging_subtracts_the_hole_area_exactly() {
        let points = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [3.0, 3.0],
            [3.0, 7.0],
            [7.0, 7.0],
            [7.0, 3.0],
            [8.0, 8.0],
            [8.0, 9.0],
            [9.0, 9.0],
            [9.0, 8.0],
        ];
        let merged = bridge_holes(
            &points,
            vec![0, 1, 2, 3],
            vec![vec![4, 5, 6, 7], vec![8, 9, 10, 11]],
        );
        assert_eq!(merged.len(), 4 + 4 + 4 + 2 + 2);
        let triangles = ear_clip(&points, &merged);
        assert_eq!(triangles.len(), merged.len() - 2);
        let expected = 100.0 - 16.0 - 1.0;
        assert!((triangle_area_sum(&points, &triangles) - expected).abs() < 1e-9);
        for t in &triangles {
            assert!(orient(points[t[0]], points[t[1]], points[t[2]]) >= -1e-12);
        }
    }

    #[test]
    fn point_containment_uses_even_odd_rule() {
        let points = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        let polygon = [0, 1, 2, 3];
        assert!(contains_point(&points, &polygon, [1.0, 1.0]));
        assert!(!contains_point(&points, &polygon, [3.0, 1.0]));
    }
}
