//! Polyline operators: arc-length resampling and rotation-minimizing tube sweeps.

use crate::common::{
    D3, SopContext, add, any_perpendicular, build_geo, cross, distance, dot, lerp, load_points,
    load_prims, normalized, scale, sub,
};
use pcc_geo::{GeoDetail, GeoError, PrimKind};
use std::f64::consts::TAU;

/// Rebuilds every polyline with uniform spacing as close to `segment_length` as an integer
/// segment count allows; endpoints are exact. Polygons pass through. Attributes do not survive.
pub fn resample_curves(
    geo: &GeoDetail,
    segment_length: f64,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(segment_length.is_finite() && segment_length > 0.0) {
        return Err(GeoError::InvalidTopology);
    }
    let points = load_points(geo)?;
    let mut out_points: Vec<D3> = Vec::new();
    let mut out_prims = Vec::with_capacity(geo.prim_count());
    let mut polygon_remap = vec![None; points.len()];
    let mut budgeted = 0usize;
    for (prim_index, prim) in load_prims(geo)?.into_iter().enumerate() {
        ctx.poll_every(prim_index)?;
        if prim.kind == PrimKind::Polygon {
            let mut mapped = Vec::with_capacity(prim.points.len());
            for &point in &prim.points {
                let slot: &mut Option<u32> = &mut polygon_remap[point as usize];
                let index = match *slot {
                    Some(index) => index,
                    None => {
                        let index = out_points.len() as u32;
                        out_points.push(points[point as usize]);
                        *slot = Some(index);
                        index
                    }
                };
                mapped.push(index);
            }
            out_prims.push((PrimKind::Polygon, mapped));
            continue;
        }
        let path: Vec<D3> = prim.points.iter().map(|&p| points[p as usize]).collect();
        let lengths: Vec<f64> = path.windows(2).map(|w| distance(w[0], w[1])).collect();
        let total: f64 = lengths.iter().sum();
        if !total.is_finite() || total <= 0.0 {
            return Err(GeoError::InvalidTopology);
        }
        let segments = (total / segment_length).round().max(1.0);
        if segments > f64::from(u32::MAX) {
            return Err(GeoError::LimitExceeded);
        }
        let segments = segments as usize;
        budgeted = budgeted
            .checked_add(segments + 1)
            .ok_or(GeoError::LimitExceeded)?;
        ctx.check_output(budgeted, budgeted)?;
        let start = u32::try_from(out_points.len()).map_err(|_| GeoError::LimitExceeded)?;
        let mut segment = 0usize;
        let mut walked = 0.0;
        for k in 0..=segments {
            ctx.poll_every(k)?;
            if k == segments {
                out_points.push(path[path.len() - 1]);
                break;
            }
            let target = total * k as f64 / segments as f64;
            while segment + 1 < lengths.len() && walked + lengths[segment] < target {
                walked += lengths[segment];
                segment += 1;
            }
            let t = if lengths[segment] > 0.0 {
                ((target - walked) / lengths[segment]).clamp(0.0, 1.0)
            } else {
                0.0
            };
            out_points.push(lerp(path[segment], path[segment + 1], t));
        }
        out_prims.push((
            PrimKind::Polyline,
            (start..start + segments as u32 + 1).collect(),
        ));
    }
    build_geo(&out_points, &out_prims, ctx)
}

/// Rotates `v` by the minimal rotation taking unit vector `from` onto unit vector `to`.
fn minimal_rotation(v: D3, from: D3, to: D3) -> D3 {
    let axis = cross(from, to);
    let sin = dot(axis, axis).sqrt();
    let cos = dot(from, to);
    if sin < 1e-12 {
        return v;
    }
    let k = scale(axis, 1.0 / sin);
    add(
        add(scale(v, cos), scale(cross(k, v), sin)),
        scale(k, dot(k, v) * (1.0 - cos)),
    )
}

/// Sweeps a regular `sides`-gon of `radius` along every polyline using parallel-transport frames,
/// producing outward-facing quad tubes, closed with n-gon caps when `capped`. Polygons are
/// consumed; degenerate curves with no extent are skipped. Bend rings are not mitered, so tubes
/// narrow slightly at sharp corners.
pub fn sweep_curves(
    geo: &GeoDetail,
    radius: f64,
    sides: u32,
    capped: bool,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(radius.is_finite() && radius > 0.0) || sides < 3 {
        return Err(GeoError::InvalidTopology);
    }
    let segments = sides as usize;
    let points = load_points(geo)?;
    let mut out_points: Vec<D3> = Vec::new();
    let mut out_prims = Vec::new();
    for (prim_index, prim) in load_prims(geo)?.into_iter().enumerate() {
        ctx.poll_every(prim_index)?;
        if prim.kind != PrimKind::Polyline {
            continue;
        }
        let mut path: Vec<D3> = Vec::with_capacity(prim.points.len());
        for &point in &prim.points {
            let p = points[point as usize];
            if path.last().is_none_or(|&last| distance(last, p) > 1e-12) {
                path.push(p);
            }
        }
        if path.len() < 2 {
            continue;
        }
        let edges: Vec<D3> = path
            .windows(2)
            .map(|w| normalized(sub(w[1], w[0])).unwrap_or([0.0, 1.0, 0.0]))
            .collect();
        let tangent_at = |i: usize| {
            if i == 0 {
                edges[0]
            } else if i == edges.len() {
                edges[edges.len() - 1]
            } else {
                normalized(add(edges[i - 1], edges[i])).unwrap_or(edges[i])
            }
        };
        let rings = path.len();
        let projected_points = out_points.len() + rings * segments;
        ctx.check_output(projected_points, projected_points * 4)?;
        let base = u32::try_from(out_points.len()).map_err(|_| GeoError::LimitExceeded)?;
        let mut tangent = tangent_at(0);
        let mut u = any_perpendicular(tangent);
        for (ring, &center) in path.iter().enumerate() {
            let next_tangent = tangent_at(ring);
            u = minimal_rotation(u, tangent, next_tangent);
            // Remove accumulated drift so the frame stays orthonormal along long curves.
            let drift_free = sub(u, scale(next_tangent, dot(u, next_tangent)));
            u = normalized(drift_free).unwrap_or_else(|| any_perpendicular(next_tangent));
            tangent = next_tangent;
            let v = cross(tangent, u);
            for side in 0..segments {
                let angle = TAU * side as f64 / f64::from(sides);
                let offset = add(scale(u, angle.cos()), scale(v, angle.sin()));
                out_points.push(add(center, scale(offset, radius)));
            }
        }
        let at = |ring: usize, side: usize| base + (ring * segments + side % segments) as u32;
        if capped {
            out_prims.push((
                PrimKind::Polygon,
                (0..segments).rev().map(|s| at(0, s)).collect::<Vec<_>>(),
            ));
        }
        for ring in 0..rings - 1 {
            for side in 0..segments {
                out_prims.push((
                    PrimKind::Polygon,
                    vec![
                        at(ring, side),
                        at(ring, side + 1),
                        at(ring + 1, side + 1),
                        at(ring + 1, side),
                    ],
                ));
            }
        }
        if capped {
            out_prims.push((
                PrimKind::Polygon,
                (0..segments).map(|s| at(rings - 1, s)).collect::<Vec<_>>(),
            ));
        }
    }
    build_geo(&out_points, &out_prims, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{signed_volume, to_d3};
    use crate::primitives::create_line;
    use pcc_geo::{PrimRun, TopologyIndex};
    use pcc_math::Vec3;
    use std::f64::consts::PI;
    use std::sync::atomic::AtomicBool;

    fn l_shape() -> Result<GeoDetail, GeoError> {
        GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(3.0, 0.0, 0.0)?,
            Vec3::checked(3.0, 4.0, 0.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polyline,
                arity: 3,
                prim_count: 1,
            }],
            vec![0, 1, 2],
        )
    }

    #[test]
    fn resample_produces_uniform_segments_and_exact_endpoints() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let out = resample_curves(&l_shape()?, 0.5, &ctx)?;
        assert_eq!(out.prim_count(), 1);
        assert_eq!(out.len(), 15);
        let path = load_points(&out)?;
        assert_eq!(path[0], [0.0, 0.0, 0.0]);
        assert_eq!(path[14], [3.0, 4.0, 0.0]);
        // Points sit on the original curve at equal arc-length parameters: 6 along X, 8 along Y.
        for (k, point) in path.iter().enumerate() {
            let s = k as f64 * 0.5;
            let expected = if s <= 3.0 {
                [s, 0.0, 0.0]
            } else {
                [3.0, s - 3.0, 0.0]
            };
            assert!(distance(*point, expected) < 1e-9, "{k}: {point:?}");
        }
        assert!(resample_curves(&l_shape()?, 0.0, &ctx).is_err());
        Ok(())
    }

    #[test]
    fn resample_never_exceeds_the_budget() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let tight = SopContext::new(4096, &flag);
        assert_eq!(
            resample_curves(&l_shape()?, 1e-6, &tight).err(),
            Some(GeoError::LimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn straight_sweep_is_a_closed_outward_prism_of_exact_volume() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let line = create_line(Vec3::ZERO, Vec3::checked(0.0, 0.0, 5.0)?, 6, &ctx)?;
        let sides = 64u32;
        let tube = sweep_curves(&line, 0.5, sides, true, &ctx)?;
        assert!(TopologyIndex::build(&tube).is_closed_manifold());
        let n = f64::from(sides);
        let exact = 0.5 * n * 0.25 * (TAU / n).sin() * 5.0;
        assert!(
            (signed_volume(&tube) - exact).abs() < 1e-5,
            "{} vs {exact}",
            signed_volume(&tube)
        );
        let open = sweep_curves(&line, 0.5, sides, false, &ctx)?;
        assert_eq!(
            TopologyIndex::build(&open).boundary_edge_count(),
            2 * sides as usize
        );
        Ok(())
    }

    #[test]
    fn bent_sweep_stays_closed_and_its_frames_do_not_twist() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        let tube = sweep_curves(&l_shape()?, 0.25, 8, true, &ctx)?;
        assert!(TopologyIndex::build(&tube).is_closed_manifold());
        assert!(signed_volume(&tube) > 0.0);
        // For a planar curve parallel transport keeps the out-of-plane frame axis fixed: the
        // +/-Z extent must therefore be the ring radius on every ring.
        for ring in 0..3 {
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            for side in 0..8 {
                let z = to_d3(
                    tube.point(ring * 8 + side)
                        .ok_or(GeoError::IndexOutOfBounds)?,
                )[2];
                low = low.min(z);
                high = high.max(z);
            }
            assert!(
                (high - low - 0.5).abs() < 0.08,
                "ring {ring}: {low}..{high}"
            );
        }
        // A semicircle must stay closed through many small frame rotations.
        let steps = 64u32;
        let mut arc = Vec::new();
        for i in 0..=steps {
            let a = PI * f64::from(i) / f64::from(steps);
            arc.push(Vec3::checked(a.cos() as f32, a.sin() as f32, 0.0)?);
        }
        let curve = GeoDetail::points(arc).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polyline,
                arity: steps + 1,
                prim_count: 1,
            }],
            (0..=steps).collect(),
        )?;
        assert!(
            TopologyIndex::build(&sweep_curves(&curve, 0.05, 6, true, &ctx)?).is_closed_manifold()
        );
        Ok(())
    }
    #[test]
    fn minimal_rotation_maps_the_tangent_and_fixes_the_rotation_axis() {
        let from = [1.0, 0.0, 0.0];
        let angle = 30f64.to_radians();
        let to = [angle.cos(), angle.sin(), 0.0];
        // The source tangent lands exactly on the target tangent.
        assert!(distance(minimal_rotation(from, from, to), to) < 1e-12);
        // The rotation axis is unchanged.
        let axis = [0.0, 0.0, 1.0];
        assert!(distance(minimal_rotation(axis, from, to), axis) < 1e-12);
        // An in-plane perpendicular turns by the same angle.
        let expected = [-angle.sin(), angle.cos(), 0.0];
        assert!(distance(minimal_rotation([0.0, 1.0, 0.0], from, to), expected) < 1e-12);
        // Parallel tangents are the identity.
        assert_eq!(minimal_rotation(axis, from, from), axis);
    }
}
