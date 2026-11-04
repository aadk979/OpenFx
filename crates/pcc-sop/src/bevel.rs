//! Edge bevel by corner offsetting: every polygon corner becomes its own point, slid inward from
//! the bevelled edges next to it; bevelled edges gain a strip and vertices a closing polygon.

use crate::common::{
    D3, SopContext, add, build_geo, cross, dot, length, load_points, load_polygons, normalized,
    polygon_area_vector, scale, sub,
};
use pcc_geo::{GeoDetail, GeoError, PrimKind, TopologyIndex};

fn find_root(parents: &mut [u32], start: u32) -> u32 {
    let mut root = start;
    while parents[root as usize] != root {
        root = parents[root as usize];
    }
    let mut node = start;
    while parents[node as usize] != root {
        let next = parents[node as usize];
        parents[node as usize] = root;
        node = next;
    }
    root
}

fn union(parents: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find_root(parents, a), find_root(parents, b));
    if ra != rb {
        // The lower index wins so that the output numbering is canonical.
        let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parents[high as usize] = low;
    }
}

/// Removes consecutive repeats, cyclically, from a polygon outline.
fn dedupe_cyclic(values: &mut Vec<u32>) {
    values.dedup();
    while values.len() > 1 && values.first() == values.last() {
        values.pop();
    }
}

/// Bevels edges whose dihedral angle is at least `min_angle_degrees` by moving each adjacent face
/// inward `offset`. Mesh boundaries are never bevelled. Requires manifold polygon input; polylines
/// are dropped and attributes do not survive because all topology is rebuilt.
pub fn poly_bevel(
    geo: &GeoDetail,
    offset: f64,
    min_angle_degrees: f64,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(offset.is_finite() && offset > 0.0 && min_angle_degrees.is_finite()) {
        return Err(GeoError::InvalidTopology);
    }
    let topology = TopologyIndex::build(geo);
    if !topology.is_manifold() {
        return Err(GeoError::InvalidTopology);
    }
    let points = load_points(geo)?;
    let polygons = load_polygons(geo)?;
    if polygons.len() != geo.prim_count() {
        return Err(GeoError::InvalidTopology);
    }
    let vertex_count = geo.vertex_count();
    let he_point = |he: u32| {
        geo.vertex_point(he as usize)
            .ok_or(GeoError::IndexOutOfBounds)
    };
    let next = |he: u32| topology.next(he).ok_or(GeoError::InvalidTopology);
    let prev = |he: u32| topology.prev(he).ok_or(GeoError::InvalidTopology);

    let mut face_normals = Vec::with_capacity(polygons.len());
    let mut he_face = Vec::with_capacity(vertex_count);
    for (face, polygon) in polygons.iter().enumerate() {
        face_normals.push(normalized(polygon_area_vector(&points, polygon)).unwrap_or([0.0; 3]));
        he_face.extend(std::iter::repeat_n(face, polygon.len()));
    }

    let threshold = min_angle_degrees.to_radians().cos();
    let mut bevelled = vec![false; vertex_count];
    for he in 0..vertex_count as u32 {
        if let Some(twin) = topology.twin(he) {
            let cosine = dot(
                face_normals[he_face[he as usize]],
                face_normals[he_face[twin as usize]],
            );
            bevelled[he as usize] = cosine <= threshold;
        }
    }

    // Corner positions: slide along each adjacent edge so that the corner sits `offset` away from
    // the bevelled edge lines within its face plane.
    let mut positions: Vec<D3> = Vec::with_capacity(vertex_count);
    for he in 0..vertex_count as u32 {
        ctx.poll_every(he as usize)?;
        let previous = prev(he)?;
        let origin = points[he_point(he)? as usize];
        let forward_to = points[he_point(next(he)?)? as usize];
        let backward_to = points[he_point(previous)? as usize];
        let (to_next, to_prev) = (sub(forward_to, origin), sub(backward_to, origin));
        let (len_next, len_prev) = (length(to_next), length(to_prev));
        if len_next <= 0.0 || len_prev <= 0.0 {
            positions.push(origin);
            continue;
        }
        let (dir_next, dir_prev) = (
            scale(to_next, 1.0 / len_next),
            scale(to_prev, 1.0 / len_prev),
        );
        let sine = length(cross(dir_next, dir_prev)).max(1e-3);
        let slide = (offset / sine).min(0.49 * len_next.min(len_prev));
        let mut corner = origin;
        // A bevelled edge pushes the corner along the opposite adjacent edge.
        if bevelled[previous as usize] {
            corner = add(corner, scale(dir_next, slide));
        }
        if bevelled[he as usize] {
            corner = add(corner, scale(dir_prev, slide));
        }
        positions.push(corner);
    }

    // Corners across an un-bevelled edge stay welded.
    let mut parents: Vec<u32> = (0..vertex_count as u32).collect();
    for he in 0..vertex_count as u32 {
        if let Some(twin) = topology.twin(he)
            && he < twin
            && !bevelled[he as usize]
        {
            union(&mut parents, he, next(twin)?);
            union(&mut parents, next(he)?, twin);
        }
    }
    let mut class_of = vec![u32::MAX; vertex_count];
    let mut sums: Vec<(D3, u32)> = Vec::new();
    for he in 0..vertex_count as u32 {
        let root = find_root(&mut parents, he) as usize;
        if class_of[root] == u32::MAX {
            class_of[root] = sums.len() as u32;
            sums.push(([0.0; 3], 0));
        }
        let slot = &mut sums[class_of[root] as usize];
        slot.0 = add(slot.0, positions[he as usize]);
        slot.1 += 1;
    }
    let class = |parents: &mut [u32], he: u32| class_of[find_root(parents, he) as usize];
    let out_points: Vec<D3> = sums
        .iter()
        .map(|&(sum, count)| scale(sum, 1.0 / f64::from(count)))
        .collect();

    let mut prims: Vec<(PrimKind, Vec<u32>)> = Vec::new();
    let mut emit = |mut outline: Vec<u32>| {
        dedupe_cyclic(&mut outline);
        if outline.len() >= 3 {
            prims.push((PrimKind::Polygon, outline));
        }
    };
    let mut cursor = 0u32;
    for polygon in &polygons {
        let outline: Vec<u32> = (cursor..cursor + polygon.len() as u32)
            .map(|he| class(&mut parents, he))
            .collect();
        emit(outline);
        cursor += polygon.len() as u32;
    }
    for he in 0..vertex_count as u32 {
        ctx.poll_every(he as usize)?;
        if let Some(twin) = topology.twin(he)
            && he < twin
            && bevelled[he as usize]
        {
            emit(vec![
                class(&mut parents, next(he)?),
                class(&mut parents, he),
                class(&mut parents, next(twin)?),
                class(&mut parents, twin),
            ]);
        }
    }
    // Closing polygons: walk each closed fan of corners around a mesh vertex once.
    let mut visited = vec![false; vertex_count];
    for start in 0..vertex_count as u32 {
        if visited[start as usize] {
            continue;
        }
        let mut fan = Vec::new();
        let mut corner = start;
        let closed = loop {
            visited[corner as usize] = true;
            fan.push(corner);
            let Some(step) = topology.twin(prev(corner)?) else {
                break false;
            };
            if step == start {
                break true;
            }
            if visited[step as usize] {
                break false;
            }
            corner = step;
        };
        if closed {
            let outline: Vec<u32> = fan.iter().map(|&c| class(&mut parents, c)).collect();
            emit(outline);
        }
    }
    build_geo(&out_points, &prims, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::signed_volume;
    use crate::primitives::create_tube;
    use pcc_geo::PrimRun;
    use pcc_math::Vec3;
    use std::sync::atomic::AtomicBool;

    fn cube(side: f32) -> Result<GeoDetail, GeoError> {
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

    #[test]
    fn cube_chamfer_matches_the_analytic_volume_and_euler_counts() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let (side, d) = (2.0f64, 0.25f64);
        let bevelled = poly_bevel(&cube(side as f32)?, d, 30.0, &ctx)?;
        let topology = TopologyIndex::build(&bevelled);
        assert!(topology.is_closed_manifold());
        // 6 faces + 12 edge strips + 8 corner triangles; V - E + F = 24 - 48 + 26 = 2.
        assert_eq!(bevelled.len(), 24);
        assert_eq!(bevelled.prim_count(), 26);
        // V = s^3 - 12 (d^2 / 2)(s - 2d) - 8 (5 d^3 / 6), from slicing the removed wedges and
        // corner cells; the retained corner cell is a tetrahedron of volume d^3 / 6.
        let exact = side.powi(3) - 6.0 * d * d * (side - 2.0 * d) - 20.0 / 3.0 * d.powi(3);
        assert!(
            (signed_volume(&bevelled) - exact).abs() < 1e-5,
            "{} vs {exact}",
            signed_volume(&bevelled)
        );
        Ok(())
    }

    #[test]
    fn angle_threshold_selects_only_sharp_edges() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 22, &flag);
        // Nothing qualifies above 90 degrees on a cube: geometry stays a cube, faces unchanged.
        let untouched = poly_bevel(&cube(1.0)?, 0.1, 120.0, &ctx)?;
        assert_eq!(untouched.prim_count(), 6);
        assert_eq!(untouched.len(), 8);
        assert!((signed_volume(&untouched) - 1.0).abs() < 1e-6);
        // On a 32-sided tube the sides meet at 11.25 degrees: a 45 degree threshold bevels only
        // the two cap rims, adding exactly one strip per rim edge.
        let tube = create_tube(1.0, 2.0, 1, 32, &ctx)?;
        assert_eq!(tube.prim_count(), 34);
        let rims = poly_bevel(&tube, 0.05, 45.0, &ctx)?;
        assert!(TopologyIndex::build(&rims).is_closed_manifold());
        assert_eq!(rims.prim_count(), 34 + 64);
        let before = signed_volume(&tube);
        let after = signed_volume(&rims);
        assert!(
            after < before && after > 0.95 * before,
            "{after} vs {before}"
        );
        Ok(())
    }

    #[test]
    fn excessive_offset_is_clamped_without_inverting_the_solid() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let input = cube(1.0)?;
        let huge = poly_bevel(&input, 100.0, 30.0, &ctx)?;
        assert!(TopologyIndex::build(&huge).is_closed_manifold());
        assert!(signed_volume(&huge) > 0.0);
        assert!(poly_bevel(&input, 0.0, 30.0, &ctx).is_err());
        Ok(())
    }
    #[test]
    fn non_right_angle_corners_keep_the_exact_inward_offset() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        // A three-sided tube of circumradius 1 has an equilateral cap of inradius 1/2. Offsetting
        // every cap edge inward by d leaves an equilateral triangle of area 3 sqrt(3) (r - d)^2.
        let prism = create_tube(1.0, 2.0, 1, 3, &ctx)?;
        let d = 0.1;
        let bevelled = poly_bevel(&prism, d, 30.0, &ctx)?;
        assert!(TopologyIndex::build(&bevelled).is_closed_manifold());
        let points = load_points(&bevelled)?;
        let polygons = load_polygons(&bevelled)?;
        assert_eq!(polygons[0].len(), 3, "the first face is the cap");
        let area = length(polygon_area_vector(&points, &polygons[0])) * 0.5;
        let exact = 3.0 * 3f64.sqrt() * (0.5 - d) * (0.5 - d);
        assert!((area - exact).abs() < 1e-5, "{area} vs {exact}");
        Ok(())
    }
}
