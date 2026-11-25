//! `mesh_repair` as a node: reached by a real graph, and refusing what it cannot carry.

use pcc_eval::{
    CookBudget, CookCache, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets,
    cook_node_cached,
};
use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail, TopologyIndex};
use pcc_graph::{GraphSnapshot, Node};
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::sync::{Arc, atomic::AtomicBool};

const GENEROUS: CookBudget = CookBudget {
    max_memory_bytes: 1 << 28,
    max_elements: 1_000_000,
    max_state_ticks: DEFAULT_MAX_STATE_TICKS,
};

fn run(nodes: Vec<Node>, id: u64) -> Result<Arc<GeoDetail>, EvalError> {
    let graph = GraphSnapshot {
        revision: RevisionId(1),
        nodes,
    };
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    cook_node_cached(&CookCache::default(), &graph, NodeId(id), 0, 1, &ctx)
}

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

fn sphere() -> Node {
    Node::new("sphere", NodeId(1))
        .with_f32("radius", 1.0)
        .with_u32("rows", 12)
        .with_u32("cols", 16)
}

fn repair(id: u64, input: u64, max_hole_edges: u32) -> Node {
    Node::new("mesh_repair", NodeId(id))
        .with_input(NodeId(input))
        .with_u32("max_hole_edges", max_hole_edges)
}

#[test]
fn a_mesh_merged_onto_itself_repairs_back_to_one_surface() -> Result<(), EvalError> {
    let doubled = vec![
        sphere(),
        Node::new("merge", NodeId(2))
            .with_input(NodeId(1))
            .with_input(NodeId(1)),
        Node::new("fuse", NodeId(3))
            .with_input(NodeId(2))
            .with_f32("distance", 1e-4),
        repair(4, 3, 0),
    ];
    let broken = run(doubled.clone(), 3)?;
    assert!(!TopologyIndex::build(&broken).is_manifold());

    let out = run(doubled, 4)?;
    let single = run(vec![sphere(), repair(2, 1, 0)], 2)?;
    assert_eq!(out.prim_count(), single.prim_count());
    assert!(TopologyIndex::build(&out).is_closed_manifold());
    let Some(AttributeColumn::I32(removed)) =
        out.domain_attribute(AttrDomain::Detail, "repair_duplicate_faces")
    else {
        return Err(EvalError::InvalidGraph);
    };
    assert_eq!(removed.get(0), Some(single.prim_count() as i32));
    Ok(())
}

#[test]
fn the_hole_limit_reaches_the_operator() -> Result<(), EvalError> {
    // Blasting the top of a box leaves one four-sided hole.
    let open = |limit| {
        vec![
            Node::new("box", NodeId(1)).with_vec3("size", v3(1.0, 1.0, 1.0)),
            Node::new("group_bounds", NodeId(2))
                .with_input(NodeId(1))
                .with_text("group_name", "top")
                .with_text("domain", "prim")
                .with_vec3("min", v3(-1.0, 0.4, -1.0))
                .with_vec3("max", v3(1.0, 1.0, 1.0)),
            Node::new("blast", NodeId(3))
                .with_input(NodeId(2))
                .with_text("group_name", "top")
                .with_bool("negate", false),
            repair(4, 3, limit),
        ]
    };
    let left = run(open(0), 4)?;
    assert!(!TopologyIndex::build(&left).is_closed_manifold());
    let filled = run(open(4), 4)?;
    assert!(TopologyIndex::build(&filled).is_closed_manifold());
    Ok(())
}

#[test]
fn solids_are_refused_rather_than_lost() {
    let tets = vec![
        Node::new("tetbox", NodeId(1))
            .with_vec3("size", v3(1.0, 1.0, 1.0))
            .with_u32("divisions", 1),
        repair(2, 1, 0),
    ];
    let refused = run(tets, 2);
    assert!(refused.is_err());
    // The refusal names the way forward rather than only saying no.
    assert!(format!("{:?}", refused.err()).contains("tet_surface"));
}
