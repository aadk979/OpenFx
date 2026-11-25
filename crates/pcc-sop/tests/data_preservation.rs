//! Operators must carry the data they can, refuse the input they cannot handle, and survive
//! coordinates far from the origin.

use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, GeoError, GroupDomain, PrimKind, PrimRun};
use pcc_math::Vec3;
use pcc_sop::{
    RemeshParams, SopContext, clip_halfspace, fuse_points, poly_bevel, poly_reduce, remesh,
    triangulate,
};
use std::sync::atomic::AtomicBool;

fn ints(geo: &GeoDetail, domain: AttrDomain, name: &str) -> Vec<i32> {
    geo.domain_attribute(domain, name)
        .and_then(AttributeColumn::as_i32)
        .map(|column| (0..column.len()).filter_map(|i| column.get(i)).collect())
        .unwrap_or_default()
}

fn members(geo: &GeoDetail, domain: GroupDomain, name: &str) -> Option<Vec<usize>> {
    let group = geo.group(domain, name)?;
    Some((0..group.len()).filter(|&i| group.contains(i)).collect())
}

fn polygons(arity: u32, prim_count: usize) -> Vec<PrimRun> {
    vec![PrimRun {
        kind: PrimKind::Polygon,
        arity,
        prim_count,
    }]
}

#[test]
fn triangulate_carries_vertex_and_primitive_data() -> Result<(), GeoError> {
    let flag = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 20, &flag);
    // A concave quad, so the split is not simply a fan from the first corner.
    let quad = GeoDetail::points(vec![
        Vec3::checked(0.0, 0.0, 0.0)?,
        Vec3::checked(2.0, 0.0, 0.0)?,
        Vec3::checked(0.5, 0.5, 0.0)?,
        Vec3::checked(0.0, 2.0, 0.0)?,
        Vec3::checked(5.0, 5.0, 5.0)?,
        Vec3::checked(6.0, 5.0, 5.0)?,
    ])
    .with_primitives(
        vec![
            PrimRun {
                kind: PrimKind::Polygon,
                arity: 4,
                prim_count: 1,
            },
            PrimRun {
                kind: PrimKind::Polyline,
                arity: 2,
                prim_count: 1,
            },
        ],
        vec![0, 1, 2, 3, 4, 5],
    )?
    .with_domain_attribute(
        AttrDomain::Vertex,
        "corner",
        AttributeColumn::i32_values(vec![10, 11, 12, 13, 14, 15]),
    )?
    .with_domain_attribute(
        AttrDomain::Prim,
        "face",
        AttributeColumn::i32_values(vec![7, 9]),
    )?
    .with_group(GroupDomain::Prim, "surface", &[0])?
    .with_group(GroupDomain::Point, "tip", &[2])?;

    let out = triangulate(&quad, &ctx)?;
    assert_eq!(out.prim_count(), 3);
    assert_eq!(ints(&out, AttrDomain::Prim, "face"), vec![7, 7, 9]);
    assert_eq!(
        members(&out, GroupDomain::Prim, "surface"),
        Some(vec![0, 1])
    );
    assert_eq!(members(&out, GroupDomain::Point, "tip"), Some(vec![2]));
    // Every corner keeps the value its polygon corner had: here, ten plus its point.
    let corners = ints(&out, AttrDomain::Vertex, "corner");
    assert_eq!(corners.len(), 8);
    for (vertex, value) in corners.iter().enumerate() {
        assert_eq!(
            out.vertex_point(vertex).map(|p| p as i32 + 10),
            Some(*value)
        );
    }
    Ok(())
}

#[test]
fn fuse_carries_data_for_what_survives() -> Result<(), GeoError> {
    let flag = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 20, &flag);
    // Point 3 sits on point 1. The middle triangle collapses once they merge.
    let geo = GeoDetail::points(vec![
        Vec3::checked(0.0, 0.0, 0.0)?,
        Vec3::checked(1.0, 0.0, 0.0)?,
        Vec3::checked(0.0, 1.0, 0.0)?,
        Vec3::checked(1.0, 0.0, 0.0)?,
        Vec3::checked(2.0, 0.0, 0.0)?,
        Vec3::checked(2.0, 1.0, 0.0)?,
    ])
    .with_primitives(polygons(3, 3), vec![0, 1, 2, 1, 3, 2, 3, 4, 5])?
    .with_domain_attribute(
        AttrDomain::Vertex,
        "corner",
        AttributeColumn::i32_values((0..9).collect()),
    )?
    .with_domain_attribute(
        AttrDomain::Prim,
        "face",
        AttributeColumn::i32_values(vec![100, 200, 300]),
    )?
    .with_group(GroupDomain::Point, "far", &[4])?
    .with_group(GroupDomain::Prim, "last", &[2])?;

    let out = fuse_points(&geo, 1e-6, &ctx)?;
    assert_eq!((out.len(), out.prim_count()), (5, 2));
    assert_eq!(
        (0..6).map(|v| out.vertex_point(v)).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2), Some(1), Some(3), Some(4)]
    );
    assert_eq!(ints(&out, AttrDomain::Prim, "face"), vec![100, 300]);
    assert_eq!(
        ints(&out, AttrDomain::Vertex, "corner"),
        vec![0, 1, 2, 6, 7, 8]
    );
    assert_eq!(members(&out, GroupDomain::Point, "far"), Some(vec![3]));
    assert_eq!(members(&out, GroupDomain::Prim, "last"), Some(vec![1]));
    Ok(())
}

#[test]
fn fuse_handles_points_far_from_the_origin() -> Result<(), GeoError> {
    let flag = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 20, &flag);
    // With no extent the cell is tiny, so these positions saturate the cell index.
    let far = Vec3::checked(1e10, -1e10, 3e38)?;
    let out = fuse_points(&GeoDetail::points(vec![far; 3]), 0.0, &ctx)?;
    assert_eq!(out.len(), 1);
    assert_eq!(out.point(0), Some(far));
    Ok(())
}

#[test]
fn surface_operators_refuse_curves_beside_polygons() -> Result<(), GeoError> {
    let flag = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 24, &flag);
    let mixed = GeoDetail::points(vec![
        Vec3::checked(0.0, 0.0, 0.0)?,
        Vec3::checked(1.0, 0.0, 0.0)?,
        Vec3::checked(0.0, 1.0, 0.0)?,
        Vec3::checked(3.0, 3.0, 3.0)?,
    ])
    .with_primitives(
        vec![
            PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 1,
            },
            PrimRun {
                kind: PrimKind::Polyline,
                arity: 2,
                prim_count: 1,
            },
        ],
        vec![0, 1, 2, 2, 3],
    )?;
    let refused = Err(GeoError::InvalidTopology);
    let counts = |result: Result<GeoDetail, GeoError>| result.map(|geo| geo.prim_count());
    assert_eq!(
        counts(clip_halfspace(
            &mixed,
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            false,
            &ctx
        )),
        refused
    );
    assert_eq!(counts(poly_reduce(&mixed, 0.5, &ctx)), refused);
    assert_eq!(counts(poly_bevel(&mixed, 0.1, 30.0, &ctx)), refused);
    let params = RemeshParams {
        target_length: 0.25,
        iterations: 1,
        feature_angle_degrees: 30.0,
    };
    assert_eq!(counts(remesh(&mixed, &params, &ctx)), refused);

    // The same triangle alone is accepted, so the curve is what was refused.
    let triangle = GeoDetail::points(vec![
        Vec3::checked(0.0, 0.0, 0.0)?,
        Vec3::checked(1.0, 0.0, 0.0)?,
        Vec3::checked(0.0, 1.0, 0.0)?,
    ])
    .with_primitives(polygons(3, 1), vec![0, 1, 2])?;
    assert!(clip_halfspace(&triangle, [0.5, 0.0, 0.0], [1.0, 0.0, 0.0], false, &ctx).is_ok());
    assert!(remesh(&triangle, &params, &ctx).is_ok());
    Ok(())
}

#[test]
fn scatter_with_spacing_handles_enormous_surfaces() -> Result<(), GeoError> {
    let flag = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 24, &flag);
    // Spacing far below the surface's size saturates the cell index of the spacing grid.
    let huge = GeoDetail::points(vec![
        Vec3::checked(0.0, 0.0, 0.0)?,
        Vec3::checked(3e38, 0.0, 0.0)?,
        Vec3::checked(0.0, 3e38, 0.0)?,
    ])
    .with_primitives(polygons(3, 1), vec![0, 1, 2])?;
    let params = pcc_sop::SurfaceScatter {
        count: 16,
        seed: 7,
        min_distance: 1e-3,
    };
    let out = pcc_sop::scatter_surface(&huge, &params, &ctx)?;
    assert!(out.len() > 1 && out.len() <= 16);
    Ok(())
}
