//! Non-topological and clean-up operators: TRS transform, reverse, triangulate, fuse, measure.

use crate::common::{
    D3, SopContext, add, any_perpendicular, attach_prims, cross, distance, dot, load_points,
    load_prims, normalized, polygon_area_vector, signed_volume, sub, to_d3, to_vec3,
};
use crate::earclip::{P2, ear_clip};
use pcc_geo::{AttrDomain, AttributeColumn, FiniteF32, GeoDetail, GeoError, PrimKind};
use pcc_math::Vec3;
use std::collections::HashMap;

/// Scale, then Euler XYZ rotation in degrees (`Rz * Ry * Rx`), then translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub translate: Vec3,
    pub rotate_degrees: Vec3,
    pub scale: Vec3,
}

fn rotation_matrix(degrees: Vec3) -> [D3; 3] {
    let (sx, cx) = f64::from(degrees.x).to_radians().sin_cos();
    let (sy, cy) = f64::from(degrees.y).to_radians().sin_cos();
    let (sz, cz) = f64::from(degrees.z).to_radians().sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

fn apply_matrix(m: &[D3; 3], v: D3) -> D3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// Applies a full TRS transform to positions and every `N` attribute. A mirroring (negative
/// determinant) scale also reverses polygon winding so closed surfaces keep facing outward.
pub fn transform_trs(
    geo: &GeoDetail,
    trs: &Trs,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    let scale = to_d3(trs.scale);
    if scale.iter().any(|&s| s == 0.0 || !s.is_finite()) {
        return Err(GeoError::InvalidTopology);
    }
    ctx.check_output(geo.len(), 0)?;
    let rotation = rotation_matrix(trs.rotate_degrees);
    let translate = to_d3(trs.translate);
    let mut positions = Vec::with_capacity(geo.len());
    for (i, p) in load_points(geo)?.into_iter().enumerate() {
        ctx.poll_every(i)?;
        let scaled = [p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]];
        positions.push(to_vec3(add(apply_matrix(&rotation, scaled), translate))?);
    }
    let mut out = geo.with_positions(positions)?;
    // Instances compose the same map onto their own transforms, leaving prototypes untouched.
    let rows_of = |row: D3| {
        Vec3::checked(
            (row[0] * scale[0]) as f32,
            (row[1] * scale[1]) as f32,
            (row[2] * scale[2]) as f32,
        )
    };
    out = out.with_packed_linear(&[
        rows_of(rotation[0])?,
        rows_of(rotation[1])?,
        rows_of(rotation[2])?,
    ])?;
    for domain in [AttrDomain::Point, AttrDomain::Vertex, AttrDomain::Prim] {
        let Some(normals) = geo
            .domain_attribute(domain, "N")
            .and_then(AttributeColumn::as_vec3)
        else {
            continue;
        };
        let mut rotated = Vec::with_capacity(normals.len());
        for i in 0..normals.len() {
            ctx.poll_every(i)?;
            let n = normals.get(i).map_or([0.0; 3], to_d3);
            // Normals transform by the inverse transpose: R * S^-1 for a TRS matrix.
            let unscaled = [n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]];
            let mapped = apply_matrix(&rotation, unscaled);
            rotated.push(to_vec3(normalized(mapped).unwrap_or(mapped))?);
        }
        out = out.with_domain_attribute(domain, "N", AttributeColumn::vec3_values(rotated))?;
    }
    if scale[0] * scale[1] * scale[2] < 0.0 {
        out = out.with_reversed_winding()?;
    }
    Ok(out)
}

/// Flips the orientation of every polygon, keeping all attributes attached.
pub fn reverse_winding(geo: &GeoDetail, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
    ctx.poll()?;
    ctx.check_output(0, geo.vertex_count())?;
    geo.with_reversed_winding()
}

/// Triangulates one polygon in its best-fit plane, handling concave outlines.
pub(crate) fn triangulate_polygon(points: &[D3], polygon: &[u32]) -> Vec<[u32; 3]> {
    if let [a, b, c] = polygon {
        return vec![[*a, *b, *c]];
    }
    let Some(normal) = normalized(polygon_area_vector(points, polygon)) else {
        return (1..polygon.len() - 1)
            .map(|k| [polygon[0], polygon[k], polygon[k + 1]])
            .collect();
    };
    let u = any_perpendicular(normal);
    let v = cross(normal, u);
    let flat: Vec<P2> = polygon
        .iter()
        .map(|&i| {
            let p = points[i as usize];
            [dot(p, u), dot(p, v)]
        })
        .collect();
    let corners: Vec<usize> = (0..polygon.len()).collect();
    ear_clip(&flat, &corners)
        .into_iter()
        .map(|t| [polygon[t[0]], polygon[t[1]], polygon[t[2]]])
        .collect()
}

/// Every polygon triangulated, in primitive order.
pub(crate) fn triangles_of(geo: &GeoDetail) -> Result<(Vec<D3>, Vec<[u32; 3]>), GeoError> {
    let points = load_points(geo)?;
    let mut triangles = Vec::new();
    for prim in load_prims(geo)? {
        if prim.kind == PrimKind::Polygon {
            triangles.extend(triangulate_polygon(&points, &prim.points));
        }
    }
    Ok((points, triangles))
}

/// Splits all polygons into triangles, preserving point data and passing polylines through.
/// Vertex and primitive attributes do not survive because the primitive set changes.
pub fn triangulate(geo: &GeoDetail, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
    let points = load_points(geo)?;
    let mut prims = Vec::with_capacity(geo.prim_count());
    for (i, prim) in load_prims(geo)?.into_iter().enumerate() {
        ctx.poll_every(i)?;
        match prim.kind {
            PrimKind::Polygon => {
                for t in triangulate_polygon(&points, &prim.points) {
                    prims.push((PrimKind::Polygon, t.to_vec()));
                }
            }
            kind => prims.push((kind, prim.points)),
        }
    }
    ctx.check_output(geo.len(), prims.iter().map(|p| p.1.len()).sum())?;
    attach_prims(geo, &prims)
}

/// Merges points closer than `max_distance`, keeping the lowest-indexed point of each cluster and
/// its attributes. Collapsed primitives are removed; primitive attributes do not survive.
pub fn fuse_points(
    geo: &GeoDetail,
    max_distance: f64,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(max_distance.is_finite() && max_distance >= 0.0) {
        return Err(GeoError::InvalidTopology);
    }
    let points = load_points(geo)?;
    let extent = geo
        .bounds()
        .map_or(0.0, |(lo, hi)| distance(to_d3(lo), to_d3(hi)));
    let cell = max_distance.max(extent * 1e-5).max(1e-12);
    let key = |p: D3| {
        (
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        )
    };
    let mut cells: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut remap = Vec::with_capacity(points.len());
    let mut kept: Vec<usize> = Vec::new();
    for (i, &p) in points.iter().enumerate() {
        ctx.poll_every(i)?;
        let (cx, cy, cz) = key(p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = cells.get(&(cx + dx, cy + dy, cz + dz)) else {
                        continue;
                    };
                    for &candidate in bucket {
                        let representative = points[kept[candidate as usize]];
                        if distance(p, representative) <= max_distance
                            && found.is_none_or(|best| candidate < best)
                        {
                            found = Some(candidate);
                            if candidate == 0 {
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        let target = match found {
            Some(index) => index,
            None => {
                let index = u32::try_from(kept.len()).map_err(|_| GeoError::LimitExceeded)?;
                kept.push(i);
                cells.entry((cx, cy, cz)).or_default().push(index);
                index
            }
        };
        remap.push(target);
    }
    let mut prims = Vec::with_capacity(geo.prim_count());
    for prim in load_prims(geo)? {
        let mut mapped: Vec<u32> = Vec::with_capacity(prim.points.len());
        for &point in &prim.points {
            let target = remap[point as usize];
            if mapped.last() != Some(&target) {
                mapped.push(target);
            }
        }
        let minimum = match prim.kind {
            PrimKind::Polygon => {
                if mapped.len() > 1 && mapped.first() == mapped.last() {
                    mapped.pop();
                }
                3
            }
            PrimKind::Polyline => 2,
            PrimKind::Packed => 1,
            PrimKind::Tetrahedron => 4,
        };
        if mapped.len() >= minimum {
            prims.push((prim.kind, mapped));
        }
    }
    ctx.check_output(kept.len(), prims.iter().map(|p| p.1.len()).sum())?;
    let positions = kept
        .iter()
        .map(|&i| to_vec3(points[i]))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = GeoDetail::points(positions);
    for name in geo.domain_attribute_names(AttrDomain::Point) {
        if let Some(column) = geo.domain_attribute(AttrDomain::Point, name) {
            out = out.with_attribute(name, column.selected(&kept)?)?;
        }
    }
    for name in geo.domain_attribute_names(AttrDomain::Detail) {
        if let Some(column) = geo.domain_attribute(AttrDomain::Detail, name) {
            out = out.with_domain_attribute(AttrDomain::Detail, name, column.clone())?;
        }
    }
    attach_prims(&out, &prims)
}

/// Writes primitive `f@area` and `f@perimeter`, and detail `f@total_area` and `f@total_volume`.
pub fn measure(geo: &GeoDetail, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
    ctx.check_output(0, 2 * geo.prim_count())?;
    let points = load_points(geo)?;
    let prims = load_prims(geo)?;
    let mut areas = Vec::with_capacity(prims.len());
    let mut perimeters = Vec::with_capacity(prims.len());
    let mut total_area = 0.0;
    for (i, prim) in prims.iter().enumerate() {
        ctx.poll_every(i)?;
        let segments = prim.points.len()
            - match prim.kind {
                PrimKind::Polygon => 0,
                PrimKind::Polyline | PrimKind::Packed | PrimKind::Tetrahedron => 1,
            };
        let mut perimeter = 0.0;
        for k in 0..segments {
            let a = points[prim.points[k] as usize];
            let b = points[prim.points[(k + 1) % prim.points.len()] as usize];
            perimeter += distance(a, b);
        }
        let area = match prim.kind {
            PrimKind::Polygon => {
                let origin = points[prim.points[0] as usize];
                let shifted: Vec<D3> = prim
                    .points
                    .iter()
                    .map(|&p| sub(points[p as usize], origin))
                    .collect();
                let order: Vec<u32> = (0..shifted.len() as u32).collect();
                dot_len(polygon_area_vector(&shifted, &order)) * 0.5
            }
            PrimKind::Polyline | PrimKind::Packed | PrimKind::Tetrahedron => 0.0,
        };
        total_area += area;
        areas.push(FiniteF32::checked(area as f32)?);
        perimeters.push(FiniteF32::checked(perimeter as f32)?);
    }
    let mut out = geo.clone();
    if !prims.is_empty() {
        out = out
            .with_domain_attribute(
                AttrDomain::Prim,
                "area",
                AttributeColumn::f32_checked(areas.iter().map(|f| f.get()).collect())?,
            )?
            .with_domain_attribute(
                AttrDomain::Prim,
                "perimeter",
                AttributeColumn::f32_checked(perimeters.iter().map(|f| f.get()).collect())?,
            )?;
    }
    out.with_domain_attribute(
        AttrDomain::Detail,
        "total_area",
        AttributeColumn::f32_checked(vec![total_area as f32])?,
    )?
    .with_domain_attribute(
        AttrDomain::Detail,
        "total_volume",
        AttributeColumn::f32_checked(vec![signed_volume(geo) as f32])?,
    )
}

fn dot_len(v: D3) -> f64 {
    dot(v, v).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::create_sphere;
    use pcc_geo::{PrimRun, TopologyIndex};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn unit_cube() -> Result<GeoDetail, GeoError> {
        let v = |x, y, z| Vec3::checked(x, y, z);
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

    #[test]
    fn trs_applies_scale_then_rotation_then_translation_and_transforms_normals()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let geo = GeoDetail::points(vec![Vec3::checked(1.0, 0.0, 0.0)?]).with_attribute(
            "N",
            AttributeColumn::vec3_values(vec![Vec3::checked(1.0, 0.0, 0.0)?]),
        )?;
        let out = transform_trs(
            &geo,
            &Trs {
                translate: Vec3::checked(0.0, 0.0, 5.0)?,
                rotate_degrees: Vec3::checked(0.0, 0.0, 90.0)?,
                scale: Vec3::checked(2.0, 1.0, 1.0)?,
            },
            &ctx,
        )?;
        let p = out.point(0).ok_or(GeoError::IndexOutOfBounds)?;
        assert!(p.x.abs() < 1e-6 && (p.y - 2.0).abs() < 1e-6 && (p.z - 5.0).abs() < 1e-6);
        let n = out
            .attribute("N")
            .and_then(AttributeColumn::as_vec3)
            .and_then(|c| c.get(0))
            .ok_or(GeoError::AttributeNotFound)?;
        assert!(n.x.abs() < 1e-6 && (n.y - 1.0).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn nonuniform_scale_keeps_normals_perpendicular_and_mirroring_keeps_volume_positive()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 24, &flag);
        // A plane with normal (1,1,0)/sqrt2 scaled x2 in X has normal proportional to (1,2,0).
        let geo = GeoDetail::points(vec![Vec3::ZERO]).with_attribute(
            "N",
            AttributeColumn::vec3_values(vec![Vec3::checked(0.70710677, 0.70710677, 0.0)?]),
        )?;
        let identity_rotation = Vec3::ZERO;
        let out = transform_trs(
            &geo,
            &Trs {
                translate: Vec3::ZERO,
                rotate_degrees: identity_rotation,
                scale: Vec3::checked(2.0, 1.0, 1.0)?,
            },
            &ctx,
        )?;
        let n = out
            .attribute("N")
            .and_then(AttributeColumn::as_vec3)
            .and_then(|c| c.get(0))
            .ok_or(GeoError::AttributeNotFound)?;
        assert!((n.y / n.x - 2.0).abs() < 1e-5);

        let sphere = create_sphere(1.0, 8, 12, &ctx)?;
        let before = signed_volume(&sphere);
        let mirrored = transform_trs(
            &sphere,
            &Trs {
                translate: Vec3::ZERO,
                rotate_degrees: Vec3::ZERO,
                scale: Vec3::checked(-1.0, 1.0, 2.0)?,
            },
            &ctx,
        )?;
        assert!((signed_volume(&mirrored) - 2.0 * before).abs() < 1e-5);
        Ok(())
    }

    #[test]
    fn reverse_is_an_involution_that_negates_volume() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let cube = unit_cube()?;
        let reversed = reverse_winding(&cube, &ctx)?;
        assert!((signed_volume(&cube) - 1.0).abs() < 1e-6);
        assert!((signed_volume(&reversed) + 1.0).abs() < 1e-6);
        let restored = reverse_winding(&reversed, &ctx)?;
        assert_eq!(restored.hash(), cube.hash());
        Ok(())
    }

    #[test]
    fn triangulate_preserves_measures_and_concave_outlines() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let cube = unit_cube()?;
        let tris = triangulate(&cube, &ctx)?;
        assert_eq!(tris.prim_count(), 12);
        assert!(TopologyIndex::build(&tris).is_closed_manifold());
        assert!((signed_volume(&tris) - 1.0).abs() < 1e-6);
        // A fan from the first corner of this arrowhead would produce an inverted triangle.
        let points = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 2.0],
            [0.0, 0.0, 1.0],
            [-2.0, 0.0, 2.0],
        ];
        let triangles = triangulate_polygon(&points, &[0, 3, 2, 1]);
        assert_eq!(triangles.len(), 2);
        let total: f64 = triangles
            .iter()
            .map(|t| {
                let n = polygon_area_vector(&points, t);
                dot_len(n) * 0.5
            })
            .sum();
        assert!((total - 2.0).abs() < 1e-12, "area {total}");
        Ok(())
    }

    #[test]
    fn fuse_welds_coincident_shells_and_removes_collapsed_prims() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        // Two triangles sharing an edge through duplicated points, plus one sliver.
        let geo = GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(1.0, 0.0, 0.0)?,
            Vec3::checked(0.0, 1.0, 0.0)?,
            Vec3::checked(1.0, 0.0, 0.001)?,
            Vec3::checked(1.0, 1.0, 0.0)?,
            Vec3::checked(0.0, 1.0, 0.001)?,
        ])
        .with_attribute(
            "weight",
            AttributeColumn::f32_checked(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0])?,
        )?
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 3,
            }],
            vec![0, 1, 2, 3, 4, 5, 1, 3, 2],
        )?;
        let fused = fuse_points(&geo, 0.01, &ctx)?;
        assert_eq!(fused.len(), 4);
        // The third triangle (1,3,2) welds to (1,1,2): only two corners remain, so it is removed.
        assert_eq!(fused.prim_count(), 2);
        let weight = fused
            .attribute("weight")
            .and_then(AttributeColumn::as_f32)
            .and_then(|c| c.get(3))
            .map(|f| f.get());
        assert_eq!(weight, Some(4.0));
        // Points 0.001 apart must stay separate below that distance.
        assert_eq!(fuse_points(&geo, 0.0006, &ctx)?.len(), 6);
        assert!(fuse_points(&geo, -1.0, &ctx).is_err());
        Ok(())
    }

    #[test]
    fn measure_reports_exact_prim_and_detail_values_and_honours_cancellation()
    -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let out = measure(&unit_cube()?, &ctx)?;
        let read = |domain, name: &str, index| {
            out.domain_attribute(domain, name)
                .and_then(AttributeColumn::as_f32)
                .and_then(|c| c.get(index))
                .map(|f| f.get())
        };
        assert_eq!(read(AttrDomain::Prim, "area", 3), Some(1.0));
        assert_eq!(read(AttrDomain::Prim, "perimeter", 3), Some(4.0));
        assert_eq!(read(AttrDomain::Detail, "total_area", 0), Some(6.0));
        assert_eq!(read(AttrDomain::Detail, "total_volume", 0), Some(1.0));

        flag.store(true, Ordering::Release);
        assert_eq!(
            reverse_winding(&unit_cube()?, &ctx).err(),
            Some(GeoError::Cancelled)
        );
        Ok(())
    }
    #[test]
    fn euler_rotation_matches_independently_composed_axis_rotations() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let (rx, ry, rz) = (30f64.to_radians(), 40f64.to_radians(), 50f64.to_radians());
        // Applies Rx, then Ry, then Rz, one plane rotation at a time.
        let expected = |[x, y, z]: D3| {
            let (y, z) = (y * rx.cos() - z * rx.sin(), y * rx.sin() + z * rx.cos());
            let (x, z) = (x * ry.cos() + z * ry.sin(), -x * ry.sin() + z * ry.cos());
            let (x, y) = (x * rz.cos() - y * rz.sin(), x * rz.sin() + y * rz.cos());
            [x, y, z]
        };
        let inputs: [D3; 4] = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, -2.0, 3.0],
        ];
        let positions = inputs
            .iter()
            .map(|&p| to_vec3(p))
            .collect::<Result<Vec<_>, _>>()?;
        let out = transform_trs(
            &GeoDetail::points(positions),
            &Trs {
                translate: Vec3::ZERO,
                rotate_degrees: Vec3::checked(30.0, 40.0, 50.0)?,
                scale: Vec3::checked(1.0, 1.0, 1.0)?,
            },
            &ctx,
        )?;
        for (i, &input) in inputs.iter().enumerate() {
            let actual = to_d3(out.point(i).ok_or(GeoError::IndexOutOfBounds)?);
            let gap = distance(actual, expected(input));
            assert!(gap < 1e-5, "{input:?}: {actual:?}, off by {gap}");
        }
        Ok(())
    }
}
