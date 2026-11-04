//! Small hand-built reductions of the defects found in real models, run through every operator.
//! These keep the torture run's lessons in normal CI, which never downloads a corpus.

use pcc_geo::{GeoDetail, GeoError, PrimKind, PrimRun};
use pcc_math::Vec3;
use pcc_sop::SopContext;
use pcc_torture::{OPERATORS, Status, input_facts, prepare, run_operator};
use std::sync::atomic::AtomicBool;

type Mesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

fn build((points, triangles): &Mesh) -> Result<GeoDetail, GeoError> {
    let positions = points
        .iter()
        .map(|p| Vec3::checked(p[0], p[1], p[2]))
        .collect::<Result<Vec<_>, _>>()?;
    let runs = if triangles.is_empty() {
        Vec::new()
    } else {
        vec![PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count: triangles.len(),
        }]
    };
    GeoDetail::points(positions)
        .with_primitives(runs, triangles.iter().flatten().copied().collect())
}

fn cube() -> Mesh {
    let points = vec![
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ];
    let triangles = vec![
        [0, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [2, 3, 7],
        [2, 7, 6],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    (points, triangles)
}

fn offset_copy((points, triangles): &Mesh, by: [f32; 3]) -> Mesh {
    let base = points.len() as u32;
    let mut merged = points.clone();
    merged.extend(
        points
            .iter()
            .map(|p| [p[0] + by[0], p[1] + by[1], p[2] + by[2]]),
    );
    let mut faces = triangles.clone();
    faces.extend(
        triangles
            .iter()
            .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
    );
    (merged, faces)
}

/// One model per defect class from the Thingi10K metadata.
fn nasty_meshes() -> Vec<(&'static str, Mesh)> {
    let (points, triangles) = cube();
    let mut cases = vec![("clean_cube", cube())];

    let inverted = triangles.iter().map(|t| [t[2], t[1], t[0]]).collect();
    cases.push(("inside_out", (points.clone(), inverted)));

    let mut one_flipped = triangles.clone();
    one_flipped[3] = [one_flipped[3][2], one_flipped[3][1], one_flipped[3][0]];
    cases.push(("one_flipped_face", (points.clone(), one_flipped)));

    cases.push(("open_box", (points.clone(), triangles[2..].to_vec())));

    let mut duplicated = triangles.clone();
    duplicated.extend_from_slice(&triangles[..4]);
    cases.push(("duplicated_faces", (points.clone(), duplicated)));

    // A fin: a third face on one cube edge, which makes that edge non-manifold.
    let mut fin_points = points.clone();
    fin_points.push([0.5, -1.0, -1.0]);
    let mut fin = triangles.clone();
    fin.push([0, 1, 8]);
    cases.push(("non_manifold_fin", (fin_points, fin)));

    // Two cubes touching at exactly one corner: a pinched, non-manifold vertex.
    let mut pinch = offset_copy(&cube(), [1.0, 1.0, 1.0]);
    for triangle in &mut pinch.1 {
        for vertex in triangle.iter_mut() {
            if *vertex == 8 {
                *vertex = 6;
            }
        }
    }
    cases.push(("pinched_vertex", pinch));

    cases.push(("overlapping_shells", offset_copy(&cube(), [0.5, 0.5, 0.5])));
    cases.push(("separate_shells", offset_copy(&cube(), [5.0, 0.0, 0.0])));

    // Degenerate faces: a repeated index, and collinear corners with zero area.
    let mut degenerate_points = points.clone();
    degenerate_points.push([0.5, 0.0, 0.0]);
    let mut degenerate = triangles.clone();
    degenerate.push([0, 0, 1]);
    degenerate.push([0, 8, 1]);
    cases.push(("degenerate_faces", (degenerate_points, degenerate)));

    // A sliver with an aspect ratio near a million, attached to nothing.
    cases.push((
        "extreme_sliver",
        (
            vec![[0., 0., 0.], [1000., 0., 0.], [500., 0.0005, 0.]],
            vec![[0, 1, 2]],
        ),
    ));

    // A flat sheet: closed by no one, and thinner than any voxel.
    cases.push((
        "flat_sheet",
        (
            vec![[0., 0., 0.], [10., 0., 0.], [10., 10., 0.], [0., 10., 0.]],
            vec![[0, 1, 2], [0, 2, 3]],
        ),
    ));

    // A closed, zero-volume, doubled sheet.
    cases.push((
        "zero_volume_shell",
        (
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            vec![[0, 1, 2], [0, 2, 1]],
        ),
    ));

    // Unwelded triangle soup: every triangle has its own three points.
    let mut soup_points = Vec::new();
    let mut soup = Vec::new();
    for t in &triangles {
        let base = soup_points.len() as u32;
        soup_points.extend(t.iter().map(|&v| points[v as usize]));
        soup.push([base, base + 1, base + 2]);
    }
    cases.push(("triangle_soup", (soup_points, soup)));

    // Interlocking cubes with faces crossing but no shared vertex.
    cases.push(("self_intersecting", offset_copy(&cube(), [0.3, -0.2, 0.25])));
    cases
}

#[test]
fn every_operator_survives_every_defect_class() -> Result<(), GeoError> {
    let mut failures = Vec::new();
    for (name, mesh) in nasty_meshes() {
        let cancelled = AtomicBool::new(false);
        let ctx = SopContext::new(1 << 30, &cancelled);
        let prepared = prepare(&build(&mesh)?, &ctx)?;
        let facts = input_facts(&prepared);
        for &operator in OPERATORS {
            let first = run_operator(operator, &prepared, facts, &cancelled);
            if first.status.is_failure() {
                failures.push(format!(
                    "{name} / {operator}: {:?} {}",
                    first.status, first.detail
                ));
            }
            let second = run_operator(operator, &prepared, facts, &cancelled);
            if second.hash != first.hash {
                failures.push(format!("{name} / {operator}: nondeterministic"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn a_clean_solid_passes_every_operator_outright() -> Result<(), GeoError> {
    let cancelled = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 30, &cancelled);
    let prepared = prepare(&build(&cube())?, &ctx)?;
    let mut facts = input_facts(&prepared);
    facts.simple_solid = true;
    assert!(facts.closed && facts.manifold);
    for &operator in OPERATORS {
        let found = run_operator(operator, &prepared, facts, &cancelled);
        assert_eq!(found.status, Status::Ok, "{operator}: {}", found.detail);
    }
    Ok(())
}

#[test]
fn inside_out_solids_keep_their_volume_through_the_voxel_operators() -> Result<(), GeoError> {
    let (points, triangles) = cube();
    let inverted: Vec<[u32; 3]> = triangles.iter().map(|t| [t[2], t[1], t[0]]).collect();
    let cancelled = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 30, &cancelled);
    let prepared = prepare(&build(&(points, inverted))?, &ctx)?;
    let mut facts = input_facts(&prepared);
    facts.simple_solid = true;
    for operator in ["vdb_roundtrip", "boolean", "clip", "voronoifracture"] {
        let found = run_operator(operator, &prepared, facts, &cancelled);
        assert_eq!(found.status, Status::Ok, "{operator}: {}", found.detail);
    }
    Ok(())
}

#[test]
fn cancellation_is_honoured_by_every_operator() -> Result<(), GeoError> {
    let cancelled = AtomicBool::new(false);
    let ctx = SopContext::new(1 << 30, &cancelled);
    let prepared = prepare(&build(&cube())?, &ctx)?;
    let facts = input_facts(&prepared);
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    for &operator in OPERATORS {
        let found = run_operator(operator, &prepared, facts, &cancelled);
        // Trivial operators may finish first, but none may report a defect.
        assert!(!found.status.is_failure(), "{operator}: {:?}", found.status);
    }
    Ok(())
}

#[test]
fn manifests_are_validated() {
    let header = "file_id,sha256,bytes,faces,license,defects";
    let good = format!("# comment\n{header}\n42,abc,10,12,CC,open+large\n");
    let parsed = pcc_torture::parse_manifest(&good);
    assert_eq!(parsed.as_ref().map(Vec::len), Ok(1));
    assert_eq!(parsed.map(|rows| rows[0].faces), Ok(12));
    assert!(pcc_torture::parse_manifest("").is_err());
    assert!(pcc_torture::parse_manifest("wrong,header\n").is_err());
    for row in ["../../etc,a,1,1,CC,x", "42,a,1,many,CC,x", "42,too,few"] {
        let text = format!("{header}\n{row}\n");
        assert!(pcc_torture::parse_manifest(&text).is_err(), "{row}");
    }
    assert!(pcc_torture::is_simple_solid("clean+extreme_aspect"));
    assert!(!pcc_torture::is_simple_solid("clean+self_intersecting"));
}
