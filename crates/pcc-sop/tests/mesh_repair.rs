//! Each defect has a hand-built mesh with a result that can be stated exactly: how many shells, and
//! how much volume and area they hold. Manifold checks alone would pass a repair that shredded the
//! mesh into separate triangles.

use pcc_geo::{
    AttrDomain, AttributeColumn, GeoDetail, GeoError, GroupDomain, PrimKind, PrimRun, TopologyIndex,
};
use pcc_math::Vec3;
use pcc_sop::{FILLED_GROUP, SopContext, has_repeated_faces, mesh_repair, signed_volume};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

const BUDGET: usize = 1 << 30;

type Faces = Vec<Vec<u32>>;

fn mesh(points: &[[f32; 3]], faces: &[Vec<u32>]) -> Result<GeoDetail, GeoError> {
    let positions = points
        .iter()
        .map(|p| Vec3::checked(p[0], p[1], p[2]))
        .collect::<Result<Vec<_>, _>>()?;
    let runs = faces
        .iter()
        .map(|face| PrimRun {
            kind: PrimKind::Polygon,
            arity: face.len() as u32,
            prim_count: 1,
        })
        .collect();
    GeoDetail::points(positions).with_primitives(runs, faces.iter().flatten().copied().collect())
}

fn repair(geo: &GeoDetail, max_hole_edges: usize) -> Result<GeoDetail, GeoError> {
    let cancelled = AtomicBool::new(false);
    mesh_repair(geo, max_hole_edges, &SopContext::new(BUDGET, &cancelled))
}

fn count(geo: &GeoDetail, name: &str) -> i32 {
    match geo.domain_attribute(AttrDomain::Detail, name) {
        Some(AttributeColumn::I32(column)) => column.get(0).unwrap_or(-1),
        _ => -1,
    }
}

fn faces_of(geo: &GeoDetail) -> Faces {
    let mut faces = Vec::new();
    let mut vertex = 0usize;
    for run in geo.prim_runs() {
        for _ in 0..run.prim_count {
            let face = (vertex..vertex + run.arity as usize)
                .filter_map(|at| geo.vertex_point(at))
                .collect();
            faces.push(face);
            vertex += run.arity as usize;
        }
    }
    faces
}

/// Passes every test of a manifold surface.
fn assert_manifold(geo: &GeoDetail) {
    let topology = TopologyIndex::build(geo);
    assert_eq!(topology.non_manifold_edge_count(), 0);
    assert_eq!(topology.non_manifold_vertex_count(geo), 0);
    assert!(!has_repeated_faces(geo));
}

fn assert_solid(geo: &GeoDetail, volume: f64) {
    assert_manifold(geo);
    assert!(TopologyIndex::build(geo).is_closed_manifold());
    let actual = signed_volume(geo);
    assert!(
        (actual - volume).abs() < 1e-5,
        "volume {actual}, expected {volume}"
    );
}

/// Sorted sizes of the shells connected through shared points.
fn shell_sizes(geo: &GeoDetail) -> Vec<usize> {
    let faces = faces_of(geo);
    let mut parents: Vec<usize> = (0..geo.len()).collect();
    fn find(parents: &mut [usize], mut at: usize) -> usize {
        while parents[at] != at {
            parents[at] = parents[parents[at]];
            at = parents[at];
        }
        at
    }
    for face in &faces {
        for pair in face.windows(2) {
            let (a, b) = (
                find(&mut parents, pair[0] as usize),
                find(&mut parents, pair[1] as usize),
            );
            parents[a] = b;
        }
    }
    let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
    for face in &faces {
        *sizes
            .entry(find(&mut parents, face[0] as usize))
            .or_default() += 1;
    }
    let mut sizes: Vec<usize> = sizes.into_values().collect();
    sizes.sort_unstable();
    sizes
}

/// A unit cube's corners from `origin`, with outward quads.
fn cube_points(origin: [f32; 3]) -> Vec<[f32; 3]> {
    [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ]
    .iter()
    .map(|c| [c[0] + origin[0], c[1] + origin[1], c[2] + origin[2]])
    .collect()
}

fn cube_faces() -> Faces {
    vec![
        vec![0, 3, 2, 1],
        vec![4, 5, 6, 7],
        vec![0, 1, 5, 4],
        vec![2, 3, 7, 6],
        vec![3, 0, 4, 7],
        vec![1, 2, 6, 5],
    ]
}

fn cube() -> Result<GeoDetail, GeoError> {
    mesh(&cube_points([0.; 3]), &cube_faces())
}

/// Unit cubes at `origins`, sharing any points that coincide.
fn welded_cubes(origins: &[[f32; 3]]) -> (Vec<[f32; 3]>, Faces) {
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut faces = Vec::new();
    for &origin in origins {
        let corners = cube_points(origin);
        let map: Vec<u32> = corners
            .iter()
            .map(|corner| {
                let found = points.iter().position(|point| point == corner);
                found.unwrap_or_else(|| {
                    points.push(*corner);
                    points.len() - 1
                }) as u32
            })
            .collect();
        for face in cube_faces() {
            faces.push(face.iter().map(|&at| map[at as usize]).collect());
        }
    }
    (points, faces)
}

fn triangulated(faces: &[Vec<u32>]) -> Faces {
    faces
        .iter()
        .flat_map(|face| (1..face.len() - 1).map(|k| vec![face[0], face[k], face[k + 1]]))
        .collect()
}

#[test]
fn clean_input_comes_back_unchanged() -> Result<(), GeoError> {
    let solid = cube()?;
    assert_eq!(repair(&solid, 64)?.hash(), solid.hash());

    let open = mesh(&cube_points([0.; 3]), &cube_faces()[1..])?;
    assert_eq!(repair(&open, 0)?.hash(), open.hash());
    assert_eq!(repair(&open, 3)?.hash(), open.hash());

    let points = GeoDetail::points(vec![Vec3::ZERO]);
    assert_eq!(repair(&points, 0)?.hash(), points.hash());
    Ok(())
}

#[test]
fn inside_out_solids_and_cavities_keep_their_direction() -> Result<(), GeoError> {
    let inside_out: Faces = cube_faces()
        .into_iter()
        .map(|face| face.into_iter().rev().collect())
        .collect();
    let geo = mesh(&cube_points([0.; 3]), &inside_out)?;
    assert_eq!(repair(&geo, 0)?.hash(), geo.hash());

    // A hollow box: the inner shell faces inward, so flipping it to match would fill the cavity.
    let mut points = cube_points([0.; 3]);
    points.iter_mut().for_each(|p| *p = p.map(|c| c * 3.0));
    points.extend(cube_points([1.; 3]));
    let mut faces = cube_faces();
    faces.extend(
        cube_faces()
            .into_iter()
            .map(|face| face.into_iter().rev().map(|at| at + 8).collect::<Vec<_>>()),
    );
    let hollow = mesh(&points, &faces)?;
    assert_eq!(repair(&hollow, 0)?.hash(), hollow.hash());
    assert_solid(&hollow, 26.0);
    Ok(())
}

#[test]
fn degenerate_faces_are_removed_and_repeated_corners_collapsed() -> Result<(), GeoError> {
    let mut faces = cube_faces();
    faces.push(vec![0, 0, 1]);
    faces.push(vec![5, 5, 5]);
    // One point twice, apart: a figure of eight.
    faces.push(vec![0, 1, 2, 0, 3, 4]);
    // Repeats side by side and across the wrap: still the cube's top, once they collapse.
    faces[1] = vec![4, 5, 5, 6, 7, 4];
    let out = repair(&mesh(&cube_points([0.; 3]), &faces)?, 0)?;
    assert_solid(&out, 1.0);
    assert_eq!(out.prim_count(), 6);
    assert_eq!(count(&out, "repair_degenerate_faces"), 3);
    assert_eq!(faces_of(&out)[1], vec![4, 5, 6, 7]);
    Ok(())
}

#[test]
fn every_triangle_stored_three_times_becomes_one() -> Result<(), GeoError> {
    let once = triangulated(&cube_faces());
    let thrice: Faces = once
        .iter()
        .flat_map(|face| std::iter::repeat_n(face.clone(), 3))
        .collect();
    let out = repair(&mesh(&cube_points([0.; 3]), &thrice)?, 0)?;
    assert_solid(&out, 1.0);
    assert_eq!(out.prim_count(), 12);
    assert_eq!(count(&out, "repair_duplicate_faces"), 24);
    assert_eq!(count(&out, "repair_back_to_back_faces"), 0);
    assert_eq!(out.len(), 8);
    assert_eq!(count(&out, "repair_split_points"), 0);
    Ok(())
}

#[test]
fn repeated_faces_keep_the_direction_most_of_them_face() -> Result<(), GeoError> {
    let points = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
    let forward = vec![0, 1, 2];
    let backward = vec![2, 1, 0];
    let rotated_backward = vec![1, 0, 2];

    let out = repair(
        &mesh(&points, &[forward.clone(), backward.clone(), vec![1, 2, 0]])?,
        0,
    )?;
    assert_eq!(faces_of(&out), vec![forward.clone()]);
    assert_eq!(count(&out, "repair_duplicate_faces"), 2);

    let out = repair(
        &mesh(
            &points,
            &[forward.clone(), backward.clone(), rotated_backward],
        )?,
        0,
    )?;
    assert_eq!(faces_of(&out), vec![backward]);
    assert_eq!(count(&out, "repair_duplicate_faces"), 2);
    assert_eq!(count(&out, "repair_back_to_back_faces"), 0);
    Ok(())
}

#[test]
fn an_internal_wall_between_two_solids_cancels() -> Result<(), GeoError> {
    let (points, faces) = welded_cubes(&[[0.; 3], [1., 0., 0.]]);
    let out = repair(&mesh(&points, &faces)?, 0)?;
    assert_solid(&out, 2.0);
    assert_eq!(out.prim_count(), 10);
    assert_eq!(count(&out, "repair_back_to_back_faces"), 2);
    assert_eq!(count(&out, "repair_duplicate_faces"), 0);
    Ok(())
}

#[test]
fn a_mesh_that_encloses_nothing_is_an_error_not_an_empty_result() -> Result<(), GeoError> {
    let points = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
    let doubled = mesh(&points, &[vec![0, 1, 2], vec![2, 1, 0]])?;
    assert!(matches!(
        repair(&doubled, 0),
        Err(GeoError::InvalidTopology)
    ));

    let nothing_left = mesh(&points, &[vec![0, 0, 1]])?;
    assert!(matches!(
        repair(&nothing_left, 0),
        Err(GeoError::InvalidTopology)
    ));
    Ok(())
}

#[test]
fn a_flipped_face_joins_its_neighbours_winding() -> Result<(), GeoError> {
    for flipped in 0..6 {
        let mut faces = cube_faces();
        faces[flipped].reverse();
        let out = repair(&mesh(&cube_points([0.; 3]), &faces)?, 0)?;
        assert_solid(&out, 1.0);
        assert_eq!(count(&out, "repair_flipped_faces"), 1);
        assert_eq!(out.len(), 8);
    }
    Ok(())
}

#[test]
fn a_patch_keeps_the_winding_of_most_of_its_area() -> Result<(), GeoError> {
    // A small quad beside a large one, wound against it. Whichever comes first, the small one turns.
    let points = [
        [0., 0., 0.],
        [10., 0., 0.],
        [10., 10., 0.],
        [0., 10., 0.],
        [11., 0., 0.],
        [11., 10., 0.],
    ];
    let large = vec![0, 1, 2, 3];
    let small = vec![1, 2, 5, 4];
    let turned = vec![4, 5, 2, 1];

    let out = repair(&mesh(&points, &[large.clone(), small.clone()])?, 0)?;
    assert_eq!(faces_of(&out), vec![large.clone(), turned.clone()]);
    assert_eq!(count(&out, "repair_flipped_faces"), 1);

    let out = repair(&mesh(&points, &[small, large.clone()])?, 0)?;
    assert_eq!(faces_of(&out), vec![turned, large]);
    assert_eq!(count(&out, "repair_flipped_faces"), 1);
    Ok(())
}

#[test]
fn cubes_sharing_an_edge_separate_into_their_own_solids() -> Result<(), GeoError> {
    // Every axis and both diagonals, so the sweep around the edge starts at every position.
    for origin in [
        [1., 1., 0.],
        [1., -1., 0.],
        [-1., 1., 0.],
        [-1., -1., 0.],
        [1., 0., 1.],
        [1., 0., -1.],
        [-1., 0., 1.],
        [-1., 0., -1.],
        [0., 1., 1.],
        [0., 1., -1.],
        [0., -1., 1.],
        [0., -1., -1.],
    ] {
        let (points, faces) = welded_cubes(&[[0.; 3], origin]);
        let geo = mesh(&points, &faces)?;
        assert!(!TopologyIndex::build(&geo).is_manifold());
        let out = repair(&geo, 0)?;
        assert_solid(&out, 2.0);
        assert_eq!(shell_sizes(&out), vec![6, 6], "around {origin:?}");
        assert_eq!(count(&out, "repair_split_points"), 2);
        assert_eq!(out.len(), points.len() + 2);
        assert_eq!(count(&out, "repair_flipped_faces"), 0);
    }
    Ok(())
}

#[test]
fn four_cubes_around_one_edge_become_four_solids() -> Result<(), GeoError> {
    let (points, faces) = welded_cubes(&[[0.; 3], [1., 1., 0.], [1., 3., 0.], [3., 1., 0.]]);
    let out = repair(&mesh(&points, &faces)?, 0)?;
    assert_solid(&out, 4.0);
    assert_eq!(shell_sizes(&out), vec![6, 6, 6, 6]);
    Ok(())
}

#[test]
fn inside_out_cubes_sharing_an_edge_also_separate() -> Result<(), GeoError> {
    let (points, faces) = welded_cubes(&[[0.; 3], [1., 1., 0.]]);
    let reversed: Faces = faces
        .iter()
        .map(|face| face.iter().rev().copied().collect())
        .collect();
    let out = repair(&mesh(&points, &reversed)?, 0)?;
    assert_solid(&out, -2.0);
    assert_eq!(shell_sizes(&out), vec![6, 6]);

    // One each way: each cube's own faces must still find each other.
    let mixed: Faces = faces
        .iter()
        .enumerate()
        .map(|(at, face)| {
            if at < 6 {
                face.clone()
            } else {
                face.iter().rev().copied().collect()
            }
        })
        .collect();
    let out = repair(&mesh(&points, &mixed)?, 0)?;
    assert_solid(&out, 0.0);
    assert_eq!(shell_sizes(&out), vec![6, 6]);
    Ok(())
}

#[test]
fn a_fin_on_a_solid_comes_away_and_leaves_it_closed() -> Result<(), GeoError> {
    let mut points = cube_points([0.; 3]);
    points.push([0.5, -1.0, -1.0]);
    // Both windings: one opens the bracket sequence around the edge and the other closes it.
    for fin in [vec![0, 1, 8], vec![1, 0, 8]] {
        let mut faces = cube_faces();
        faces.push(fin);
        let out = repair(&mesh(&points, &faces)?, 0)?;
        assert_manifold(&out);
        assert_eq!(shell_sizes(&out), vec![1, 6]);
        assert_eq!(count(&out, "repair_split_points"), 2);
        assert_eq!(count(&out, "repair_open_holes"), 1);
        assert!((signed_volume(&out) - 1.0).abs() < 1e-5);
    }
    Ok(())
}

#[test]
fn solids_touching_at_one_point_are_split_there() -> Result<(), GeoError> {
    let (points, faces) = welded_cubes(&[[0.; 3], [1., 1., 1.]]);
    let geo = mesh(&points, &faces)?;
    let topology = TopologyIndex::build(&geo);
    assert!(topology.is_closed_manifold());
    assert_eq!(topology.non_manifold_vertex_count(&geo), 1);
    let out = repair(&geo, 0)?;
    assert_solid(&out, 2.0);
    assert_eq!(shell_sizes(&out), vec![6, 6]);
    assert_eq!(count(&out, "repair_split_points"), 1);
    assert_eq!(out.len(), points.len() + 1);
    Ok(())
}

fn mobius(segments: u32) -> Result<GeoDetail, GeoError> {
    let mut points = Vec::new();
    for at in 0..segments {
        let turn = f32::from(at as u16) / f32::from(segments as u16) * std::f32::consts::TAU;
        let (sin, cos) = turn.sin_cos();
        let (twist_sin, twist_cos) = (turn / 2.0).sin_cos();
        for side in [-0.2, 0.2] {
            let radius = 1.0 + side * twist_cos;
            points.push([radius * cos, radius * sin, side * twist_sin]);
        }
    }
    let mut faces: Faces = (0..segments - 1)
        .map(|at| {
            let a = 2 * at;
            vec![a, a + 2, a + 3, a + 1]
        })
        .collect();
    // The twist: the last segment joins the first the other way up.
    let last = 2 * (segments - 1);
    faces.push(vec![last, 1, 0, last + 1]);
    mesh(&points, &faces)
}

#[test]
fn a_mobius_strip_is_cut_because_it_has_no_consistent_winding() -> Result<(), GeoError> {
    for segments in [5, 6, 7, 16] {
        let strip = mobius(segments)?;
        assert!(!TopologyIndex::build(&strip).is_manifold());
        let out = repair(&strip, 0)?;
        assert_manifold(&out);
        assert_eq!(out.prim_count(), segments as usize);
        assert!(count(&out, "repair_split_points") > 0);
        // Still one strip, cut once across: more copies would mean it had been shredded.
        assert_eq!(shell_sizes(&out), vec![segments as usize]);
    }
    Ok(())
}

#[test]
fn a_hole_is_filled_only_when_the_limit_allows() -> Result<(), GeoError> {
    let open = mesh(&cube_points([0.; 3]), &cube_faces()[1..])?;
    let out = repair(&open, 4)?;
    assert_solid(&out, 1.0);
    assert_eq!(out.prim_count(), 7);
    assert_eq!(count(&out, "repair_filled_holes"), 1);
    assert_eq!(count(&out, "repair_filled_faces"), 2);
    assert_eq!(count(&out, "repair_open_holes"), 0);
    assert!((f64::from(out.surface_area()) - 6.0).abs() < 1e-5);
    let filled = out.group(GroupDomain::Prim, FILLED_GROUP);
    assert_eq!(filled.map(pcc_geo::ElementGroup::selected_count), Some(2));
    assert_eq!(filled.map(|group| group.contains(4)), Some(false));
    assert_eq!(filled.map(|group| group.contains(5)), Some(true));
    assert_eq!(filled.map(|group| group.contains(6)), Some(true));
    Ok(())
}

#[test]
fn holes_left_open_are_counted_when_something_else_was_repaired() -> Result<(), GeoError> {
    let mut faces = cube_faces();
    faces[0] = faces[1].clone();
    let out = repair(&mesh(&cube_points([0.; 3]), &faces)?, 0)?;
    assert_eq!(count(&out, "repair_duplicate_faces"), 1);
    assert_eq!(count(&out, "repair_open_holes"), 1);
    assert_eq!(count(&out, "repair_filled_holes"), 0);
    assert!(out.group(GroupDomain::Prim, FILLED_GROUP).is_none());
    Ok(())
}

#[test]
fn filling_a_concave_hole_stays_inside_it() -> Result<(), GeoError> {
    // An L-shaped prism with no caps. Any triangle outside the L would add area.
    let outline = [[0., 0.], [2., 0.], [2., 1.], [1., 1.], [1., 2.], [0., 2.]];
    let mut points = Vec::new();
    for height in [0.0, 1.0] {
        points.extend(outline.iter().map(|p| [p[0], p[1], height]));
    }
    let count_around = outline.len() as u32;
    let walls: Faces = (0..count_around)
        .map(|at| {
            let next = (at + 1) % count_around;
            vec![at, next, next + count_around, at + count_around]
        })
        .collect();
    let out = repair(&mesh(&points, &walls)?, 6)?;
    assert_solid(&out, 3.0);
    assert_eq!(count(&out, "repair_filled_holes"), 2);
    assert_eq!(count(&out, "repair_filled_faces"), 8);
    let wall_area = 8.0;
    assert!((f64::from(out.surface_area()) - (wall_area + 6.0)).abs() < 1e-5);
    Ok(())
}

#[test]
fn filling_never_reuses_an_edge_the_mesh_already_has() -> Result<(), GeoError> {
    // Two faces of a tetrahedron leave a four-sided hole. Its shorter diagonal is the edge they
    // already share, and using it would give each face a back-to-back twin.
    let points = [[0., 0., 0.], [4., 0., 0.], [2., 3., 3.], [2., -3., 3.]];
    let out = repair(&mesh(&points, &[vec![0, 1, 2], vec![1, 0, 3]])?, 4)?;
    assert_manifold(&out);
    assert!(TopologyIndex::build(&out).is_closed_manifold());
    assert_eq!(out.prim_count(), 4);
    assert!(signed_volume(&out).abs() > 1.0);
    Ok(())
}

#[test]
fn a_lone_triangle_is_not_filled_with_its_own_back() -> Result<(), GeoError> {
    let lone = mesh(
        &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        &[vec![0, 1, 2]],
    )?;
    assert_eq!(repair(&lone, 64)?.hash(), lone.hash());
    Ok(())
}

fn cone(sides: u32) -> Result<GeoDetail, GeoError> {
    let mut points = vec![[0., 0., 1.]];
    for at in 0..sides {
        let (sin, cos) = (f64::from(at) / f64::from(sides) * std::f64::consts::TAU).sin_cos();
        points.push([cos as f32, sin as f32, 0.]);
    }
    let faces: Faces = (0..sides)
        .map(|at| vec![0, at + 1, (at + 1) % sides + 1])
        .collect();
    mesh(&points, &faces)
}

#[test]
fn the_limit_on_hole_size_is_exact() -> Result<(), GeoError> {
    let eight = cone(8)?;
    assert_eq!(repair(&eight, 7)?.hash(), eight.hash());
    let out = repair(&eight, 8)?;
    assert!(TopologyIndex::build(&out).is_closed_manifold());
    assert_eq!(count(&out, "repair_filled_faces"), 6);

    let largest = cone(512)?;
    let out = repair(&largest, usize::MAX)?;
    assert!(TopologyIndex::build(&out).is_closed_manifold());
    let too_large = cone(513)?;
    assert_eq!(repair(&too_large, usize::MAX)?.hash(), too_large.hash());
    Ok(())
}

#[test]
fn split_points_carry_their_attributes_and_groups() -> Result<(), GeoError> {
    let (points, faces) = welded_cubes(&[[0.; 3], [1., 1., 0.]]);
    let heat: Vec<f32> = (0..points.len()).map(|at| at as f32).collect();
    // Faces 0 and 11 are in the group: one from each cube.
    let geo = mesh(&points, &faces)?
        .with_attribute("heat", AttributeColumn::f32_checked(heat)?)?
        .with_domain_attribute(
            AttrDomain::Detail,
            "note",
            AttributeColumn::i32_values(vec![7]),
        )?
        .with_group(GroupDomain::Point, "shared", &[2, 6])?
        .with_group(GroupDomain::Prim, "picked", &[0, 11])?;
    let out = repair(&geo, 0)?;
    let copies = [out.len() - 2, out.len() - 1];
    let Some(AttributeColumn::F32(column)) = out.attribute("heat") else {
        return Err(GeoError::AttributeNotFound);
    };
    for (original, copy) in [2usize, 6].into_iter().zip(copies) {
        assert_eq!(out.point(copy), out.point(original));
        assert_eq!(
            column.get(copy).map(|v| v.get()),
            column.get(original).map(|v| v.get())
        );
        let group = out.group(GroupDomain::Point, "shared");
        assert_eq!(group.map(|g| g.contains(copy)), Some(true));
    }
    assert_eq!(
        out.group(GroupDomain::Point, "shared")
            .map(pcc_geo::ElementGroup::selected_count),
        Some(4)
    );
    let picked = out.group(GroupDomain::Prim, "picked");
    assert_eq!(picked.map(pcc_geo::ElementGroup::selected_count), Some(2));
    assert_eq!(picked.map(|g| g.contains(11)), Some(true));
    assert_eq!(count(&out, "note"), 7);
    Ok(())
}

#[test]
fn face_data_follows_its_face_through_removal_and_flipping() -> Result<(), GeoError> {
    let mut faces = cube_faces();
    faces.insert(0, vec![0, 0, 1]);
    faces[1].reverse();
    let corners: usize = faces.iter().map(Vec::len).sum();
    let ids: Vec<i32> = (0..faces.len() as i32).collect();
    let marks: Vec<i32> = (0..corners as i32).collect();
    let geo = mesh(&cube_points([0.; 3]), &faces)?
        .with_domain_attribute(AttrDomain::Prim, "id", AttributeColumn::i32_values(ids))?
        .with_domain_attribute(
            AttrDomain::Vertex,
            "mark",
            AttributeColumn::i32_values(marks),
        )?;
    let out = repair(&geo, 0)?;
    assert_solid(&out, 1.0);
    let read = |domain, name, at| match out.domain_attribute(domain, name) {
        Some(AttributeColumn::I32(column)) => column.get(at),
        _ => None,
    };
    // The degenerate face was first, so everything moved down one.
    assert_eq!(read(AttrDomain::Prim, "id", 0), Some(1));
    assert_eq!(read(AttrDomain::Prim, "id", 5), Some(6));
    // Its three corners are gone, and the flipped face holds its own in reversed order.
    assert_eq!(faces_of(&out)[0], vec![0, 3, 2, 1]);
    for (at, expected) in [(0, 6), (1, 5), (2, 4), (3, 3)] {
        assert_eq!(read(AttrDomain::Vertex, "mark", at), Some(expected));
    }
    assert_eq!(read(AttrDomain::Vertex, "mark", 4), Some(7));
    Ok(())
}

#[test]
fn filled_faces_borrow_data_from_the_face_beside_them() -> Result<(), GeoError> {
    let faces = cube_faces()[1..].to_vec();
    let ids: Vec<i32> = (0..faces.len() as i32).map(|at| 100 + at).collect();
    let marks: Vec<i32> = (0..20).collect();
    let geo = mesh(&cube_points([0.; 3]), &faces)?
        .with_domain_attribute(AttrDomain::Prim, "id", AttributeColumn::i32_values(ids))?
        .with_domain_attribute(
            AttrDomain::Vertex,
            "mark",
            AttributeColumn::i32_values(marks),
        )?;
    let out = repair(&geo, 4)?;
    let Some(AttributeColumn::I32(id)) = out.domain_attribute(AttrDomain::Prim, "id") else {
        return Err(GeoError::AttributeNotFound);
    };
    assert!(id.get(5).is_some_and(|value| (100..105).contains(&value)));
    assert!(id.get(6).is_some_and(|value| (100..105).contains(&value)));
    let Some(AttributeColumn::I32(mark)) = out.domain_attribute(AttrDomain::Vertex, "mark") else {
        return Err(GeoError::AttributeNotFound);
    };
    // Each filled corner takes the mark of an input corner at the same point.
    let source = faces.iter().flatten().copied().collect::<Vec<u32>>();
    for vertex in 20..26 {
        let point = out.vertex_point(vertex);
        let from = mark.get(vertex).map(|value| source[value as usize]);
        assert_eq!(point, from);
    }
    Ok(())
}

#[test]
fn a_filled_group_from_an_earlier_repair_is_not_inherited() -> Result<(), GeoError> {
    let open = mesh(&cube_points([0.; 3]), &cube_faces()[1..])?.with_group(
        GroupDomain::Prim,
        FILLED_GROUP,
        &[0, 1],
    )?;
    let out = repair(&open, 4)?;
    let group = out.group(GroupDomain::Prim, FILLED_GROUP);
    assert_eq!(group.map(pcc_geo::ElementGroup::selected_count), Some(2));
    assert_eq!(group.map(|g| g.contains(0)), Some(false));
    Ok(())
}

#[test]
fn anything_but_polygons_is_refused_rather_than_lost() -> Result<(), GeoError> {
    let lines = GeoDetail::points(vec![Vec3::ZERO; 2]).with_primitives(
        vec![PrimRun {
            kind: PrimKind::Polyline,
            arity: 2,
            prim_count: 1,
        }],
        vec![0, 1],
    )?;
    assert!(matches!(repair(&lines, 0), Err(GeoError::InvalidTopology)));
    Ok(())
}

#[test]
fn repair_is_idempotent_and_deterministic() -> Result<(), GeoError> {
    let (two, two_faces) = welded_cubes(&[[0.; 3], [1., 1., 0.]]);
    let (four, four_faces) = welded_cubes(&[[0.; 3], [1., 1., 0.], [1., 3., 0.], [3., 1., 0.]]);
    let mut flipped = cube_faces();
    flipped[2].reverse();
    let mut tripled = triangulated(&cube_faces());
    tripled.extend(tripled.clone());
    let cases = [
        mesh(&two, &two_faces)?,
        mesh(&four, &four_faces)?,
        mesh(&cube_points([0.; 3]), &flipped)?,
        mesh(&cube_points([0.; 3]), &tripled)?,
        mobius(7)?,
        cone(9)?,
    ];
    for case in &cases {
        for limit in [0, 4, 64] {
            let once = repair(case, limit)?;
            assert_eq!(repair(case, limit)?.hash(), once.hash());
            assert_eq!(repair(&once, limit)?.hash(), once.hash());
            assert_manifold(&once);
        }
    }
    Ok(())
}

#[test]
fn cancellation_and_the_memory_budget_are_honoured() -> Result<(), GeoError> {
    let (points, faces) = welded_cubes(&[[0.; 3], [1., 1., 0.]]);
    let geo = mesh(&points, &faces)?;
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        mesh_repair(&geo, 0, &SopContext::new(BUDGET, &cancelled)),
        Err(GeoError::Cancelled)
    ));
    let running = AtomicBool::new(false);
    assert!(matches!(
        mesh_repair(&geo, 0, &SopContext::new(16, &running)),
        Err(GeoError::LimitExceeded)
    ));
    let hole = cone(16)?;
    // Enough for the mesh, not for the table that triangulates its hole.
    assert!(matches!(
        mesh_repair(&hole, 16, &SopContext::new(1024, &running)),
        Err(GeoError::LimitExceeded)
    ));
    Ok(())
}
