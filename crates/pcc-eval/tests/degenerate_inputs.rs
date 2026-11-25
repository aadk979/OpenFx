//! Every operator, fed inputs that real scenes produce by accident: nothing at all, loose points,
//! zero-area and repeated-corner faces, non-manifold fans, curves beside polygons, and coordinates
//! at the edge of `f32`. An operator may refuse any of them, but it must not panic, and what it
//! returns must be finite.
//!
//! This sweep found integer overflows in voxelization and in both point grids on its first run.

use pcc_eval::{
    CookBudget, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets, cook, register_operator,
};
use pcc_geo::{GeoDetail, PrimKind, PrimRun};
use pcc_graph::{GraphSnapshot, Node, NodeDef};
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

type Inputs<'a> = &'a [Arc<GeoDetail>];
type Fixture = fn(&Node, Inputs<'_>, &EvalContext<'_>) -> Result<GeoDetail, EvalError>;

const fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

fn mesh(
    points: Vec<Vec3>,
    runs: &[(PrimKind, u32, usize)],
    vertices: Vec<u32>,
) -> Result<GeoDetail, EvalError> {
    let runs = runs
        .iter()
        .map(|&(kind, arity, prim_count)| PrimRun {
            kind,
            arity,
            prim_count,
        })
        .collect();
    GeoDetail::points(points)
        .with_primitives(runs, vertices)
        .map_err(EvalError::Resource)
}

/// A closed tetrahedron of triangles over four points.
fn shell(points: Vec<Vec3>) -> Result<GeoDetail, EvalError> {
    mesh(
        points,
        &[(PrimKind::Polygon, 3, 4)],
        vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
    )
}

fn triangle(points: Vec<Vec3>) -> Result<GeoDetail, EvalError> {
    mesh(points, &[(PrimKind::Polygon, 3, 1)], vec![0, 1, 2])
}

fn empty(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    Ok(GeoDetail::points(Vec::new()))
}

fn one_point(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    Ok(GeoDetail::points(vec![v(1.0, 2.0, 3.0)]))
}

fn loose_points(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    Ok(GeoDetail::points(vec![
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(0.0, 1.0, 0.0),
    ]))
}

fn coincident_points(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    Ok(GeoDetail::points(vec![v(1.0, 1.0, 1.0); 5]))
}

fn far_points(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    Ok(GeoDetail::points(vec![v(-3e38, 3e38, 1e30); 2]))
}

fn segment(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    mesh(
        vec![v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0)],
        &[(PrimKind::Polyline, 2, 1)],
        vec![0, 1],
    )
}

fn zero_length_curve(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    mesh(
        vec![v(0.0, 0.0, 0.0); 3],
        &[(PrimKind::Polyline, 3, 1)],
        vec![0, 1, 2],
    )
}

fn one_triangle(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    triangle(vec![v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)])
}

fn point_triangle(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    triangle(vec![v(1.0, 1.0, 1.0); 3])
}

fn collinear_triangle(
    _: &Node,
    _: Inputs<'_>,
    _: &EvalContext<'_>,
) -> Result<GeoDetail, EvalError> {
    triangle(vec![v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(2.0, 0.0, 0.0)])
}

fn far_point_triangle(
    _: &Node,
    _: Inputs<'_>,
    _: &EvalContext<'_>,
) -> Result<GeoDetail, EvalError> {
    triangle(vec![v(1e10, -1e10, 1e10); 3])
}

fn repeated_corner(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    mesh(
        vec![v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)],
        &[(PrimKind::Polygon, 4, 1)],
        vec![0, 1, 1, 2],
    )
}

/// A triangle, a quad, a curve, and a point that only the curve reaches.
fn mixed(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    mesh(
        vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(2.0, 0.0, 0.0),
            v(5.0, 5.0, 5.0),
        ],
        &[
            (PrimKind::Polygon, 3, 1),
            (PrimKind::Polygon, 4, 1),
            (PrimKind::Polyline, 2, 1),
        ],
        vec![0, 1, 2, 1, 4, 3, 2, 4, 5],
    )
}

/// Three triangles on one edge.
fn fan(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    mesh(
        vec![
            v(0.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(-1.0, 0.0, 0.0),
        ],
        &[(PrimKind::Polygon, 3, 3)],
        vec![0, 1, 2, 0, 1, 3, 0, 1, 4],
    )
}

/// Two triangles that meet at one point.
fn bow_tie(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    mesh(
        vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(-1.0, 0.0, 0.0),
            v(-1.0, -1.0, 0.0),
        ],
        &[(PrimKind::Polygon, 3, 2)],
        vec![0, 1, 2, 0, 3, 4],
    )
}

fn unit_shell(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    shell(vec![
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(0.0, 1.0, 0.0),
        v(0.0, 0.0, 1.0),
    ])
}

fn huge_shell(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    shell(vec![
        v(0.0, 0.0, 0.0),
        v(3e38, 0.0, 0.0),
        v(0.0, 3e38, 0.0),
        v(0.0, 0.0, 3e38),
    ])
}

fn tiny_shell(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    shell(vec![
        v(0.0, 0.0, 0.0),
        v(1e-38, 0.0, 0.0),
        v(0.0, 1e-38, 0.0),
        v(0.0, 0.0, 1e-38),
    ])
}

fn far_shell(_: &Node, _: Inputs<'_>, _: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    shell(vec![
        v(1e10, 0.0, 0.0),
        v(1.0001e10, 0.0, 0.0),
        v(1e10, 1e6, 0.0),
        v(1e10, 0.0, 1e6),
    ])
}

const FIXTURES: &[(&str, Fixture)] = &[
    ("degenerate_empty", empty),
    ("degenerate_one_point", one_point),
    ("degenerate_loose_points", loose_points),
    ("degenerate_coincident_points", coincident_points),
    ("degenerate_far_points", far_points),
    ("degenerate_segment", segment),
    ("degenerate_zero_length_curve", zero_length_curve),
    ("degenerate_one_triangle", one_triangle),
    ("degenerate_point_triangle", point_triangle),
    ("degenerate_collinear_triangle", collinear_triangle),
    ("degenerate_far_point_triangle", far_point_triangle),
    ("degenerate_repeated_corner", repeated_corner),
    ("degenerate_mixed", mixed),
    ("degenerate_fan", fan),
    ("degenerate_bow_tie", bow_tie),
    ("degenerate_unit_shell", unit_shell),
    ("degenerate_huge_shell", huge_shell),
    ("degenerate_tiny_shell", tiny_shell),
    ("degenerate_far_shell", far_shell),
];

/// Zones need a body, which this sweep does not build.
const ZONE_OPERATORS: [&str; 4] = ["state_prev", "solver", "foreach", "foreach_piece"];

#[test]
fn no_operator_panics_or_returns_non_finite_points() -> Result<(), String> {
    for (name, fixture) in FIXTURES {
        register_operator(
            NodeDef::new(name, "test", "Degenerate input.", &[], &[]),
            *fixture,
        )
        .map_err(|error| format!("{error:?}"))?;
    }
    let golden = GraphSnapshot::from_ofxs(include_str!(
        "../../pcc-graph/tests/golden_all_operators.ofxs"
    ))
    .map_err(|error| format!("{error:?}"))?;
    let clean = Node::new("box", NodeId(2)).with_vec3("size", v(1.0, 1.0, 1.0));
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        cancelled: &cancelled,
        assets: &NoAssets,
        budget: CookBudget {
            max_memory_bytes: 1 << 28,
            max_elements: 2_000_000,
            max_state_ticks: DEFAULT_MAX_STATE_TICKS,
        },
    };

    let (mut cooked, mut refused) = (0usize, 0usize);
    let operators = golden
        .nodes
        .iter()
        .filter(|node| !node.inputs.is_empty() && !ZONE_OPERATORS.contains(&node.op.as_str()));
    for node in operators {
        // The degenerate input on every port, then on each port beside clean geometry.
        let wirings: Vec<Vec<NodeId>> = if node.inputs.len() == 1 {
            vec![vec![NodeId(1)]]
        } else {
            vec![
                vec![NodeId(1), NodeId(1)],
                vec![NodeId(1), NodeId(2)],
                vec![NodeId(2), NodeId(1)],
            ]
        };
        for (name, _) in FIXTURES {
            for wiring in &wirings {
                let mut target = node.clone();
                target.id = NodeId(3);
                target.inputs.clone_from(wiring);
                let scene = GraphSnapshot {
                    revision: RevisionId(1),
                    nodes: vec![Node::new(name, NodeId(1)), clean.clone(), target],
                };
                match cook(&scene, &ctx) {
                    Ok(geo) => {
                        cooked += 1;
                        let finite = (0..geo.len()).all(|index| {
                            geo.point(index).is_some_and(|p| {
                                p.x.is_finite() && p.y.is_finite() && p.z.is_finite()
                            })
                        });
                        assert!(
                            finite,
                            "`{}` on `{name}` returned a non-finite point",
                            node.op
                        );
                        // The probes an agent would run next must hold up as well.
                        let topology = geo.topology_index();
                        assert!(topology.boundary_edge_count() <= geo.vertex_count());
                        assert!(geo.surface_area().is_finite());
                        assert!(geo.enclosed_volume().is_finite());
                    }
                    Err(EvalError::InvalidGraph) => {
                        return Err(format!("`{}` on `{name}` was not a valid scene", node.op));
                    }
                    Err(_) => refused += 1,
                }
            }
        }
    }
    // Both outcomes must actually occur, or the sweep is not exercising the operators.
    assert!(cooked > 500, "only {cooked} cases cooked");
    assert!(refused > 100, "only {refused} cases were refused");
    Ok(())
}
