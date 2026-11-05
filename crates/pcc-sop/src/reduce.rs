//! Garland-Heckbert quadric error metric decimation with boundary-preserving constraint planes.

use crate::common::{D3, OrdF64, SopContext, cross, distance, dot, lerp, normalized, sub};
use crate::trimesh::TriMesh;
use pcc_geo::{GeoDetail, GeoError};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Symmetric 4x4 quadric stored as its upper triangle.
#[derive(Clone, Copy, Debug, Default)]
struct Quadric {
    a2: f64,
    ab: f64,
    ac: f64,
    ad: f64,
    b2: f64,
    bc: f64,
    bd: f64,
    c2: f64,
    cd: f64,
    d2: f64,
}

impl Quadric {
    fn from_plane(normal: D3, d: f64, weight: f64) -> Self {
        let [a, b, c] = normal;
        Self {
            a2: weight * a * a,
            ab: weight * a * b,
            ac: weight * a * c,
            ad: weight * a * d,
            b2: weight * b * b,
            bc: weight * b * c,
            bd: weight * b * d,
            c2: weight * c * c,
            cd: weight * c * d,
            d2: weight * d * d,
        }
    }

    fn add(&mut self, o: &Self) {
        self.a2 += o.a2;
        self.ab += o.ab;
        self.ac += o.ac;
        self.ad += o.ad;
        self.b2 += o.b2;
        self.bc += o.bc;
        self.bd += o.bd;
        self.c2 += o.c2;
        self.cd += o.cd;
        self.d2 += o.d2;
    }

    fn sum(a: &Self, b: &Self) -> Self {
        let mut total = *a;
        total.add(b);
        total
    }

    fn error(&self, p: D3) -> f64 {
        let [x, y, z] = p;
        x * (self.a2 * x + self.ab * y + self.ac * z + self.ad)
            + y * (self.ab * x + self.b2 * y + self.bc * z + self.bd)
            + z * (self.ac * x + self.bc * y + self.c2 * z + self.cd)
            + (self.ad * x + self.bd * y + self.cd * z + self.d2)
    }

    /// Error-minimizing position, unless the 3x3 system is too ill-conditioned to trust.
    fn optimum(&self) -> Option<D3> {
        let (a, b, c, e, f, i) = (self.a2, self.ab, self.ac, self.b2, self.bc, self.c2);
        let det = a * (e * i - f * f) - b * (b * i - f * c) + c * (b * f - e * c);
        let magnitude = (a.abs() + e.abs() + i.abs()).powi(3);
        if det.abs() <= 1e-10 * magnitude {
            return None;
        }
        let (r0, r1, r2) = (-self.ad, -self.bd, -self.cd);
        Some([
            (r0 * (e * i - f * f) - b * (r1 * i - f * r2) + c * (r1 * f - e * r2)) / det,
            (a * (r1 * i - f * r2) - r0 * (b * i - f * c) + c * (b * r2 - r1 * c)) / det,
            (a * (e * r2 - r1 * f) - b * (b * r2 - r1 * c) + r0 * (b * f - e * c)) / det,
        ])
    }
}

/// Best placement for collapsing `(a, b)`: the quadric optimum when it stays near the edge,
/// otherwise the cheapest of the endpoints and the midpoint.
fn placement(quadric: &Quadric, a: D3, b: D3) -> (D3, f64) {
    let reach = 2.0 * distance(a, b);
    if let Some(optimum) = quadric.optimum()
        && distance(optimum, lerp(a, b, 0.5)) <= reach
    {
        return (optimum, quadric.error(optimum));
    }
    let mut best = (a, quadric.error(a));
    for candidate in [b, lerp(a, b, 0.5)] {
        let error = quadric.error(candidate);
        if error < best.1 {
            best = (candidate, error);
        }
    }
    best
}

type HeapEntry = Reverse<(OrdF64, u32, u32, u32, u32)>;

fn push_edge(
    heap: &mut BinaryHeap<HeapEntry>,
    mesh: &TriMesh,
    quadrics: &[Quadric],
    versions: &[u32],
    a: u32,
    b: u32,
) {
    let (low, high) = (a.min(b), a.max(b));
    let total = Quadric::sum(&quadrics[low as usize], &quadrics[high as usize]);
    let (_, cost) = placement(&total, mesh.verts[low as usize], mesh.verts[high as usize]);
    heap.push(Reverse((
        OrdF64(cost),
        low,
        high,
        versions[low as usize],
        versions[high as usize],
    )));
}

/// Decimates to at most `target_ratio` of the triangle count where the manifold and no-flip
/// constraints allow. Output is always triangles and carries only positions.
pub fn poly_reduce(
    geo: &GeoDetail,
    target_ratio: f64,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(target_ratio.is_finite() && target_ratio > 0.0 && target_ratio <= 1.0) {
        return Err(GeoError::InvalidTopology);
    }
    let mut mesh = TriMesh::from_geo(geo, ctx)?;
    ctx.check_output(mesh.verts.len() * 8, mesh.live_tris * 12)?;
    let target = ((mesh.live_tris as f64 * target_ratio).ceil() as usize).max(4);

    let mut quadrics = vec![Quadric::default(); mesh.verts.len()];
    for (index, t) in mesh.tris.iter().enumerate() {
        ctx.poll_every(index)?;
        let (p0, p1, p2) = (
            mesh.verts[t[0] as usize],
            mesh.verts[t[1] as usize],
            mesh.verts[t[2] as usize],
        );
        let area_vector = cross(sub(p1, p0), sub(p2, p0));
        let area = dot(area_vector, area_vector).sqrt() * 0.5;
        if let Some(normal) = normalized(area_vector) {
            let plane = Quadric::from_plane(normal, -dot(normal, p0), area);
            for &v in t {
                quadrics[v as usize].add(&plane);
            }
        }
    }
    // Boundary edges get strong planes perpendicular to their faces so outlines hold their shape.
    for (index, (a, b)) in mesh.edges().into_iter().enumerate() {
        ctx.poll_every(index)?;
        let shared = mesh.edge_triangles(a, b);
        if let [only] = shared[..] {
            let t = mesh.tris[only as usize];
            let (pa, pb) = (mesh.verts[a as usize], mesh.verts[b as usize]);
            let face = cross(
                sub(mesh.verts[t[1] as usize], mesh.verts[t[0] as usize]),
                sub(mesh.verts[t[2] as usize], mesh.verts[t[0] as usize]),
            );
            let along = sub(pb, pa);
            if let Some(normal) = normalized(cross(along, face)) {
                let weight = 100.0 * dot(along, along);
                let plane = Quadric::from_plane(normal, -dot(normal, pa), weight);
                quadrics[a as usize].add(&plane);
                quadrics[b as usize].add(&plane);
            }
        }
    }

    let mut versions = vec![0u32; mesh.verts.len()];
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::new();
    for (index, (a, b)) in mesh.edges().into_iter().enumerate() {
        ctx.poll_every(index)?;
        push_edge(&mut heap, &mesh, &quadrics, &versions, a, b);
    }
    let mut steps = 0usize;
    while mesh.live_tris > target {
        steps += 1;
        ctx.poll_every(steps)?;
        let Some(Reverse((_, a, b, version_a, version_b))) = heap.pop() else {
            break;
        };
        if !mesh.vert_alive[a as usize]
            || !mesh.vert_alive[b as usize]
            || versions[a as usize] != version_a
            || versions[b as usize] != version_b
            || !mesh.collapse_keeps_manifold(a, b)
        {
            continue;
        }
        let total = Quadric::sum(&quadrics[a as usize], &quadrics[b as usize]);
        let (position, _) = placement(&total, mesh.verts[a as usize], mesh.verts[b as usize]);
        if !mesh.collapse_keeps_orientation(a, b, position) {
            continue;
        }
        mesh.collapse(a, b, position);
        quadrics[a as usize] = total;
        versions[a as usize] += 1;
        let neighbours = mesh.neighbours(a);
        for &n in &neighbours {
            versions[n as usize] += 1;
        }
        // Re-price every edge whose endpoint version just changed.
        for &n in &neighbours {
            for other in mesh.neighbours(n) {
                push_edge(&mut heap, &mesh, &quadrics, &versions, n, other);
            }
        }
    }
    mesh.to_geo(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{load_points, signed_volume};
    use crate::primitives::{create_sphere, create_torus};
    use pcc_geo::{PrimKind, PrimRun, TopologyIndex};
    use pcc_math::Vec3;
    use std::f64::consts::PI;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn grid(n: u32) -> Result<GeoDetail, GeoError> {
        let mut points = Vec::new();
        for i in 0..n {
            for j in 0..n {
                points.push(Vec3::checked(j as f32, 0.0, i as f32)?);
            }
        }
        let mut vertices = Vec::new();
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                let a = i * n + j;
                vertices.extend_from_slice(&[a, a + n, a + n + 1, a + 1]);
            }
        }
        GeoDetail::points(points).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: ((n - 1) * (n - 1)) as usize,
            }],
            vertices,
        )
    }

    #[test]
    fn sphere_decimation_stays_closed_on_the_surface_and_near_the_volume() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let sphere = create_sphere(1.0, 32, 48, &ctx)?;
        let input_triangles = 48 * 2 + 30 * 48 * 2;
        let reduced = poly_reduce(&sphere, 0.25, &ctx)?;
        assert!(TopologyIndex::build(&reduced).is_closed_manifold());
        assert!(reduced.prim_count() <= input_triangles / 4 + 1);
        assert!(reduced.prim_count() >= input_triangles / 5);
        let exact = 4.0 / 3.0 * PI;
        let error = ((signed_volume(&reduced) - exact) / exact).abs();
        assert!(error < 0.02, "volume error {error}");
        for p in load_points(&reduced)? {
            let r = dot(p, p).sqrt();
            assert!((r - 1.0).abs() < 0.02, "radius {r}");
        }
        Ok(())
    }

    #[test]
    fn flat_regions_collapse_almost_completely_without_moving_the_outline() -> Result<(), GeoError>
    {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let input = grid(17)?;
        let area = f64::from(input.surface_area());
        let reduced = poly_reduce(&input, 0.05, &ctx)?;
        assert!(reduced.prim_count() <= 26, "{}", reduced.prim_count());
        assert!((f64::from(reduced.surface_area()) - area).abs() < 1e-3);
        assert_eq!(reduced.bounds(), input.bounds());
        let topology = TopologyIndex::build(&reduced);
        assert!(topology.is_manifold());
        assert!(topology.boundary_edge_count() >= 4);
        Ok(())
    }

    #[test]
    fn genus_is_preserved_and_runs_are_deterministic() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let torus = create_torus(2.0, 0.6, 48, 24, &ctx)?;
        let a = poly_reduce(&torus, 0.1, &ctx)?;
        let b = poly_reduce(&torus, 0.1, &ctx)?;
        assert_eq!(a.hash(), b.hash());
        assert!(TopologyIndex::build(&a).is_closed_manifold());
        // Euler characteristic of a closed triangle mesh: V - F / 2. A torus has chi = 0.
        assert_eq!(a.len() as i64 - a.prim_count() as i64 / 2, 0);
        Ok(())
    }

    #[test]
    fn invalid_inputs_budgets_and_cancellation_are_reported() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let input = grid(5)?;
        assert!(poly_reduce(&input, 0.0, &ctx).is_err());
        assert!(poly_reduce(&input, 1.5, &ctx).is_err());
        assert_eq!(
            poly_reduce(&input, 0.5, &SopContext::new(8, &flag)).err(),
            Some(GeoError::LimitExceeded)
        );
        // Two faces traversing an edge in the same direction are not an oriented manifold.
        let inconsistent = GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(1.0, 0.0, 0.0)?,
            Vec3::checked(0.0, 1.0, 0.0)?,
            Vec3::checked(0.0, -1.0, 0.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 2,
            }],
            vec![0, 1, 2, 0, 1, 3],
        )?;
        assert_eq!(
            poly_reduce(&inconsistent, 0.5, &ctx).err(),
            Some(GeoError::InvalidTopology)
        );
        flag.store(true, Ordering::Release);
        assert_eq!(
            poly_reduce(&input, 0.5, &ctx).err(),
            Some(GeoError::Cancelled)
        );
        Ok(())
    }
    #[test]
    fn aggressive_decimation_never_folds_a_convex_solid() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let sphere = create_sphere(1.0, 24, 32, &ctx)?;
        for ratio in [0.05, 0.01] {
            let reduced = poly_reduce(&sphere, ratio, &ctx)?;
            assert!(TopologyIndex::build(&reduced).is_closed_manifold());
            let points = load_points(&reduced)?;
            // A sphere's decimation stays star-shaped about its centre, so every face looks away
            // from the origin; any fold would produce a face looking toward it.
            for polygon in crate::common::load_polygons(&reduced)? {
                let (a, b, c) = (
                    points[polygon[0] as usize],
                    points[polygon[1] as usize],
                    points[polygon[2] as usize],
                );
                assert!(
                    dot(cross(sub(b, a), sub(c, a)), a) > 0.0,
                    "{ratio}: {polygon:?}"
                );
            }
        }
        // Topology floors the result: a closed surface cannot go below a tetrahedron.
        let floor = poly_reduce(&sphere, 1e-6, &ctx)?;
        assert!(TopologyIndex::build(&floor).is_closed_manifold());
        assert!(floor.prim_count() >= 4);
        assert!(signed_volume(&floor) > 0.0);
        Ok(())
    }
    #[test]
    fn back_to_back_faces_are_rejected_as_not_being_a_surface() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        // Topologically a closed sphere, geometrically nothing: both faces share all three points.
        let doubled = GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(1.0, 0.0, 0.0)?,
            Vec3::checked(0.0, 1.0, 0.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 2,
            }],
            vec![0, 1, 2, 0, 2, 1],
        )?;
        assert!(TopologyIndex::build(&doubled).is_closed_manifold());
        assert!(crate::common::has_repeated_faces(&doubled));
        let invalid = Some(GeoError::InvalidTopology);
        assert_eq!(poly_reduce(&doubled, 0.5, &ctx).err(), invalid);
        let params = crate::remesh::RemeshParams {
            target_length: 0.1,
            iterations: 1,
            feature_angle_degrees: 45.0,
        };
        assert_eq!(
            crate::remesh::remesh(&doubled, &params, &ctx).err(),
            invalid
        );
        let seeds = GeoDetail::points(vec![Vec3::ZERO]);
        assert_eq!(
            crate::clip::voronoi_fracture(&doubled, &seeds, &ctx).err(),
            invalid
        );
        assert!(!crate::common::has_repeated_faces(&create_sphere(
            1.0, 8, 8, &ctx
        )?));
        Ok(())
    }
}
