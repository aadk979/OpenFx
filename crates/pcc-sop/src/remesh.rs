//! Isotropic triangle remeshing (Botsch & Kobbelt 2004): split long edges, collapse short ones,
//! equalize valences by flipping, smooth tangentially, and reproject onto the input surface.

use crate::common::{OrdF64, SopContext, add, distance, dot, lerp, normalized, scale, sub};
use crate::modify::triangles_of;
use crate::spatial::TriBvh;
use crate::trimesh::TriMesh;
use pcc_geo::{GeoDetail, GeoError};
use std::collections::BTreeSet;

/// Parameters of [`remesh`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RemeshParams {
    pub target_length: f64,
    pub iterations: u32,
    /// Edges with a dihedral angle at or above this stay in place as sharp features.
    pub feature_angle_degrees: f64,
}

/// Upper limit on triangles along boundaries and features. Each split on such an edge can fan into
/// one vertex that may never collapse, making the work quadratic in this count.
const MAX_CONSTRAINED_TRIANGLES: f64 = 200_000.0;

struct Constraints {
    edges: BTreeSet<(u32, u32)>,
    locked: Vec<bool>,
}

impl Constraints {
    fn has_edge(&self, a: u32, b: u32) -> bool {
        self.edges.contains(&(a.min(b), a.max(b)))
    }

    fn insert_edge(&mut self, a: u32, b: u32) {
        self.edges.insert((a.min(b), a.max(b)));
        for v in [a, b] {
            if self.locked.len() <= v as usize {
                self.locked.resize(v as usize + 1, false);
            }
            self.locked[v as usize] = true;
        }
    }

    fn is_locked(&self, v: u32) -> bool {
        self.locked.get(v as usize).copied().unwrap_or(false)
    }
}

fn detect_constraints(mesh: &TriMesh, cos_threshold: f64) -> Constraints {
    let mut constraints = Constraints {
        edges: BTreeSet::new(),
        locked: vec![false; mesh.verts.len()],
    };
    let normal = |t: u32| {
        let tri = mesh.tris[t as usize];
        let (a, b, c) = (
            mesh.verts[tri[0] as usize],
            mesh.verts[tri[1] as usize],
            mesh.verts[tri[2] as usize],
        );
        normalized(crate::common::cross(sub(b, a), sub(c, a))).unwrap_or([0.0; 3])
    };
    for (a, b) in mesh.edges() {
        let is_feature = match mesh.edge_triangles(a, b)[..] {
            [t0, t1] => dot(normal(t0), normal(t1)) <= cos_threshold,
            _ => true,
        };
        if is_feature {
            constraints.insert_edge(a, b);
        }
    }
    constraints
}

fn longest_first(mesh: &TriMesh) -> Vec<(u32, u32)> {
    let mut edges = mesh.edges();
    edges.sort_by_key(|&(a, b)| {
        (
            std::cmp::Reverse(OrdF64(distance(
                mesh.verts[a as usize],
                mesh.verts[b as usize],
            ))),
            a,
            b,
        )
    });
    edges
}

fn split_long_edges(
    mesh: &mut TriMesh,
    constraints: &mut Constraints,
    high: f64,
    ctx: &SopContext<'_>,
) -> Result<(), GeoError> {
    for _pass in 0..32 {
        let mut changed = false;
        for (index, (a, b)) in longest_first(mesh).into_iter().enumerate() {
            ctx.poll_every(index)?;
            let (pa, pb) = (mesh.verts[a as usize], mesh.verts[b as usize]);
            if distance(pa, pb) <= high {
                break;
            }
            let was_constrained = constraints.has_edge(a, b);
            let middle = mesh.split_edge(a, b, lerp(pa, pb, 0.5));
            if was_constrained {
                constraints.insert_edge(a, middle);
                constraints.insert_edge(middle, b);
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

fn collapse_short_edges(
    mesh: &mut TriMesh,
    constraints: &Constraints,
    low: f64,
    high: f64,
    ctx: &SopContext<'_>,
) -> Result<(), GeoError> {
    let mut edges = mesh.edges();
    edges.sort_by_key(|&(a, b)| {
        (
            OrdF64(distance(mesh.verts[a as usize], mesh.verts[b as usize])),
            a,
            b,
        )
    });
    for (index, (a, b)) in edges.into_iter().enumerate() {
        ctx.poll_every(index)?;
        if !mesh.vert_alive[a as usize]
            || !mesh.vert_alive[b as usize]
            || mesh.edge_triangles(a, b).is_empty()
        {
            continue;
        }
        let (pa, pb) = (mesh.verts[a as usize], mesh.verts[b as usize]);
        if distance(pa, pb) >= low {
            continue;
        }
        let (keep, drop, position) = match (constraints.is_locked(a), constraints.is_locked(b)) {
            (true, true) => continue,
            (true, false) => (a, b, pa),
            (false, true) => (b, a, pb),
            (false, false) => (a, b, lerp(pa, pb, 0.5)),
        };
        // Reject collapses that would immediately recreate over-long edges.
        let too_long = |v: u32| {
            mesh.neighbours(v)
                .into_iter()
                .filter(|&n| n != keep && n != drop)
                .any(|n| distance(mesh.verts[n as usize], position) > high)
        };
        if too_long(keep)
            || too_long(drop)
            || !mesh.collapse_keeps_manifold(keep, drop)
            || !mesh.collapse_keeps_orientation(keep, drop, position)
        {
            continue;
        }
        mesh.collapse(keep, drop, position);
    }
    Ok(())
}

fn ideal_valence(mesh: &TriMesh, v: u32) -> i64 {
    if mesh.is_boundary_vertex(v) { 4 } else { 6 }
}

fn equalize_valences(
    mesh: &mut TriMesh,
    constraints: &Constraints,
    ctx: &SopContext<'_>,
) -> Result<(), GeoError> {
    for (index, (a, b)) in mesh.edges().into_iter().enumerate() {
        ctx.poll_every(index)?;
        if constraints.has_edge(a, b) {
            continue;
        }
        let shared = mesh.edge_triangles(a, b);
        let [t0, t1] = shared[..] else {
            continue;
        };
        let opposite = |t: u32| {
            mesh.tris[t as usize]
                .iter()
                .copied()
                .find(|&v| v != a && v != b)
        };
        let (Some(c), Some(d)) = (opposite(t0), opposite(t1)) else {
            continue;
        };
        let valence = |v: u32| mesh.neighbours(v).len() as i64;
        let deviation = |v: u32, delta: i64| (valence(v) + delta - ideal_valence(mesh, v)).abs();
        let before = deviation(a, 0) + deviation(b, 0) + deviation(c, 0) + deviation(d, 0);
        let after = deviation(a, -1) + deviation(b, -1) + deviation(c, 1) + deviation(d, 1);
        if after < before {
            mesh.flip_edge(a, b);
        }
    }
    Ok(())
}

fn smooth_and_project(
    mesh: &mut TriMesh,
    constraints: &Constraints,
    reference: &TriBvh,
    ctx: &SopContext<'_>,
) -> Result<(), GeoError> {
    let mut updates = Vec::with_capacity(mesh.verts.len());
    for v in 0..mesh.verts.len() as u32 {
        ctx.poll_every(v as usize)?;
        if !mesh.vert_alive[v as usize] || constraints.is_locked(v) {
            continue;
        }
        let neighbours = mesh.neighbours(v);
        if neighbours.is_empty() {
            continue;
        }
        let mut centroid = [0.0; 3];
        for &n in &neighbours {
            centroid = add(centroid, mesh.verts[n as usize]);
        }
        centroid = scale(centroid, 1.0 / neighbours.len() as f64);
        let position = mesh.verts[v as usize];
        let shift = sub(centroid, position);
        let tangential = match normalized(mesh.vertex_area_normal(v)) {
            Some(normal) => sub(shift, scale(normal, dot(shift, normal))),
            None => shift,
        };
        updates.push((v, add(position, scale(tangential, 0.5))));
    }
    for (v, position) in updates {
        mesh.verts[v as usize] = reference.closest_point(position);
    }
    Ok(())
}

/// Remeshes polygon surfaces into near-equilateral triangles of edge length `target_length`.
/// Mesh boundaries and feature edges keep their exact positions (they may only gain points).
/// Output is always triangles and carries only positions.
pub fn remesh(
    geo: &GeoDetail,
    params: &RemeshParams,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let RemeshParams {
        target_length,
        iterations,
        feature_angle_degrees,
    } = *params;
    if !(target_length.is_finite() && target_length > 0.0 && feature_angle_degrees.is_finite())
        || iterations == 0
        || iterations > 100
    {
        return Err(GeoError::InvalidTopology);
    }
    // An equilateral triangle of side L has area sqrt(3)/4 L^2. Doubling that estimate leaves room
    // for transient over-refinement while rejecting runaway requests before any allocation.
    let estimated_triangles =
        2.0 * f64::from(geo.surface_area()) / (3f64.sqrt() / 4.0 * target_length * target_length);
    if !estimated_triangles.is_finite() || estimated_triangles > usize::MAX as f64 / 64.0 {
        return Err(GeoError::LimitExceeded);
    }
    let estimated_triangles = estimated_triangles as usize + geo.prim_count();
    ctx.check_output(estimated_triangles, estimated_triangles.saturating_mul(6))?;

    let mut mesh = TriMesh::from_geo(geo, ctx)?;
    let (reference_points, reference_triangles) = triangles_of(geo)?;
    let reference = TriBvh::build(reference_points, reference_triangles);
    let mut constraints = detect_constraints(&mesh, feature_angle_degrees.to_radians().cos());
    // Constrained edges are split but never collapsed, so their total length also bounds the
    // output. Area alone misses slivers: almost no area, yet very long locked edges to subdivide.
    let constrained_length: f64 = constraints
        .edges
        .iter()
        .map(|&(a, b)| distance(mesh.verts[a as usize], mesh.verts[b as usize]))
        .sum();
    let constrained_triangles = 2.0 * constrained_length / target_length;
    if !constrained_triangles.is_finite() || constrained_triangles > MAX_CONSTRAINED_TRIANGLES {
        return Err(GeoError::LimitExceeded);
    }
    ctx.check_output(
        estimated_triangles.saturating_add(constrained_triangles as usize),
        estimated_triangles
            .saturating_add(constrained_triangles as usize)
            .saturating_mul(6),
    )?;
    let (low, high) = (0.8 * target_length, 4.0 / 3.0 * target_length);
    for _ in 0..iterations {
        ctx.poll()?;
        split_long_edges(&mut mesh, &mut constraints, high, ctx)?;
        collapse_short_edges(&mut mesh, &constraints, low, high, ctx)?;
        equalize_valences(&mut mesh, &constraints, ctx)?;
        smooth_and_project(&mut mesh, &constraints, &reference, ctx)?;
    }
    mesh.to_geo(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{load_points, load_polygons, signed_volume};
    use crate::primitives::create_sphere;
    use pcc_geo::{PrimKind, PrimRun, TopologyIndex};
    use pcc_math::Vec3;
    use std::f64::consts::PI;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn box_mesh(side: f32) -> Result<GeoDetail, GeoError> {
        let v = |x: f32, y: f32, z: f32| Vec3::checked(x * side, y * side, z * side);
        GeoDetail::points(vec![
            v(0., 0., 0.)?,
            v(1., 0., 0.)?,
            v(1., 1., 0.)?,
            v(0., 1., 0.)?,
            v(0., 0., 1.)?,
            v(1., 0., 1.)?,
            v(1., 1., 1.)?,
            v(0., 1., 1.)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 6,
            }],
            vec![
                0, 3, 2, 1, 4, 5, 6, 7, 0, 1, 5, 4, 2, 3, 7, 6, 0, 4, 7, 3, 1, 2, 6, 5,
            ],
        )
    }

    fn edge_lengths(geo: &GeoDetail) -> Result<Vec<f64>, GeoError> {
        let points = load_points(geo)?;
        let mut edges = BTreeSet::new();
        for polygon in load_polygons(geo)? {
            for k in 0..polygon.len() {
                let (a, b) = (polygon[k], polygon[(k + 1) % polygon.len()]);
                edges.insert((a.min(b), a.max(b)));
            }
        }
        Ok(edges
            .into_iter()
            .map(|(a, b)| distance(points[a as usize], points[b as usize]))
            .collect())
    }

    #[test]
    fn sphere_edges_converge_to_the_target_length_on_the_surface() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let sphere = create_sphere(1.0, 24, 32, &ctx)?;
        let target = 0.2;
        let out = remesh(
            &sphere,
            &RemeshParams {
                target_length: target,
                iterations: 6,
                feature_angle_degrees: 180.0,
            },
            &ctx,
        )?;
        assert!(TopologyIndex::build(&out).is_closed_manifold());
        let lengths = edge_lengths(&out)?;
        let mean = lengths.iter().sum::<f64>() / lengths.len() as f64;
        let (shortest, longest) = lengths
            .iter()
            .fold((f64::INFINITY, 0.0f64), |(lo, hi), &l| {
                (lo.min(l), hi.max(l))
            });
        assert!((mean - target).abs() < 0.2 * target, "mean {mean}");
        assert!(shortest > 0.3 * target, "shortest {shortest}");
        assert!(longest < 1.7 * target, "longest {longest}");
        let exact = 4.0 / 3.0 * PI;
        let volume_error = ((signed_volume(&out) - exact) / exact).abs();
        assert!(volume_error < 0.03, "volume error {volume_error}");
        for p in load_points(&out)? {
            assert!((dot(p, p).sqrt() - 1.0).abs() < 0.015);
        }
        // Valence equalization: most interior vertices of an isotropic mesh have valence six.
        let polygons = load_polygons(&out)?;
        let mut neighbours = vec![BTreeSet::new(); out.len()];
        for polygon in &polygons {
            for k in 0..3 {
                neighbours[polygon[k] as usize].insert(polygon[(k + 1) % 3]);
                neighbours[polygon[(k + 1) % 3] as usize].insert(polygon[k]);
            }
        }
        let regular = neighbours.iter().filter(|n| n.len() == 6).count();
        assert!(regular * 10 >= out.len() * 6, "{regular}/{}", out.len());
        Ok(())
    }

    #[test]
    fn sharp_features_survive_exactly_on_a_cube() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let cube = box_mesh(2.0)?;
        let out = remesh(
            &cube,
            &RemeshParams {
                target_length: 0.3,
                iterations: 5,
                feature_angle_degrees: 45.0,
            },
            &ctx,
        )?;
        assert!(TopologyIndex::build(&out).is_closed_manifold());
        assert!(out.prim_count() > 200);
        assert!(
            (signed_volume(&out) - 8.0).abs() < 1e-4,
            "{}",
            signed_volume(&out)
        );
        assert!((f64::from(out.surface_area()) - 24.0).abs() < 1e-3);
        assert_eq!(out.bounds(), cube.bounds());
        let points = load_points(&out)?;
        for corner in load_points(&cube)? {
            assert!(points.iter().any(|&p| distance(p, corner) == 0.0));
        }
        // Every output point stays on the cube's surface.
        for p in points {
            let on_face = p.iter().any(|&c| c.abs() < 1e-5 || (c - 2.0).abs() < 1e-5);
            assert!(on_face, "{p:?} left the surface");
        }
        Ok(())
    }

    #[test]
    fn sliver_budgets_count_locked_edge_length_not_just_area() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 30, &flag);
        // Almost no area, so the area estimate allows nearly any edge length.
        let sliver = GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(10.0, 0.0, 0.0)?,
            Vec3::checked(5.0, 1e-5, 0.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 1,
            }],
            vec![0, 1, 2],
        )?;
        let params = RemeshParams {
            target_length: 1e-4,
            iterations: 1,
            feature_angle_degrees: 45.0,
        };
        assert_eq!(
            remesh(&sliver, &params, &ctx).err(),
            Some(GeoError::LimitExceeded)
        );
        // A sensible length on the same shape still works and subdivides the boundary.
        let out = remesh(
            &sliver,
            &RemeshParams {
                target_length: 0.5,
                ..params
            },
            &ctx,
        )?;
        assert!(out.len() > 30, "{}", out.len());
        Ok(())
    }

    #[test]
    fn determinism_budget_and_cancellation() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 26, &flag);
        let cube = box_mesh(1.0)?;
        let params = RemeshParams {
            target_length: 0.25,
            iterations: 3,
            feature_angle_degrees: 45.0,
        };
        assert_eq!(
            remesh(&cube, &params, &ctx)?.hash(),
            remesh(&cube, &params, &ctx)?.hash()
        );
        let runaway = RemeshParams {
            target_length: 1e-4,
            ..params
        };
        assert_eq!(
            remesh(&cube, &runaway, &ctx).err(),
            Some(GeoError::LimitExceeded)
        );
        assert!(
            remesh(
                &cube,
                &RemeshParams {
                    iterations: 0,
                    ..params
                },
                &ctx
            )
            .is_err()
        );
        flag.store(true, Ordering::Release);
        assert_eq!(
            remesh(&cube, &params, &ctx).err(),
            Some(GeoError::Cancelled)
        );
        Ok(())
    }
}
