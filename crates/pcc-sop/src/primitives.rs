//! Parametric closed-surface generators (Y-up, outward-facing) and straight polylines.

use crate::common::{D3, SopContext, build_geo, lerp, to_d3};
use pcc_geo::{GeoDetail, GeoError, PrimKind, PrimRun};
use pcc_math::Vec3;
use std::f64::consts::{PI, TAU};

fn checked_cells(rows: u32, cols: u32) -> Result<usize, GeoError> {
    (rows as usize)
        .checked_mul(cols as usize)
        .ok_or(GeoError::LimitExceeded)
}

/// Latitude-longitude sphere with `rows` latitude bands and `cols` longitude segments.
pub fn create_sphere(
    radius: f64,
    rows: u32,
    cols: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(radius.is_finite() && radius > 0.0) || rows < 2 || cols < 3 {
        return Err(GeoError::InvalidTopology);
    }
    let rings = rows as usize - 1;
    let segments = cols as usize;
    let quads = checked_cells(rows - 2, cols)?;
    ctx.check_output(rings * segments + 2, quads * 4 + segments * 6)?;
    let mut points: Vec<D3> = Vec::with_capacity(rings * segments + 2);
    points.push([0.0, radius, 0.0]);
    for ring in 0..rings {
        ctx.poll()?;
        let polar = PI * (ring + 1) as f64 / f64::from(rows);
        for segment in 0..segments {
            let azimuth = TAU * segment as f64 / f64::from(cols);
            points.push([
                radius * polar.sin() * azimuth.cos(),
                radius * polar.cos(),
                radius * polar.sin() * azimuth.sin(),
            ]);
        }
    }
    points.push([0.0, -radius, 0.0]);
    let at = |ring: usize, segment: usize| (1 + ring * segments + segment % segments) as u32;
    let bottom = (points.len() - 1) as u32;
    let mut prims = Vec::with_capacity(quads + 2 * segments);
    for s in 0..segments {
        prims.push((PrimKind::Polygon, vec![0, at(0, s + 1), at(0, s)]));
    }
    for ring in 0..rings - 1 {
        for s in 0..segments {
            prims.push((
                PrimKind::Polygon,
                vec![
                    at(ring, s),
                    at(ring, s + 1),
                    at(ring + 1, s + 1),
                    at(ring + 1, s),
                ],
            ));
        }
    }
    for s in 0..segments {
        prims.push((
            PrimKind::Polygon,
            vec![bottom, at(rings - 1, s), at(rings - 1, s + 1)],
        ));
    }
    build_geo(&points, &prims, ctx)
}

/// Capped cylinder centred on the origin along +Y, with `rows` height bands and `cols` sides.
pub fn create_tube(
    radius: f64,
    height: f64,
    rows: u32,
    cols: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(radius.is_finite() && radius > 0.0 && height.is_finite() && height > 0.0)
        || rows < 1
        || cols < 3
    {
        return Err(GeoError::InvalidTopology);
    }
    let rings = rows as usize + 1;
    let segments = cols as usize;
    let side_quads = checked_cells(rows, cols)?;
    ctx.check_output(rings * segments, side_quads * 4 + segments * 2)?;
    let mut points: Vec<D3> = Vec::with_capacity(rings * segments);
    for ring in 0..rings {
        ctx.poll()?;
        let y = height * (0.5 - ring as f64 / f64::from(rows));
        for segment in 0..segments {
            let azimuth = TAU * segment as f64 / f64::from(cols);
            points.push([radius * azimuth.cos(), y, radius * azimuth.sin()]);
        }
    }
    let at = |ring: usize, segment: usize| (ring * segments + segment % segments) as u32;
    let mut prims = Vec::with_capacity(side_quads + 2);
    prims.push((
        PrimKind::Polygon,
        (0..segments).rev().map(|s| at(0, s)).collect(),
    ));
    for ring in 0..rings - 1 {
        for s in 0..segments {
            prims.push((
                PrimKind::Polygon,
                vec![
                    at(ring, s),
                    at(ring, s + 1),
                    at(ring + 1, s + 1),
                    at(ring + 1, s),
                ],
            ));
        }
    }
    prims.push((
        PrimKind::Polygon,
        (0..segments).map(|s| at(rings - 1, s)).collect(),
    ));
    build_geo(&points, &prims, ctx)
}

/// Quad torus around +Y with `rows` major and `cols` minor segments.
pub fn create_torus(
    major_radius: f64,
    minor_radius: f64,
    rows: u32,
    cols: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if !(major_radius.is_finite() && minor_radius.is_finite())
        || minor_radius <= 0.0
        || major_radius <= minor_radius
        || rows < 3
        || cols < 3
    {
        return Err(GeoError::InvalidTopology);
    }
    let (major, minor) = (rows as usize, cols as usize);
    let cells = checked_cells(rows, cols)?;
    ctx.check_output(cells, cells * 4)?;
    let mut points: Vec<D3> = Vec::with_capacity(cells);
    for i in 0..major {
        ctx.poll()?;
        let theta = TAU * i as f64 / f64::from(rows);
        for j in 0..minor {
            let phi = TAU * j as f64 / f64::from(cols);
            let ring = major_radius + minor_radius * phi.cos();
            points.push([
                ring * theta.cos(),
                minor_radius * phi.sin(),
                ring * theta.sin(),
            ]);
        }
    }
    let at = |i: usize, j: usize| ((i % major) * minor + j % minor) as u32;
    let mut prims = Vec::with_capacity(cells);
    for i in 0..major {
        for j in 0..minor {
            prims.push((
                PrimKind::Polygon,
                vec![at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)],
            ));
        }
    }
    build_geo(&points, &prims, ctx)
}

/// Single straight polyline with `points` evenly spaced points from `start` to `end`.
pub fn create_line(
    start: Vec3,
    end: Vec3,
    points: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if points < 2 {
        return Err(GeoError::InvalidTopology);
    }
    ctx.check_output(points as usize, points as usize)?;
    let (a, b) = (to_d3(start), to_d3(end));
    let positions: Vec<D3> = (0..points)
        .map(|i| lerp(a, b, f64::from(i) / f64::from(points - 1)))
        .collect();
    build_geo(
        &positions,
        &[(PrimKind::Polyline, (0..points).collect())],
        ctx,
    )
}

/// A solid box of tetrahedra, centred on the origin, with `divisions` cells along each axis.
///
/// Each cell is cut into six elements around its main diagonal. Every cell uses the same diagonal
/// direction, so neighbours agree on the triangles of the face they share and the mesh is conforming.
pub fn create_tet_box(
    size: Vec3,
    divisions: u32,
    ctx: &SopContext<'_>,
) -> Result<GeoDetail, GeoError> {
    if divisions == 0 || size.x <= 0.0 || size.y <= 0.0 || size.z <= 0.0 {
        return Err(GeoError::InvalidTopology);
    }
    let cells_per_axis = divisions as usize;
    let side = cells_per_axis + 1;
    let points = side
        .checked_mul(side)
        .and_then(|n| n.checked_mul(side))
        .ok_or(GeoError::LimitExceeded)?;
    let cells = cells_per_axis
        .checked_mul(cells_per_axis)
        .and_then(|n| n.checked_mul(cells_per_axis))
        .ok_or(GeoError::LimitExceeded)?;
    let elements = cells.checked_mul(6).ok_or(GeoError::LimitExceeded)?;
    let corners = elements.checked_mul(4).ok_or(GeoError::LimitExceeded)?;
    ctx.check_output(points, corners)?;
    let extent = to_d3(size);
    let coordinate =
        |index: usize, axis: usize| extent[axis] * (index as f64 / cells_per_axis as f64 - 0.5);
    let mut positions = Vec::with_capacity(points);
    for z in 0..side {
        ctx.poll()?;
        for y in 0..side {
            for x in 0..side {
                positions.push(Vec3::checked(
                    coordinate(x, 0) as f32,
                    coordinate(y, 1) as f32,
                    coordinate(z, 2) as f32,
                )?);
            }
        }
    }
    let at = |[x, y, z]: [usize; 3]| ((z * side + y) * side + x) as u32;
    // Walking from a cell's low corner to its high corner one axis at a time gives one element
    // per order of the axes. An odd order is inverted, so its middle corners swap.
    let walks: [([usize; 3], bool); 6] = [
        ([0, 1, 2], false),
        ([1, 2, 0], false),
        ([2, 0, 1], false),
        ([0, 2, 1], true),
        ([1, 0, 2], true),
        ([2, 1, 0], true),
    ];
    let mut vertices = Vec::with_capacity(corners);
    for z in 0..cells_per_axis {
        ctx.poll()?;
        for y in 0..cells_per_axis {
            for x in 0..cells_per_axis {
                for (walk, odd) in walks {
                    let mut corner = [x, y, z];
                    let start = at(corner);
                    corner[walk[0]] += 1;
                    let first = at(corner);
                    corner[walk[1]] += 1;
                    let second = at(corner);
                    let end = at([x + 1, y + 1, z + 1]);
                    let (b, c) = if odd {
                        (second, first)
                    } else {
                        (first, second)
                    };
                    vertices.extend_from_slice(&[start, b, c, end]);
                }
            }
        }
    }
    GeoDetail::points(positions).with_primitives(
        vec![PrimRun {
            kind: PrimKind::Tetrahedron,
            arity: 4,
            prim_count: elements,
        }],
        vertices,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::signed_volume;
    use pcc_geo::TopologyIndex;
    use std::sync::atomic::AtomicBool;

    fn relative_error(actual: f64, expected: f64) -> f64 {
        ((actual - expected) / expected).abs()
    }

    #[test]
    fn sphere_is_closed_outward_and_converges_to_analytic_volume() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let coarse = create_sphere(2.0, 16, 32, &ctx)?;
        let fine = create_sphere(2.0, 128, 256, &ctx)?;
        assert!(TopologyIndex::build(&coarse).is_closed_manifold());
        assert!(TopologyIndex::build(&fine).is_closed_manifold());
        assert_eq!(coarse.len(), 2 + 15 * 32);
        let exact = 4.0 / 3.0 * PI * 8.0;
        let (e_coarse, e_fine) = (
            relative_error(signed_volume(&coarse), exact),
            relative_error(signed_volume(&fine), exact),
        );
        assert!(signed_volume(&coarse) > 0.0, "sphere must face outward");
        assert!(e_coarse < 0.02, "coarse error {e_coarse}");
        // Second-order discretization: an 8x refinement must shrink the error at least 30x.
        assert!(e_fine * 30.0 < e_coarse, "{e_fine} vs {e_coarse}");
        Ok(())
    }

    #[test]
    fn tube_and_torus_match_analytic_measures() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let tube = create_tube(1.5, 4.0, 3, 512, &ctx)?;
        assert!(TopologyIndex::build(&tube).is_closed_manifold());
        let exact_tube = PI * 1.5 * 1.5 * 4.0;
        assert!(relative_error(signed_volume(&tube), exact_tube) < 1e-3);

        let torus = create_torus(3.0, 1.0, 256, 128, &ctx)?;
        assert!(TopologyIndex::build(&torus).is_closed_manifold());
        let exact_volume = 2.0 * PI * PI * 3.0;
        let exact_area = 4.0 * PI * PI * 3.0;
        assert!(relative_error(signed_volume(&torus), exact_volume) < 1e-3);
        assert!(relative_error(f64::from(torus.surface_area()), exact_area) < 1e-3);
        Ok(())
    }

    #[test]
    fn line_spaces_points_evenly_and_invalid_parameters_are_rejected() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 20, &flag);
        let line = create_line(Vec3::ZERO, Vec3::checked(4.0, 0.0, 0.0)?, 5, &ctx)?;
        assert_eq!(line.len(), 5);
        assert_eq!(line.prim_count(), 1);
        assert_eq!(line.point(2).map(|p| p.x), Some(2.0));
        assert!(create_sphere(-1.0, 8, 8, &ctx).is_err());
        assert!(create_sphere(1.0, 1, 8, &ctx).is_err());
        assert!(create_tube(1.0, 1.0, 1, 2, &ctx).is_err());
        assert!(create_torus(1.0, 2.0, 8, 8, &ctx).is_err());
        assert_eq!(
            create_sphere(1.0, 4096, 4096, &SopContext::new(1024, &flag)).err(),
            Some(GeoError::LimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn a_tet_box_is_a_conforming_solid_of_the_right_size() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let size = Vec3 {
            x: 2.0,
            y: 3.0,
            z: 5.0,
        };
        for divisions in [1u32, 2, 5] {
            let n = divisions as usize;
            let geo = create_tet_box(size, divisions, &ctx)?;
            assert_eq!(geo.tet_count(), 6 * n * n * n);
            assert_eq!(geo.len(), (n + 1) * (n + 1) * (n + 1));
            assert!((geo.tet_volume()? - 30.0).abs() < 1e-3, "{divisions}");
            let (low, high) = geo.bounds().ok_or(GeoError::IndexOutOfBounds)?;
            assert_eq!(
                (low.x, low.y, low.z, high.x, high.y, high.z),
                (-1.0, -1.5, -2.5, 1.0, 1.5, 2.5)
            );
            // Extracting a surface refuses inverted or overlapping elements, so this also
            // proves that every element is positive and that neighbours conform.
            let surface = geo.tet_surface_cancellable(1 << 28, &flag)?;
            assert_eq!(surface.prim_count(), 12 * n * n);
            assert!(surface.topology_index().is_closed_manifold());
            assert!((surface.surface_area() - 62.0).abs() < 1e-3, "{divisions}");
            assert!(
                (surface.enclosed_volume() - 30.0).abs() < 1e-3,
                "{divisions}"
            );
            // Only a cell count above one leaves any point inside.
            let inside = (n + 1).pow(3) - (n.saturating_sub(1)).pow(3);
            assert_eq!(surface.len(), inside);
        }
        Ok(())
    }

    #[test]
    fn a_tet_box_refuses_bad_input_and_respects_its_budget() -> Result<(), GeoError> {
        let flag = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 28, &flag);
        let unit = Vec3 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        };
        let refused = Some(GeoError::InvalidTopology);
        assert_eq!(create_tet_box(unit, 0, &ctx).err(), refused);
        for flat in [
            Vec3 { x: 0.0, ..unit },
            Vec3 { y: 0.0, ..unit },
            Vec3 { z: -1.0, ..unit },
        ] {
            assert_eq!(create_tet_box(flat, 1, &ctx).err(), refused);
        }
        assert_eq!(
            create_tet_box(unit, u32::MAX, &ctx).err(),
            Some(GeoError::LimitExceeded)
        );
        assert_eq!(
            create_tet_box(unit, 64, &SopContext::new(1024, &flag)).err(),
            Some(GeoError::LimitExceeded)
        );
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            create_tet_box(unit, 1, &SopContext::new(1 << 28, &cancelled)).err(),
            Some(GeoError::Cancelled)
        );
        Ok(())
    }
}
