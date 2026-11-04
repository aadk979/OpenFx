//! Torture harness: runs geometry operators on hostile real-world meshes and checks invariants.
//!
//! Each `(mesh, operator)` pair runs in its own child process, so a panic, stack overflow, or abort
//! is observed and classified instead of ending the run, and a real hang can be killed.

use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, GeoError, TopologyIndex};
use pcc_math::Vec3;
use pcc_sop::{
    RemeshParams, SopContext, SurfaceScatter, Trs, clip_halfspace, fuse_points, has_repeated_faces,
    poly_bevel, poly_reduce, remesh, scatter_surface, signed_volume, transform_trs, triangulate,
    uv_flatten, voronoi_fracture,
};
use pcc_vdb::{VdbCombineOp, VdbGrid, boolean_meshes_cancellable};
use std::sync::atomic::AtomicBool;

/// Every mesh is rescaled to this bounding-box diagonal, so absolute parameters mean the same thing
/// for models authored in microns and in metres.
pub const NORMALIZED_DIAGONAL: f64 = 10.0;
/// Voxel size for volumetric operators: 48 voxels across the normalized diagonal.
pub const VOXEL_SIZE: f32 = (NORMALIZED_DIAGONAL / 48.0) as f32;
/// Memory budget handed to each operator.
pub const MEMORY_LIMIT: usize = 2 << 30;
/// Target triangle count when choosing the remesh edge length.
const REMESH_TARGET_TRIANGLES: f64 = 20_000.0;

/// All operators the harness can run.
pub const OPERATORS: &[&str] = &[
    "boolean",
    "vdb_roundtrip",
    "remesh",
    "polyreduce",
    "polyreduce_half",
    "uvflatten",
    "polybevel",
    "clip",
    "voronoifracture",
    "scatter_surface",
    "triangulate",
    "subdivide",
    "extrude",
    "normal",
];

/// How one operator run ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Status {
    /// Finished with all invariants holding.
    Ok,
    /// Finished correctly with nothing to output, such as a voxel union of an open sheet.
    OkEmpty,
    /// Returned a structured error because the input does not meet the operator's contract.
    Rejected,
    /// Returned a structured error because it would exceed the memory budget.
    OverBudget,
    /// Observed cancellation at the soft time limit.
    TimedOut,
    /// Finished but broke an invariant.
    Invariant,
    /// Produced different output on a repeat run.
    Nondeterministic,
    /// The process panicked.
    Panic,
    /// The process died any other way, such as a stack overflow or an abort.
    Crash,
    /// Ignored cancellation past the grace period, so it was killed.
    Hang,
}

impl Status {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::OkEmpty => "ok_empty",
            Self::Rejected => "rejected",
            Self::OverBudget => "over_budget",
            Self::TimedOut => "timed_out",
            Self::Invariant => "invariant",
            Self::Nondeterministic => "nondeterministic",
            Self::Panic => "panic",
            Self::Crash => "crash",
            Self::Hang => "hang",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == text)
    }

    pub const ALL: [Self; 10] = [
        Self::Ok,
        Self::OkEmpty,
        Self::Rejected,
        Self::OverBudget,
        Self::TimedOut,
        Self::Invariant,
        Self::Nondeterministic,
        Self::Panic,
        Self::Crash,
        Self::Hang,
    ];

    /// Whether this outcome is a defect rather than an acceptable limit or rejection.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(
            self,
            Self::Invariant | Self::Nondeterministic | Self::Panic | Self::Crash | Self::Hang
        )
    }
}

/// Result of running one operator on one prepared mesh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub detail: String,
    pub out_points: usize,
    pub out_prims: usize,
    pub hash: String,
}

/// Facts about the prepared input, reported alongside every result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputFacts {
    pub points: usize,
    pub prims: usize,
    pub closed: bool,
    pub manifold: bool,
    /// The input is a single closed shell with no self-intersection, so its signed volume is
    /// meaningful and volume-based oracles apply. Set by the caller from the corpus metadata.
    pub simple_solid: bool,
}

#[must_use]
pub fn input_facts(geo: &GeoDetail) -> InputFacts {
    let topology = TopologyIndex::build(geo);
    InputFacts {
        points: geo.len(),
        prims: geo.prim_count(),
        // Back-to-back faces are closed only in name: they bound no solid to keep closed.
        closed: topology.is_closed_manifold() && !has_repeated_faces(geo),
        manifold: topology.is_manifold(),
        simple_solid: false,
    }
}

/// Whether manifest defect labels describe a single closed shell with no self-intersection.
#[must_use]
pub fn is_simple_solid(defects: &str) -> bool {
    !defects.split('+').any(|label| {
        matches!(
            label,
            "self_intersecting"
                | "coplanar_overlap"
                | "many_components"
                | "open"
                | "edge_nonmanifold"
                | "vertex_nonmanifold"
                | "nonoriented"
                | "non_pwn"
                | "duplicated_faces"
        )
    })
}

/// Fails when a simple solid's volume drifted by more than `tolerance` of the input volume.
fn volume_kept(
    out: &GeoDetail,
    input: &GeoDetail,
    facts: InputFacts,
    tolerance: f64,
) -> Option<String> {
    let before = signed_volume(input).abs();
    let after = signed_volume(out).abs();
    (facts.simple_solid && (after - before).abs() > tolerance * before)
        .then(|| format!("volume drifted from {before} to {after}"))
}

/// Centres a mesh, rescales it to [`NORMALIZED_DIAGONAL`], and welds near-coincident points.
pub fn prepare(geo: &GeoDetail, ctx: &SopContext<'_>) -> Result<GeoDetail, GeoError> {
    let (low, high) = geo.bounds().ok_or(GeoError::InvalidTopology)?;
    let extent = [
        f64::from(high.x) - f64::from(low.x),
        f64::from(high.y) - f64::from(low.y),
        f64::from(high.z) - f64::from(low.z),
    ];
    let diagonal = (extent[0] * extent[0] + extent[1] * extent[1] + extent[2] * extent[2]).sqrt();
    if !diagonal.is_finite() || diagonal <= 0.0 {
        return Err(GeoError::InvalidTopology);
    }
    let factor = NORMALIZED_DIAGONAL / diagonal;
    let centre = |a: f32, b: f32| -(f64::from(a) + f64::from(b)) * 0.5 * factor;
    let scale = factor as f32;
    let normalized = transform_trs(
        geo,
        &Trs {
            translate: Vec3::checked(
                centre(low.x, high.x) as f32,
                centre(low.y, high.y) as f32,
                centre(low.z, high.z) as f32,
            )?,
            rotate_degrees: Vec3::ZERO,
            scale: Vec3::checked(scale, scale, scale)?,
        },
        ctx,
    )?;
    fuse_points(&normalized, NORMALIZED_DIAGONAL * 1e-6, ctx)
}

fn outcome(status: Status, detail: impl Into<String>, geo: Option<&GeoDetail>) -> Outcome {
    Outcome {
        status,
        detail: detail.into(),
        out_points: geo.map_or(0, GeoDetail::len),
        out_prims: geo.map_or(0, GeoDetail::prim_count),
        hash: geo.map_or_else(String::new, |g| g.hash().to_hex().to_string()),
    }
}

fn finished(geo: &GeoDetail, violation: Option<String>) -> Outcome {
    match violation {
        Some(message) => outcome(Status::Invariant, message, Some(geo)),
        None => outcome(Status::Ok, "", Some(geo)),
    }
}

fn from_error(error: GeoError) -> Outcome {
    let status = match error {
        GeoError::Cancelled => Status::TimedOut,
        GeoError::LimitExceeded => Status::OverBudget,
        // Reaching this with finite input means an operator built a value it could not represent.
        GeoError::NonFinite => Status::Invariant,
        _ => Status::Rejected,
    };
    outcome(status, format!("{error:?}"), None)
}

/// Fails when any position is outside the input bounds, expanded by `margin`.
fn outside_bounds(out: &GeoDetail, input: &GeoDetail, margin: f32) -> Option<String> {
    let (low, high) = input.bounds()?;
    let (out_low, out_high) = out.bounds()?;
    let escaped = out_low.x < low.x - margin
        || out_low.y < low.y - margin
        || out_low.z < low.z - margin
        || out_high.x > high.x + margin
        || out_high.y > high.y + margin
        || out_high.z > high.z + margin;
    escaped.then(|| format!("escaped input bounds: {out_low:?}..{out_high:?} vs {low:?}..{high:?}"))
}

/// Fails when a closed-manifold input did not produce a closed-manifold output.
fn closure_kept(out: &GeoDetail, input: InputFacts) -> Option<String> {
    let topology = TopologyIndex::build(out);
    (input.closed && !topology.is_closed_manifold()).then(|| {
        format!(
            "closed input became open: {} boundary, {} non-manifold edges",
            topology.boundary_edge_count(),
            topology.non_manifold_edge_count()
        )
    })
}

/// The plan's acceptance test for voxel surfaces: output is empty, or a closed oriented manifold.
fn watertight(out: &GeoDetail) -> Outcome {
    if out.prim_count() == 0 {
        return outcome(Status::OkEmpty, "", Some(out));
    }
    let topology = TopologyIndex::build(out);
    let pinched = topology.non_manifold_vertex_count(out);
    if topology.is_closed_manifold() && pinched == 0 {
        outcome(Status::Ok, "", Some(out))
    } else {
        outcome(
            Status::Invariant,
            format!(
                "not watertight: {} boundary, {} non-manifold edges, {pinched} pinched vertices",
                topology.boundary_edge_count(),
                topology.non_manifold_edge_count()
            ),
            Some(out),
        )
    }
}

/// Bound on the error in a UV triangle's doubled area, per unit of its longest edge, caused by
/// rounding coordinates in `[0, 1]` to `f32`: half a unit in the last place (6e-8), on four terms.
const UV_ROUNDING_PER_EDGE: f64 = 2.4e-7;

fn check_uvs(out: &GeoDetail) -> Option<String> {
    let column = out
        .domain_attribute(AttrDomain::Vertex, "uv")
        .and_then(AttributeColumn::as_vec3)?;
    let positions: Vec<Vec3> = (0..out.len()).filter_map(|i| out.point(i)).collect();
    let uv = |i: usize| {
        column
            .get(i)
            .map_or([0.0f64; 2], |v| [f64::from(v.x), f64::from(v.y)])
    };
    let mut cursor = 0usize;
    let mut flips = 0usize;
    for run in out.prim_runs() {
        let arity = run.arity as usize;
        for _ in 0..run.prim_count {
            let vertex = |k: usize| out.vertex_point(cursor + k).unwrap_or(0) as usize;
            for k in 1..arity.saturating_sub(1) {
                let (a, b, c) = (uv(cursor), uv(cursor + k), uv(cursor + k + 1));
                if !(0.0..=1.0).contains(&a[0]) || !(0.0..=1.0).contains(&a[1]) {
                    return Some(format!("uv outside the unit square: {a:?}"));
                }
                let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                let longest = (b[0] - a[0])
                    .hypot(b[1] - a[1])
                    .max((c[0] - a[0]).hypot(c[1] - a[1]))
                    .max((c[0] - b[0]).hypot(c[1] - b[1]));
                // Faces of zero mesh area have no meaningful orientation.
                let (p, q, r) = (
                    positions[vertex(0)],
                    positions[vertex(k)],
                    positions[vertex(k + 1)],
                );
                let surface = q
                    .sub_checked(p)
                    .and_then(|u| r.sub_checked(p).and_then(|v| u.cross_checked(v)))
                    .map_or(0.0, Vec3::length);
                // Coordinates are stored as f32, so slivers may round to a slightly negative area.
                if area < -UV_ROUNDING_PER_EDGE * longest && surface > 1e-9 {
                    flips += 1;
                }
            }
            cursor += arity;
        }
    }
    (flips > 0).then(|| format!("{flips} inverted uv triangles"))
}

/// Runs `operator` once on a prepared mesh, without any repeat-run check.
#[must_use]
pub fn run_operator(
    operator: &str,
    input: &GeoDetail,
    facts: InputFacts,
    cancelled: &AtomicBool,
) -> Outcome {
    let ctx = SopContext::new(MEMORY_LIMIT, cancelled);
    let result = dispatch(operator, input, facts, &ctx);
    match result {
        Ok(found) => found,
        Err(error) => from_error(error),
    }
}

fn dispatch(
    operator: &str,
    input: &GeoDetail,
    facts: InputFacts,
    ctx: &SopContext<'_>,
) -> Result<Outcome, GeoError> {
    let cancelled = ctx.cancelled;
    Ok(match operator {
        "boolean" => {
            let shifted = input.translated_cancellable(
                Vec3::checked(1.3, 0.7, 0.4)?,
                MEMORY_LIMIT,
                cancelled,
            )?;
            let out = boolean_meshes_cancellable(
                input,
                &shifted,
                VdbCombineOp::Union,
                VOXEL_SIZE,
                MEMORY_LIMIT,
                cancelled,
            )?;
            watertight(&out)
        }
        "vdb_roundtrip" => {
            let grid =
                VdbGrid::from_polygons_cancellable(input, VOXEL_SIZE, 3, MEMORY_LIMIT, cancelled)?;
            let out = grid.to_polygons_cancellable(MEMORY_LIMIT, cancelled)?;
            let area = f64::from(input.surface_area());
            let volume = signed_volume(input).abs();
            // Solids at least two voxels thick must survive within one quarter-voxel shell of
            // their surface. Thinner walls can legitimately fall between voxel centres.
            let thick = area > 0.0 && 2.0 * volume / area >= 2.0 * f64::from(VOXEL_SIZE);
            let gap = (signed_volume(&out).abs() - volume).abs();
            if facts.simple_solid && thick && gap > 0.25 * area * f64::from(VOXEL_SIZE) {
                outcome(
                    Status::Invariant,
                    format!("voxel volume off by {gap} of {volume}"),
                    Some(&out),
                )
            } else {
                watertight(&out)
            }
        }
        "remesh" => {
            let area = f64::from(input.surface_area()).max(1e-6);
            let target = (area / (REMESH_TARGET_TRIANGLES * 3f64.sqrt() / 4.0)).sqrt();
            let out = remesh(
                input,
                &RemeshParams {
                    target_length: target,
                    iterations: 3,
                    feature_angle_degrees: 45.0,
                },
                ctx,
            )?;
            let violation = closure_kept(&out, facts)
                .or_else(|| outside_bounds(&out, input, 1e-3))
                .or_else(|| volume_kept(&out, input, facts, 0.1));
            finished(&out, violation)
        }
        // An aggressive ratio: topology and bounds must hold, but a high-genus model may legitimately
        // spend its volume to reach the target (98006 loses 29% here).
        "polyreduce" => {
            let triangles = triangulate(input, ctx)?;
            let out = poly_reduce(input, 0.25, ctx)?;
            let violation = closure_kept(&out, facts)
                .or_else(|| {
                    (out.prim_count() > triangles.prim_count()).then(|| {
                        format!(
                            "triangle count grew: {} > {}",
                            out.prim_count(),
                            triangles.prim_count()
                        )
                    })
                })
                .or_else(|| outside_bounds(&out, input, (NORMALIZED_DIAGONAL * 0.1) as f32));
            finished(&out, violation)
        }
        // A moderate ratio on a model with detail to spare must keep the shape.
        "polyreduce_half" => {
            let triangles = triangulate(input, ctx)?;
            let out = poly_reduce(input, 0.5, ctx)?;
            let violation = closure_kept(&out, facts).or_else(|| {
                // Thin walls make relative volume far too sensitive: what must hold is where the
                // surface is, so a mean displacement of 1e-4 of the diagonal is also accepted.
                let before = signed_volume(input).abs();
                let allowed = f64::from(input.surface_area()) * NORMALIZED_DIAGONAL * 1e-4;
                let shift = (signed_volume(&out).abs() - before).abs();
                if triangles.prim_count() >= 2000 && shift > allowed {
                    volume_kept(&out, input, facts, 0.01)
                } else {
                    None
                }
            });
            finished(&out, violation)
        }
        "uvflatten" => {
            let out = uv_flatten(input, 60.0, ctx)?;
            let violation = (out.prim_count() != input.prim_count())
                .then(|| "primitive count changed".to_owned())
                .or_else(|| check_uvs(&out));
            finished(&out, violation)
        }
        "polybevel" => {
            let out = poly_bevel(input, 0.01, 30.0, ctx)?;
            finished(&out, closure_kept(&out, facts))
        }
        "clip" => {
            let out = clip_halfspace(input, [0.0; 3], [0.0, 1.0, 0.0], true, ctx)?;
            if out.prim_count() == 0 {
                return Ok(outcome(Status::OkEmpty, "", Some(&out)));
            }
            let violation = closure_kept(&out, facts).or_else(|| outside_bounds(&out, input, 1e-3));
            finished(&out, violation)
        }
        "voronoifracture" => {
            let seeds = scatter_surface(
                input,
                &SurfaceScatter {
                    count: 6,
                    seed: 1,
                    min_distance: 0.0,
                },
                ctx,
            )?;
            let out = voronoi_fracture(input, &seeds, ctx)?;
            let whole = signed_volume(input);
            let gap = (signed_volume(&out) - whole).abs();
            let violation = closure_kept(&out, facts).or_else(|| {
                (gap > 1e-3 * whole.abs().max(1.0))
                    .then(|| format!("volume not conserved: off by {gap} of {whole}"))
            });
            finished(&out, violation)
        }
        "scatter_surface" => {
            let out = scatter_surface(
                input,
                &SurfaceScatter {
                    count: 1000,
                    seed: 1,
                    min_distance: 0.0,
                },
                ctx,
            )?;
            finished(&out, outside_bounds(&out, input, 1e-3))
        }
        "triangulate" => {
            let out = triangulate(input, ctx)?;
            let (before, after) = (input.surface_area(), out.surface_area());
            let violation = (((before - after) / before.max(1e-6)).abs() > 1e-2)
                .then(|| format!("area changed: {before} to {after}"));
            finished(&out, violation)
        }
        "subdivide" => {
            let out = input.subdivide_cancellable(1, MEMORY_LIMIT, cancelled)?;
            finished(&out, closure_kept(&out, facts))
        }
        "extrude" => {
            let out = input.poly_extrude_cancellable(0.01, 0.0, MEMORY_LIMIT, cancelled)?;
            finished(&out, None)
        }
        "normal" => {
            let out = input.compute_normals_cancellable(MEMORY_LIMIT, cancelled)?;
            finished(&out, None)
        }
        other => outcome(Status::Rejected, format!("unknown operator {other}"), None),
    })
}

/// One manifest row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub file_id: String,
    pub faces: usize,
    pub defects: String,
}

/// Parses a manifest, skipping `#` comments and the header.
pub fn parse_manifest(text: &str) -> Result<Vec<ManifestEntry>, String> {
    let mut rows = text
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty());
    let header = rows.next().ok_or("manifest is empty")?;
    if header != "file_id,sha256,bytes,faces,license,defects" {
        return Err(format!("unexpected manifest header: {header}"));
    }
    rows.map(|line| {
        let fields: Vec<&str> = line.split(',').collect();
        let [file_id, _, _, faces, _, defects] = fields[..] else {
            return Err(format!("malformed manifest row: {line}"));
        };
        if file_id.is_empty() || !file_id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("malformed file id: {line}"));
        }
        Ok(ManifestEntry {
            file_id: file_id.to_owned(),
            faces: faces
                .parse()
                .map_err(|_| format!("malformed face count: {line}"))?,
            defects: defects.to_owned(),
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_geo::{PrimKind, PrimRun};

    fn triangle_with_uv(uv: [[f32; 2]; 3]) -> Result<GeoDetail, GeoError> {
        let column = uv
            .iter()
            .map(|c| Vec3::checked(c[0], c[1], 0.0))
            .collect::<Result<Vec<_>, _>>()?;
        GeoDetail::points(vec![
            Vec3::checked(0.0, 0.0, 0.0)?,
            Vec3::checked(1.0, 0.0, 0.0)?,
            Vec3::checked(0.0, 1.0, 0.0)?,
        ])
        .with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: 1,
            }],
            vec![0, 1, 2],
        )?
        .with_domain_attribute(
            AttrDomain::Vertex,
            "uv",
            AttributeColumn::vec3_values(column),
        )
    }

    #[test]
    fn the_uv_oracle_flags_real_inversions_but_not_storage_rounding() -> Result<(), GeoError> {
        let good = triangle_with_uv([[0.1, 0.1], [0.2, 0.1], [0.1, 0.2]])?;
        assert_eq!(check_uvs(&good), None);
        let inverted = triangle_with_uv([[0.1, 0.1], [0.1, 0.2], [0.2, 0.1]])?;
        assert!(check_uvs(&inverted).is_some());
        // A sliver one float step past collinear is inverted only by rounding.
        let step = f32::EPSILON;
        let sliver = triangle_with_uv([[0.5, 0.5], [0.9, 0.5], [0.7, 0.5 - step]])?;
        assert_eq!(check_uvs(&sliver), None);
        let outside = triangle_with_uv([[1.5, 0.1], [1.6, 0.1], [1.5, 0.2]])?;
        assert!(check_uvs(&outside).is_some());
        Ok(())
    }

    fn polygons(points: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<GeoDetail, GeoError> {
        let positions = points
            .iter()
            .map(|p| Vec3::checked(p[0], p[1], p[2]))
            .collect::<Result<Vec<_>, _>>()?;
        GeoDetail::points(positions).with_primitives(
            vec![PrimRun {
                kind: PrimKind::Polygon,
                arity: 3,
                prim_count: triangles.len(),
            }],
            triangles.iter().flatten().copied().collect(),
        )
    }

    const TETRAHEDRON: [[u32; 3]; 4] = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
    const CORNERS: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

    #[test]
    fn the_topology_oracles_report_open_pinched_and_empty_output() -> Result<(), GeoError> {
        let closed = polygons(&CORNERS, &TETRAHEDRON)?;
        let open = polygons(&CORNERS, &TETRAHEDRON[..3])?;
        assert_eq!(watertight(&closed).status, Status::Ok);
        assert_eq!(watertight(&open).status, Status::Invariant);
        assert_eq!(
            watertight(&GeoDetail::points(Vec::new())).status,
            Status::OkEmpty
        );
        // Two tetrahedra meeting only at the origin: every edge is fine, the vertex is not.
        let mut points = CORNERS.to_vec();
        points.extend_from_slice(&[[-1., 0., 0.], [0., -1., 0.], [0., 0., -1.]]);
        let pinched = polygons(
            &points,
            &[
                [0, 2, 1],
                [0, 1, 3],
                [1, 2, 3],
                [0, 3, 2],
                [0, 5, 4],
                [0, 4, 6],
                [4, 5, 6],
                [0, 6, 5],
            ],
        )?;
        let found = watertight(&pinched);
        assert_eq!(found.status, Status::Invariant);
        assert!(found.detail.contains("1 pinched"), "{}", found.detail);

        // A closed input must stay closed, and an open input has no such promise.
        let closed_facts = input_facts(&closed);
        let open_facts = input_facts(&open);
        assert!(closed_facts.closed && !open_facts.closed);
        assert!(closure_kept(&open, closed_facts).is_some());
        assert_eq!(closure_kept(&closed, closed_facts), None);
        assert_eq!(closure_kept(&open, open_facts), None);
        Ok(())
    }

    #[test]
    fn only_defects_count_as_failures_and_statuses_round_trip() {
        for status in Status::ALL {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        let failures: Vec<_> = Status::ALL.iter().filter(|s| s.is_failure()).collect();
        assert_eq!(failures.len(), 5);
        assert!(!Status::Rejected.is_failure() && !Status::TimedOut.is_failure());
        assert_eq!(Status::parse("nonsense"), None);
    }
}
