//! Tetrahedra through the evaluator: what keeps them, what refuses them, and what they become.

use pcc_eval::{
    CookBudget, CookCache, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets, cook_cached,
    cook_node_cached,
};
use pcc_geo::{GeoDetail, GeoError};
use pcc_graph::{GraphSnapshot, Node};
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::sync::{Arc, atomic::AtomicBool};

const GENEROUS: CookBudget = CookBudget {
    max_memory_bytes: 1 << 28,
    max_elements: 1_000_000,
    max_state_ticks: DEFAULT_MAX_STATE_TICKS,
};

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

fn scene(nodes: Vec<Node>) -> GraphSnapshot {
    GraphSnapshot {
        revision: RevisionId(1),
        nodes,
    }
}

fn run(graph: &GraphSnapshot, id: u64, budget: CookBudget) -> Result<Arc<GeoDetail>, EvalError> {
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget,
    };
    cook_node_cached(&CookCache::default(), graph, NodeId(id), 0, 1, &ctx)
}

fn xform(id: u64, input: u64, scale: Vec3, translate: Vec3) -> Node {
    Node::new("xform", NodeId(id))
        .with_input(NodeId(input))
        .with_vec3("translate", translate)
        .with_vec3("rotate_degrees", Vec3::ZERO)
        .with_vec3("scale", scale)
}

/// A 2 x 3 x 5 solid with volume 30, as node 1.
fn solid() -> Node {
    Node::new("tetbox", NodeId(1))
        .with_vec3("size", v3(2.0, 3.0, 5.0))
        .with_u32("divisions", 3)
}

/// A scene must end in an operator, so this gives a lone generator something to feed.
fn alone(generator: Node) -> GraphSnapshot {
    scene(vec![
        generator,
        Node::new("tet_surface", NodeId(2)).with_input(NodeId(1)),
    ])
}

fn volume(geo: &GeoDetail) -> Result<f64, EvalError> {
    geo.tet_volume().map_err(EvalError::Resource)
}

#[test]
fn a_solid_becomes_a_surface_that_ordinary_operators_can_use() -> Result<(), EvalError> {
    let graph = scene(vec![
        solid(),
        Node::new("tet_surface", NodeId(2)).with_input(NodeId(1)),
        Node::new("normal", NodeId(3)).with_input(NodeId(2)),
    ]);
    let tets = run(&graph, 1, GENEROUS)?;
    assert_eq!(tets.tet_count(), 6 * 27);
    assert!((volume(&tets)? - 30.0).abs() < 1e-3);
    let surface = run(&graph, 3, GENEROUS)?;
    assert_eq!(surface.tet_count(), 0);
    assert_eq!(surface.prim_count(), 12 * 9);
    assert!((f64::from(surface.enclosed_volume()) - 30.0).abs() < 1e-3);
    assert!(surface.attribute("N").is_some());
    Ok(())
}

#[test]
fn operators_that_only_know_surfaces_refuse_rather_than_drop_a_solid() {
    let refuses = |op: Node| {
        let plain = Node::new("box", NodeId(3)).with_vec3("size", v3(1.0, 1.0, 1.0));
        let found = run(&scene(vec![solid(), plain, op]), 4, GENEROUS);
        assert!(
            matches!(&found, Err(EvalError::Param { node, param, message })
                if *node == NodeId(4) && param == "input" && message.contains("tet_surface")),
            "{found:?}"
        );
    };
    let input = NodeId(1);
    refuses(
        Node::new("polyreduce", NodeId(4))
            .with_input(input)
            .with_f32("target_ratio", 0.5),
    );
    refuses(Node::new("triangulate", NodeId(4)).with_input(input));
    refuses(Node::new("reverse", NodeId(4)).with_input(input));
    refuses(Node::new("normal", NodeId(4)).with_input(input));
    refuses(Node::new("measure", NodeId(4)).with_input(input));
    refuses(Node::new("pack", NodeId(4)).with_input(input));
    refuses(
        Node::new("vdb_from_polygons", NodeId(4))
            .with_input(input)
            .with_f32("voxel_size", 0.5),
    );
    // On either input of a two-input operator.
    refuses(
        Node::new("boolean", NodeId(4))
            .with_input(NodeId(3))
            .with_input(input)
            .with_text("mode", "union")
            .with_f32("voxel_size", 0.5),
    );
}

#[test]
fn transforms_and_merges_keep_elements_and_their_orientation() -> Result<(), EvalError> {
    let graph = scene(vec![
        solid(),
        Node::new("transform", NodeId(2))
            .with_input(NodeId(1))
            .with_vec3("offset", v3(10.0, 0.0, 0.0)),
        // Doubling one axis doubles the volume.
        xform(3, 1, v3(2.0, 1.0, 1.0), Vec3::ZERO),
        // A mirror would invert every element, unless the corners swap with it.
        xform(4, 1, v3(-1.0, 1.0, 1.0), Vec3::ZERO),
        Node::new("merge", NodeId(5))
            .with_input(NodeId(2))
            .with_input(NodeId(3)),
        Node::new("tet_surface", NodeId(6)).with_input(NodeId(4)),
    ]);
    let moved = run(&graph, 2, GENEROUS)?;
    assert!((volume(&moved)? - 30.0).abs() < 1e-3);
    assert_eq!(moved.bounds().map(|(low, _)| low.x), Some(9.0));
    assert!((volume(&*run(&graph, 3, GENEROUS)?)? - 60.0).abs() < 1e-3);
    let mirrored = run(&graph, 4, GENEROUS)?;
    assert!((volume(&mirrored)? - 30.0).abs() < 1e-3);
    let merged = run(&graph, 5, GENEROUS)?;
    assert_eq!(merged.tet_count(), 2 * 6 * 27);
    assert!((volume(&merged)? - 90.0).abs() < 1e-3);
    // Surface extraction refuses inverted elements, so this confirms the mirror above.
    let surface = run(&graph, 6, GENEROUS)?;
    assert!((f64::from(surface.enclosed_volume()) - 30.0).abs() < 1e-3);
    Ok(())
}

#[test]
fn a_solver_can_carry_elements_from_tick_to_tick() -> Result<(), EvalError> {
    let graph = scene(vec![
        solid(),
        Node::new("state_prev", NodeId(2)).with_u64("zone", 4),
        xform(3, 2, v3(1.0, 1.0, 1.0), v3(0.0, 1.0, 0.0)),
        Node::new("solver", NodeId(4))
            .with_input(NodeId(1))
            .with_input(NodeId(3))
            .with_u32("ticks", 10),
    ]);
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    let moved = cook_cached(&CookCache::default(), &graph, 0, 1, &ctx)?;
    assert_eq!(moved.tet_count(), 6 * 27);
    assert!((volume(&moved)? - 30.0).abs() < 1e-3);
    assert_eq!(moved.bounds().map(|(_, high)| high.y), Some(11.5));
    Ok(())
}

#[test]
fn solids_are_bounded_by_the_budget_before_they_are_built() {
    let huge = alone(
        Node::new("tetbox", NodeId(1))
            .with_vec3("size", v3(1.0, 1.0, 1.0))
            .with_u32("divisions", 1000),
    );
    // 1001 cubed points is over the element limit, before any allocation.
    assert!(matches!(
        run(&huge, 1, GENEROUS),
        Err(EvalError::ElementLimit {
            limit: 1_000_000,
            ..
        })
    ));
    // Within the element limit, the memory limit still applies.
    let tight = CookBudget {
        max_memory_bytes: 1 << 10,
        ..GENEROUS
    };
    assert_eq!(
        run(&alone(solid()), 1, tight).err(),
        Some(EvalError::Resource(GeoError::LimitExceeded))
    );
    // An invalid solid is an error to report, not an empty result.
    let beside = scene(vec![
        solid(),
        Node::new("box", NodeId(2)).with_vec3("size", v3(1.0, 1.0, 1.0)),
        Node::new("merge", NodeId(3))
            .with_input(NodeId(1))
            .with_input(NodeId(2)),
        Node::new("tet_surface", NodeId(4)).with_input(NodeId(3)),
    ]);
    assert_eq!(
        run(&beside, 4, GENEROUS).err(),
        Some(EvalError::Resource(GeoError::InvalidTopology))
    );
}

#[test]
fn elements_key_the_cache_by_content() -> Result<(), EvalError> {
    let cook = |divisions: u32| {
        let graph = alone(
            Node::new("tetbox", NodeId(1))
                .with_vec3("size", v3(1.0, 1.0, 1.0))
                .with_u32("divisions", divisions),
        );
        run(&graph, 1, GENEROUS)
    };
    assert_eq!(cook(2)?.hash(), cook(2)?.hash());
    assert_ne!(cook(2)?.hash(), cook(3)?.hash());
    Ok(())
}
