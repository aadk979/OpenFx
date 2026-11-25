//! Voxelization must refuse geometry whose voxel indices do not fit, not wrap or panic.

use pcc_geo::{GeoDetail, GeoError, PrimKind, PrimRun, VdbGrid};
use pcc_math::Vec3;
use pcc_vdb::VdbOps;
use std::sync::atomic::AtomicBool;

/// A closed tetrahedron with its right-angle corner at `corner` and legs of `size`.
fn shell(corner: f32, size: f32) -> Result<GeoDetail, GeoError> {
    GeoDetail::points(vec![
        Vec3::checked(corner, corner, corner)?,
        Vec3::checked(corner + size, corner, corner)?,
        Vec3::checked(corner, corner + size, corner)?,
        Vec3::checked(corner, corner, corner + size)?,
    ])
    .with_primitives(
        vec![PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count: 4,
        }],
        vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
    )
}

fn voxelize(geo: &GeoDetail, voxel_size: f32) -> Result<usize, GeoError> {
    let cancelled = AtomicBool::new(false);
    VdbGrid::from_polygons_cancellable(geo, voxel_size, 3, 1 << 28, &cancelled)
        .map(|grid| grid.active_voxel_count())
}

#[test]
fn geometry_beyond_the_voxel_index_range_is_refused() -> Result<(), GeoError> {
    // The extent alone overflows a 32-bit index.
    assert_eq!(
        voxelize(&shell(0.0, 3e38)?, 0.25),
        Err(GeoError::LimitExceeded)
    );
    // A small shape whose position does: 1e10 / 0.25 is past the index range.
    assert_eq!(
        voxelize(&shell(1e10, 4096.0)?, 0.25),
        Err(GeoError::LimitExceeded)
    );
    // Exactly at the limit is refused too: the bound is exclusive.
    let limit = (i32::MAX / 2) as f32;
    assert_eq!(
        voxelize(&shell(limit, 4.0)?, 1.0),
        Err(GeoError::LimitExceeded)
    );
    Ok(())
}

#[test]
fn geometry_far_from_the_origin_but_in_range_still_voxelizes() -> Result<(), GeoError> {
    let near = voxelize(&shell(0.0, 4.0)?, 0.25)?;
    let far = voxelize(&shell(4096.0, 4.0)?, 0.25)?;
    assert!(near > 0);
    // Both positions are multiples of the voxel size, so the grids differ only by a shift.
    assert_eq!(near, far);
    Ok(())
}
