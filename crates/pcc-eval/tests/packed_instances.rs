//! Packed instances through the evaluator, including the audit's million-instance requirement.

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
    max_memory_bytes: 1 << 30,
    max_elements: 10_000_000,
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

fn xform(id: u64, input: u64, rotate: Vec3, scale: Vec3, translate: Vec3) -> Node {
    Node::new("xform", NodeId(id))
        .with_input(NodeId(input))
        .with_vec3("translate", translate)
        .with_vec3("rotate_degrees", rotate)
        .with_vec3("scale", scale)
}

/// `count` instances of a box along +X, each with its own point.
fn instanced(count: u32) -> Vec<Node> {
    vec![
        Node::new("box", NodeId(1)).with_vec3("size", v3(1.0, 1.0, 1.0)),
        Node::new("create", NodeId(2)).with_u32("count", count),
        Node::new("instance", NodeId(3))
            .with_input(NodeId(1))
            .with_input(NodeId(2)),
    ]
}

#[test]
fn a_million_instances_fit_where_their_expansion_cannot() -> Result<(), EvalError> {
    const INSTANCES: u32 = 1_000_000;
    // A box unpacked a million times is 8 million points and 24 million vertices: 192 MB.
    let budget = CookBudget {
        max_memory_bytes: 120 << 20,
        ..GENEROUS
    };
    let mut nodes = instanced(INSTANCES);
    nodes.push(xform(
        4,
        3,
        Vec3::ZERO,
        v3(2.0, 2.0, 2.0),
        v3(0.0, 5.0, 0.0),
    ));
    nodes.push(Node::new("unpack", NodeId(5)).with_input(NodeId(4)));
    let graph = scene(nodes);
    let moved = run(&graph, 4, budget)?;
    assert_eq!(moved.packed_count(), INSTANCES as usize);
    assert_eq!(moved.len(), INSTANCES as usize, "one point per instance");
    assert_eq!(
        moved.packed_prototypes().len(),
        1,
        "the prototype is never copied"
    );
    assert_eq!(moved.packed_prototypes()[0].geo().len(), 8);
    // Bounds still reach the real extent of the far instance, doubled in size and moved up.
    let (_, high) = moved.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
    assert_eq!(high.y, 7.0);
    // The expansion is refused, proving that nothing above expanded implicitly.
    assert_eq!(
        run(&graph, 5, budget).err(),
        Some(EvalError::Resource(GeoError::LimitExceeded))
    );
    Ok(())
}

/// Moving instances then expanding must match expanding then moving, point for point, for every
/// kind of transform, including winding under mirroring.
#[test]
fn transforming_and_unpacking_commute() -> Result<(), EvalError> {
    let cases = [
        ("identity", Vec3::ZERO, v3(1.0, 1.0, 1.0)),
        ("rotated", v3(30.0, 40.0, 50.0), v3(1.0, 1.0, 1.0)),
        ("stretched", Vec3::ZERO, v3(2.0, 0.5, 3.0)),
        ("both", v3(15.0, 0.0, 70.0), v3(1.5, 2.0, 0.25)),
        ("mirrored", v3(10.0, 20.0, 30.0), v3(-1.0, 1.0, 2.0)),
        ("twice mirrored", Vec3::ZERO, v3(-1.0, -1.0, 2.0)),
    ];
    let translate = v3(3.0, -4.0, 5.0);
    for (name, rotate, scale) in cases {
        let mut packed_first = instanced(5);
        packed_first.push(xform(4, 3, rotate, scale, translate));
        packed_first.push(Node::new("unpack", NodeId(5)).with_input(NodeId(4)));
        let mut unpacked_first = instanced(5);
        unpacked_first.push(Node::new("unpack", NodeId(4)).with_input(NodeId(3)));
        unpacked_first.push(xform(5, 4, rotate, scale, translate));

        let a = run(&scene(packed_first), 5, GENEROUS)?;
        let b = run(&scene(unpacked_first), 5, GENEROUS)?;
        assert_eq!(
            (a.len(), a.prim_count()),
            (b.len(), b.prim_count()),
            "{name}"
        );
        for i in 0..a.len() {
            let (p, q) = (a.point(i), b.point(i));
            let gap = match (p, q) {
                (Some(p), Some(q)) => (p.x - q.x).abs() + (p.y - q.y).abs() + (p.z - q.z).abs(),
                _ => f32::INFINITY,
            };
            assert!(gap < 1e-3, "{name}: point {i}: {p:?} vs {q:?}");
        }
        for vertex in 0..a.vertex_count() {
            assert_eq!(
                a.vertex_point(vertex),
                b.vertex_point(vertex),
                "{name}: winding"
            );
        }
        assert!(pcc_sop::signed_volume(&a) > 0.0, "{name}: inside out");
        assert!(a.topology_index().is_closed_manifold(), "{name}");
    }
    Ok(())
}

#[test]
fn pscale_sizes_each_instance_and_only_prototypes_are_shared() -> Result<(), EvalError> {
    let mut nodes = instanced(3);
    // Sizes 1, 2, and 3, read from a point attribute.
    nodes[2] = Node::new("wrangle", NodeId(6))
        .with_input(NodeId(2))
        .with_text("snippet", "@pscale = @ptnum + 1;");
    nodes.push(
        Node::new("instance", NodeId(3))
            .with_input(NodeId(1))
            .with_input(NodeId(6)),
    );
    nodes.push(Node::new("unpack", NodeId(4)).with_input(NodeId(3)));
    let graph = scene(nodes);
    let flat = run(&graph, 4, GENEROUS)?;
    // Unit boxes cubed by their scale: 1 + 8 + 27.
    assert!((pcc_sop::signed_volume(&flat) - 36.0).abs() < 1e-3);
    let placed = run(&graph, 3, GENEROUS)?;
    assert!(
        placed.attribute("pscale").is_some(),
        "target points keep their attributes"
    );
    Ok(())
}

#[test]
fn operators_that_cannot_handle_instances_refuse_rather_than_lose_them() -> Result<(), EvalError> {
    let refuses = |op: Node| {
        let mut nodes = instanced(2);
        nodes.push(op);
        let found = run(&scene(nodes), 4, GENEROUS);
        assert!(
            matches!(&found, Err(EvalError::Param { node, param, message })
                if *node == NodeId(4) && param == "input" && message.contains("unpack")),
            "{found:?}"
        );
    };
    let input = NodeId(3);
    refuses(
        Node::new("polyreduce", NodeId(4))
            .with_input(input)
            .with_f32("target_ratio", 0.5),
    );
    refuses(Node::new("triangulate", NodeId(4)).with_input(input));
    refuses(Node::new("reverse", NodeId(4)).with_input(input));
    refuses(Node::new("normal", NodeId(4)).with_input(input));
    refuses(Node::new("measure", NodeId(4)).with_input(input));
    refuses(
        Node::new("blast", NodeId(4))
            .with_input(input)
            .with_text("group_name", "g")
            .with_bool("negate", false),
    );
    // On either input of a two-input operator.
    let plain = NodeId(1);
    refuses(
        Node::new("boolean", NodeId(4))
            .with_input(plain)
            .with_input(input)
            .with_text("mode", "union")
            .with_f32("voxel_size", 0.5),
    );
    // Operators that do accept them carry them through, ending in the same count.
    let mut nodes = instanced(2);
    nodes.push(
        Node::new("transform", NodeId(4))
            .with_input(input)
            .with_vec3("offset", v3(0.0, 1.0, 0.0)),
    );
    nodes.push(
        Node::new("merge", NodeId(5))
            .with_input(NodeId(4))
            .with_input(input),
    );
    let merged = run(&scene(nodes), 5, GENEROUS)?;
    assert_eq!(merged.packed_count(), 4);
    assert_eq!(merged.packed_prototypes().len(), 1);
    Ok(())
}

#[test]
fn a_solver_can_move_instances_and_nesting_stops_with_an_error() -> Result<(), EvalError> {
    // Rising a million would be too slow to test by expanding, so few ticks, many instances.
    let mut rising = instanced(100_000);
    rising.extend([
        Node::new("state_prev", NodeId(4)).with_u64("zone", 6),
        xform(5, 4, Vec3::ZERO, v3(1.0, 1.0, 1.0), v3(0.0, 1.0, 0.0)),
        Node::new("solver", NodeId(6))
            .with_input(NodeId(3))
            .with_input(NodeId(5))
            .with_u32("ticks", 10),
    ]);
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    let moved = cook_cached(&CookCache::default(), &scene(rising), 0, 1, &ctx)?;
    assert_eq!(moved.packed_count(), 100_000);
    assert_eq!(moved.bounds().map(|(_, high)| high.y), Some(11.0));

    // Packing in a loop would nest without end. It must fail cleanly, not overflow the stack.
    let nesting = scene(vec![
        Node::new("box", NodeId(1)).with_vec3("size", v3(1.0, 1.0, 1.0)),
        Node::new("state_prev", NodeId(2)).with_u64("zone", 4),
        Node::new("pack", NodeId(3)).with_input(NodeId(2)),
        Node::new("solver", NodeId(4))
            .with_input(NodeId(1))
            .with_input(NodeId(3))
            .with_u32("ticks", 1000),
    ]);
    assert_eq!(
        cook_cached(&CookCache::default(), &nesting, 0, 1, &ctx).err(),
        Some(EvalError::Resource(GeoError::LimitExceeded))
    );
    Ok(())
}

#[test]
fn instances_key_the_cache_by_content() -> Result<(), EvalError> {
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    let graph = |size: f32| {
        let mut nodes = instanced(2);
        nodes[0] = Node::new("box", NodeId(1)).with_vec3("size", v3(size, 1.0, 1.0));
        nodes.push(Node::new("unpack", NodeId(4)).with_input(NodeId(3)));
        scene(nodes)
    };
    let cache = CookCache::default();
    let small = cook_cached(&cache, &graph(1.0), 0, 1, &ctx)?;
    let large = cook_cached(&cache, &graph(4.0), 0, 1, &ctx)?;
    // A new prototype flows through instances that otherwise look the same.
    assert_ne!(small.hash(), large.hash());
    assert_eq!(large.bounds().map(|(_, high)| high.x), Some(5.0));
    Ok(())
}
