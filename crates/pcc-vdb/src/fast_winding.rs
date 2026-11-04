//! Accelerated closest-distance and generalized winding number queries over a triangle soup.
//!
//! A bounding volume hierarchy stores each subtree's area-weighted centre, summed area vector, and
//! first moment. A subtree far from the query is replaced by a Taylor expansion of the dipole
//! kernel; nearby subtrees descend to exact solid angles (Barill et al., "Fast Winding Numbers for
//! Soups and Clouds", SIGGRAPH 2018).

use crate::{generalized_winding_number, point_triangle_dist_sq};
use pcc_math::Vec3;

/// A subtree is approximated when the query is farther than this many subtree radii. Larger values
/// are more accurate and slower. Barill et al. pair a ratio of 2 with one more expansion term than
/// this code has, and truncating earlier needs more distance for comparable accuracy.
const FAR_FIELD_RATIO: f64 = 3.0;
const LEAF_SIZE: usize = 8;

type Triangle = (Vec3, Vec3, Vec3);
type D3 = [f64; 3];

fn d3(v: Vec3) -> D3 {
    [f64::from(v.x), f64::from(v.y), f64::from(v.z)]
}

fn sub(a: D3, b: D3) -> D3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: D3, b: D3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: D3, b: D3) -> D3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

struct Node {
    low: [f32; 3],
    high: [f32; 3],
    centre: D3,
    radius: f64,
    /// Sum of triangle area vectors (normal times area) in this subtree.
    area_vector: D3,
    /// Sum over triangles of `area_vector[i] * (centroid[j] - centre[j])`.
    moment: [D3; 3],
    /// Leaf: first triangle and count. Interior: left and right child, with count 0.
    first: u32,
    second: u32,
    count: u32,
}

/// Winding contribution of a distant subtree, with `r` from the query to the subtree centre. This
/// expands the kernel `G(r) = r / (4 pi |r|^3)` to first order about the centre:
/// `a . G(r) + a_i (d/dr_j G_i(r)) (c_j - centre_j)`, summed over the subtree's triangles.
fn far_field(node: &Node, r: D3, distance_squared: f64) -> f64 {
    let distance = distance_squared.sqrt();
    let inverse_cube = 1.0 / (distance_squared * distance);
    let mut value = dot(node.area_vector, r) * inverse_cube;
    let inverse_fifth = inverse_cube / distance_squared;
    for (i, row) in node.moment.iter().enumerate() {
        for (j, &cell) in row.iter().enumerate() {
            let identity = if i == j { inverse_cube } else { 0.0 };
            value += (identity - 3.0 * r[i] * r[j] * inverse_fifth) * cell;
        }
    }
    value / (4.0 * std::f64::consts::PI)
}

/// Immutable query structure built once per voxelization.
pub(crate) struct MeshField {
    triangles: Vec<Triangle>,
    nodes: Vec<Node>,
}

impl MeshField {
    pub(crate) fn build(triangles: Vec<Triangle>) -> Self {
        let mut field = Self {
            triangles,
            nodes: Vec::new(),
        };
        if !field.triangles.is_empty() {
            let count = field.triangles.len();
            field.build_node(0, count);
        }
        field
    }

    fn build_node(&mut self, start: usize, end: usize) -> u32 {
        let index = self.nodes.len() as u32;
        let mut low = [f32::INFINITY; 3];
        let mut high = [f32::NEG_INFINITY; 3];
        let mut area_vector = [0.0; 3];
        let mut weighted = [0.0; 3];
        let mut area_sum = 0.0;
        let mut plain = [0.0; 3];
        for &(a, b, c) in &self.triangles[start..end] {
            for corner in [a, b, c] {
                for (axis, value) in [corner.x, corner.y, corner.z].into_iter().enumerate() {
                    low[axis] = low[axis].min(value);
                    high[axis] = high[axis].max(value);
                }
            }
            let (pa, pb, pc) = (d3(a), d3(b), d3(c));
            let normal = cross(sub(pb, pa), sub(pc, pa));
            let area = dot(normal, normal).sqrt() * 0.5;
            for axis in 0..3 {
                let centroid = (pa[axis] + pb[axis] + pc[axis]) / 3.0;
                area_vector[axis] += normal[axis] * 0.5;
                weighted[axis] += centroid * area;
                plain[axis] += centroid;
            }
            area_sum += area;
        }
        // All-degenerate subtrees have no area weights, and also no dipole moment to place.
        let count = (end - start) as f64;
        let centre = if area_sum > 0.0 {
            [
                weighted[0] / area_sum,
                weighted[1] / area_sum,
                weighted[2] / area_sum,
            ]
        } else {
            [plain[0] / count, plain[1] / count, plain[2] / count]
        };
        let mut radius_squared = 0.0f64;
        let mut moment = [[0.0; 3]; 3];
        for &(a, b, c) in &self.triangles[start..end] {
            let (pa, pb, pc) = (d3(a), d3(b), d3(c));
            let normal = cross(sub(pb, pa), sub(pc, pa));
            for corner in [pa, pb, pc] {
                let gap = sub(corner, centre);
                radius_squared = radius_squared.max(dot(gap, gap));
            }
            for (i, row) in moment.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    let centroid = (pa[j] + pb[j] + pc[j]) / 3.0;
                    *cell += normal[i] * 0.5 * (centroid - centre[j]);
                }
            }
        }
        self.nodes.push(Node {
            low,
            high,
            centre,
            radius: radius_squared.sqrt(),
            area_vector,
            moment,
            first: start as u32,
            second: 0,
            count: (end - start) as u32,
        });
        if end - start <= LEAF_SIZE {
            return index;
        }
        let extent = [high[0] - low[0], high[1] - low[1], high[2] - low[2]];
        let axis = if extent[0] >= extent[1] && extent[0] >= extent[2] {
            0
        } else if extent[1] >= extent[2] {
            1
        } else {
            2
        };
        let key = |t: &Triangle| match axis {
            0 => t.0.x + t.1.x + t.2.x,
            1 => t.0.y + t.1.y + t.2.y,
            _ => t.0.z + t.1.z + t.2.z,
        };
        let middle = (start + end) / 2;
        self.triangles[start..end]
            .select_nth_unstable_by(middle - start, |a, b| key(a).total_cmp(&key(b)));
        let left = self.build_node(start, middle);
        let right = self.build_node(middle, end);
        let node = &mut self.nodes[index as usize];
        node.first = left;
        node.second = right;
        node.count = 0;
        index
    }

    fn box_distance_squared(node: &Node, p: Vec3) -> f32 {
        let mut total = 0.0;
        for (axis, value) in [p.x, p.y, p.z].into_iter().enumerate() {
            let gap = (node.low[axis] - value)
                .max(value - node.high[axis])
                .max(0.0);
            total += gap * gap;
        }
        total
    }

    /// Squared distance to the nearest triangle, or `limit` when none is closer than that.
    pub(crate) fn distance_squared_within(&self, p: Vec3, limit: f32) -> f32 {
        let mut best = limit;
        if self.nodes.is_empty() {
            return best;
        }
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index as usize];
            if Self::box_distance_squared(node, p) >= best {
                continue;
            }
            if node.count > 0 {
                let start = node.first as usize;
                for &(a, b, c) in &self.triangles[start..start + node.count as usize] {
                    best = best.min(point_triangle_dist_sq(p, a, b, c));
                }
            } else {
                let left = Self::box_distance_squared(&self.nodes[node.first as usize], p);
                let right = Self::box_distance_squared(&self.nodes[node.second as usize], p);
                // Push the farther child first so that the nearer one tightens the bound sooner.
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

    /// Generalized winding number at `p`, adding the exact triangle evaluations to `exact_work`.
    fn winding_counted(&self, p: Vec3, exact_work: &mut usize) -> f32 {
        if self.nodes.is_empty() {
            return 0.0;
        }
        let query = d3(p);
        let mut total = 0.0f64;
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index as usize];
            let to_centre = sub(node.centre, query);
            let distance_squared = dot(to_centre, to_centre);
            let reach = FAR_FIELD_RATIO * node.radius;
            if distance_squared > reach * reach {
                total += far_field(node, to_centre, distance_squared);
            } else if node.count > 0 {
                let start = node.first as usize;
                let leaf = &self.triangles[start..start + node.count as usize];
                total += f64::from(generalized_winding_number(p, leaf));
                *exact_work += leaf.len();
            } else {
                stack.push(node.second);
                stack.push(node.first);
            }
        }
        total as f32
    }

    pub(crate) fn winding(&self, p: Vec3) -> f32 {
        let mut ignored = 0;
        self.winding_counted(p, &mut ignored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract_triangles;
    use pcc_geo::GeoError;
    use pcc_math::Philox4x32;
    use pcc_sop::{SopContext, create_sphere, create_torus, create_tube};
    use std::sync::atomic::AtomicBool;

    fn random_point(rng: &Philox4x32, i: u32, span: f32) -> Result<Vec3, GeoError> {
        let u = rng.generate_unit_f32([i, 0, 0, 0]);
        Ok(Vec3::checked(
            (u[0] - 0.5) * span,
            (u[1] - 0.5) * span,
            (u[2] - 0.5) * span,
        )?)
    }

    fn brute_distance_squared(p: Vec3, triangles: &[Triangle]) -> f32 {
        triangles
            .iter()
            .map(|&(a, b, c)| point_triangle_dist_sq(p, a, b, c))
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn accelerated_queries_agree_with_brute_force_on_closed_and_open_surfaces()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let rng = Philox4x32::new(99);
        let closed_sphere = create_sphere(1.0, 24, 32, &ctx)?;
        let torus = create_torus(1.2, 0.4, 32, 16, &ctx)?;
        let tube = create_tube(0.7, 1.5, 4, 24, &ctx)?;
        for geo in [&closed_sphere, &torus, &tube] {
            let triangles = extract_triangles(geo)?;
            let field = MeshField::build(triangles.clone());
            for i in 0..400 {
                let p = random_point(&rng, i, 4.0)?;
                let exact = generalized_winding_number(p, &triangles);
                let fast = field.winding(p);
                // Voxelization only needs the side of 1/2, asserted next; this bounds the rest.
                assert!((exact - fast).abs() < 0.03, "{p:?}: {exact} vs {fast}");
                assert_eq!(exact >= 0.5, fast >= 0.5, "{p:?}: {exact} vs {fast}");

                let brute = brute_distance_squared(p, &triangles);
                let found = field.distance_squared_within(p, f32::INFINITY);
                assert!(
                    (brute - found).abs() <= 1e-4 * brute.max(1.0),
                    "{brute} vs {found}"
                );
                // A bound below the true distance reports the bound itself.
                assert_eq!(field.distance_squared_within(p, brute * 0.5), brute * 0.5);
            }
        }
        Ok(())
    }

    #[test]
    fn far_field_approximation_does_a_small_fraction_of_the_exact_work() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let sphere = create_sphere(1.0, 100, 100, &ctx)?;
        let triangles = extract_triangles(&sphere)?;
        let count = triangles.len();
        let field = MeshField::build(triangles);
        let rng = Philox4x32::new(5);
        let queries = 500u32;
        let mut work = 0usize;
        for i in 0..queries {
            field.winding_counted(random_point(&rng, i, 3.0)?, &mut work);
        }
        let average = work as f64 / f64::from(queries);
        assert!(
            average < 0.05 * count as f64,
            "{average} exact evaluations per query for {count} triangles"
        );
        Ok(())
    }

    #[test]
    fn degenerate_and_empty_soups_are_handled() -> Result<(), GeoError> {
        let p = Vec3::checked(1.0, 2.0, 3.0)?;
        let empty = MeshField::build(Vec::new());
        assert_eq!(empty.winding(p), 0.0);
        assert_eq!(empty.distance_squared_within(p, 9.0), 9.0);
        // Zero-area triangles contribute no winding but still have a position.
        let point = Vec3::checked(0.0, 0.0, 0.0)?;
        let degenerate = MeshField::build(vec![(point, point, point); 40]);
        assert_eq!(degenerate.winding(p), 0.0);
        assert!((degenerate.distance_squared_within(p, f32::INFINITY) - 14.0).abs() < 1e-5);
        Ok(())
    }
}
