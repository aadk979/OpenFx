//! A terrain is read back only from geometry that really is its grid.

use pcc_geo::{GeoDetail, GeoError, PrimKind, PrimRun};
use pcc_math::Vec3;
use pcc_terrain::HeightField;
use std::sync::atomic::AtomicBool;

fn quads(points: Vec<Vec3>, prim_count: usize, vertices: Vec<u32>) -> Result<GeoDetail, GeoError> {
    GeoDetail::points(points).with_primitives(
        vec![PrimRun {
            kind: PrimKind::Polygon,
            arity: 4,
            prim_count,
        }],
        vertices,
    )
}

fn read(geo: &GeoDetail) -> Result<(usize, usize), GeoError> {
    let cancelled = AtomicBool::new(false);
    HeightField::from_geo_detail_cancellable(geo, 1 << 24, &cancelled)
        .map(|field| (field.rows(), field.cols()))
}

/// Rows of `cols` points in the XZ plane, wound the way the `grid` node winds them.
fn grid(rows: u32, cols: u32) -> Result<GeoDetail, GeoError> {
    let mut points = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            points.push(Vec3::checked(c as f32, 0.0, r as f32)?);
        }
    }
    let mut vertices = Vec::new();
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            let corner = r * cols + c;
            vertices.extend([corner, corner + 1, corner + cols + 1, corner + cols]);
        }
    }
    quads(points, ((rows - 1) * (cols - 1)) as usize, vertices)
}

#[test]
fn a_grid_is_recognised_with_or_without_its_size_attributes() -> Result<(), GeoError> {
    assert_eq!(read(&grid(3, 4)?), Ok((3, 4)));
    assert_eq!(read(&grid(2, 2)?), Ok((2, 2)));
    let cancelled = AtomicBool::new(false);
    let round_trip =
        HeightField::flat(5, 3, 4.0, 2.0, 0.0)?.to_geo_detail_cancellable(1 << 24, &cancelled)?;
    assert_eq!(read(&round_trip), Ok((5, 3)));
    Ok(())
}

#[test]
fn a_box_is_not_a_terrain() -> Result<(), GeoError> {
    let v = |x: f32, y: f32, z: f32| Vec3::checked(x, y, z);
    // Eight points and a fourth vertex index of one used to read as a four-by-two grid.
    let cube = quads(
        vec![
            v(0., 0., 0.)?,
            v(1., 0., 0.)?,
            v(1., 1., 0.)?,
            v(0., 1., 0.)?,
            v(0., 0., 1.)?,
            v(1., 0., 1.)?,
            v(1., 1., 1.)?,
            v(0., 1., 1.)?,
        ],
        6,
        vec![
            0, 3, 2, 1, 4, 5, 6, 7, 0, 1, 5, 4, 3, 7, 6, 2, 0, 4, 7, 3, 1, 2, 6, 5,
        ],
    )?;
    assert_eq!(read(&cube), Err(GeoError::InvalidTopology));
    Ok(())
}

#[test]
fn a_grid_with_changed_topology_is_refused() -> Result<(), GeoError> {
    // The right point count and quad count, but one quad wound the other way.
    let points = (0..6)
        .map(|i| Vec3::checked((i % 3) as f32, 0.0, (i / 3) as f32))
        .collect::<Result<Vec<_>, _>>()?;
    let reversed = quads(points.clone(), 2, vec![0, 1, 4, 3, 5, 4, 1, 2])?;
    assert_eq!(read(&reversed), Err(GeoError::InvalidTopology));
    // A quad missing: the points still divide evenly into rows.
    let missing = quads(points.clone(), 1, vec![0, 1, 4, 3])?;
    assert_eq!(read(&missing), Err(GeoError::InvalidTopology));
    // Triangles over the same points.
    let triangles = GeoDetail::points(points).with_primitives(
        vec![PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count: 2,
        }],
        vec![0, 1, 4, 0, 4, 3],
    )?;
    assert_eq!(read(&triangles), Err(GeoError::InvalidTopology));
    Ok(())
}
