//! Volume primitives through the evaluator: voxels travel as grids, not as points.

use pcc_eval::{
    CookBudget, CookCache, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets, cook_cached,
    cook_node_cached,
};
use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail};
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

fn reshape(id: u64, input: u64, offset: f32) -> Node {
    Node::new("vdb_reshape", NodeId(id))
        .with_input(NodeId(input))
        .with_f32("offset", offset)
        .with_u32("smooth_iterations", 0)
}

/// A box as node 1, and its volume as node 2.
fn voxelized() -> Vec<Node> {
    vec![
        Node::new("box", NodeId(1)).with_vec3("size", v3(4.0, 4.0, 4.0)),
        Node::new("vdb_from_polygons", NodeId(2))
            .with_input(NodeId(1))
            .with_f32("voxel_size", 0.5),
    ]
}

#[test]
fn voxels_travel_as_a_primitive_and_become_a_surface_again() -> Result<(), EvalError> {
    let mut nodes = voxelized();
    nodes.push(reshape(3, 2, 0.25));
    nodes.push(Node::new("convert_vdb", NodeId(4)).with_input(NodeId(2)));
    nodes.push(Node::new("convert_vdb", NodeId(5)).with_input(NodeId(3)));
    let graph = scene(nodes);
    let volume = run(&graph, 2, GENEROUS)?;
    assert_eq!((volume.volume_count(), volume.prim_count()), (1, 1));
    assert_eq!(volume.len(), 1, "one anchor, not one point per voxel");
    assert!(volume.attribute("phi").is_none());
    // Bounds cover the voxels, which reach past the solid by the width of the narrow band.
    let (low, high) = volume.bounds().ok_or(EvalError::InvalidGraph)?;
    assert!(low.x <= 0.0 && high.x >= 4.0, "{low:?} {high:?}");

    let surface = run(&graph, 4, GENEROUS)?;
    assert_eq!(surface.volume_count(), 0);
    assert!(surface.topology_index().is_closed_manifold());
    assert!((surface.enclosed_volume() - 64.0).abs() < 8.0);
    // Dilating a level set grows what it encloses.
    let grown = run(&graph, 5, GENEROUS)?;
    assert!(grown.enclosed_volume() > surface.enclosed_volume());
    Ok(())
}

#[test]
fn operators_that_cannot_handle_volumes_refuse_rather_than_lose_them() {
    let refuses = |op: Node| {
        let mut nodes = voxelized();
        nodes.push(op);
        let found = run(&scene(nodes), 3, GENEROUS);
        assert!(
            matches!(&found, Err(EvalError::Param { node, param, message })
                if *node == NodeId(3) && param == "input" && message.contains("convert_vdb")),
            "{found:?}"
        );
    };
    let input = NodeId(2);
    refuses(
        Node::new("transform", NodeId(3))
            .with_input(input)
            .with_vec3("offset", v3(1.0, 0.0, 0.0)),
    );
    refuses(
        Node::new("xform", NodeId(3))
            .with_input(input)
            .with_vec3("translate", Vec3::ZERO)
            .with_vec3("rotate_degrees", Vec3::ZERO)
            .with_vec3("scale", v3(1.0, 1.0, 1.0)),
    );
    refuses(Node::new("normal", NodeId(3)).with_input(input));
    refuses(Node::new("measure", NodeId(3)).with_input(input));
    refuses(Node::new("pack", NodeId(3)).with_input(input));
    refuses(Node::new("unpack", NodeId(3)).with_input(input));
    refuses(
        Node::new("vdb_from_polygons", NodeId(3))
            .with_input(input)
            .with_f32("voxel_size", 0.5),
    );
    // On either input of a two-input operator.
    refuses(
        Node::new("boolean", NodeId(3))
            .with_input(NodeId(1))
            .with_input(input)
            .with_text("mode", "union")
            .with_f32("voxel_size", 0.5),
    );
}

#[test]
fn volume_operators_refuse_anything_that_is_not_only_volumes() {
    let refuses = |extra: Vec<Node>, id: u64| {
        let mut nodes = voxelized();
        nodes.extend(extra);
        let found = run(&scene(nodes), id, GENEROUS);
        assert!(
            matches!(&found, Err(EvalError::Param { param, message, .. })
                if param == "input" && message.contains("vdb_from_polygons")),
            "{found:?}"
        );
    };
    // A plain surface holds no grid at all: this used to be a missing-attribute error.
    refuses(
        vec![Node::new("convert_vdb", NodeId(3)).with_input(NodeId(1))],
        3,
    );
    refuses(vec![reshape(3, 1, 0.1)], 3);
    refuses(
        vec![Node::new("vdb_to_points", NodeId(3)).with_input(NodeId(1))],
        3,
    );
    // A surface merged beside a volume would otherwise vanish from the result.
    let beside = || {
        Node::new("merge", NodeId(3))
            .with_input(NodeId(1))
            .with_input(NodeId(2))
    };
    refuses(
        vec![
            beside(),
            Node::new("convert_vdb", NodeId(4)).with_input(NodeId(3)),
        ],
        4,
    );
    refuses(vec![beside(), reshape(4, 3, 0.1)], 4);
    refuses(
        vec![
            beside(),
            Node::new("vdb_to_points", NodeId(4)).with_input(NodeId(3)),
        ],
        4,
    );
}

#[test]
fn merged_volumes_are_reshaped_and_converted_together() -> Result<(), EvalError> {
    // A second box, far away, with its own volume.
    let graph = scene(vec![
        Node::new("box", NodeId(1)).with_vec3("size", v3(4.0, 4.0, 4.0)),
        Node::new("transform", NodeId(2))
            .with_input(NodeId(1))
            .with_vec3("offset", v3(50.0, 0.0, 0.0)),
        Node::new("vdb_from_polygons", NodeId(3))
            .with_input(NodeId(1))
            .with_f32("voxel_size", 0.5),
        Node::new("vdb_from_polygons", NodeId(4))
            .with_input(NodeId(2))
            .with_f32("voxel_size", 0.5),
        Node::new("merge", NodeId(5))
            .with_input(NodeId(3))
            .with_input(NodeId(4)),
        reshape(6, 5, 0.25),
        Node::new("convert_vdb", NodeId(7)).with_input(NodeId(6)),
        Node::new("convert_vdb", NodeId(8)).with_input(NodeId(3)),
        reshape(9, 3, 0.25),
        Node::new("convert_vdb", NodeId(10)).with_input(NodeId(9)),
        Node::new("vdb_to_points", NodeId(11)).with_input(NodeId(5)),
        Node::new("vdb_to_points", NodeId(12)).with_input(NodeId(3)),
    ]);
    let merged = run(&graph, 6, GENEROUS)?;
    assert_eq!(merged.volume_count(), 2);
    assert_eq!(merged.bounds().map(|(_, high)| high.x > 50.0), Some(true));
    // Two equal solids enclose twice what one does, so every grid was reshaped and converted.
    let both = run(&graph, 7, GENEROUS)?;
    let one = run(&graph, 10, GENEROUS)?;
    assert!(both.topology_index().is_closed_manifold());
    assert!((both.enclosed_volume() - 2.0 * one.enclosed_volume()).abs() < 1e-2);
    // Likewise points: the merged listing holds both grids' voxels, in primitive order.
    let all = run(&graph, 11, GENEROUS)?;
    let first = run(&graph, 12, GENEROUS)?;
    assert_eq!(all.len(), 2 * first.len());
    Ok(())
}

#[test]
fn voxels_can_still_be_inspected_as_points() -> Result<(), EvalError> {
    let mut nodes = voxelized();
    nodes.push(Node::new("vdb_to_points", NodeId(3)).with_input(NodeId(2)));
    let graph = scene(nodes);
    let grid_voxels = run(&graph, 2, GENEROUS)?
        .volume(0)
        .map(|grid| grid.active_voxel_count());
    let points = run(&graph, 3, GENEROUS)?;
    assert_eq!(Some(points.len()), grid_voxels);
    assert_eq!(points.volume_count(), 0);
    for name in ["phi", "vx", "vy", "vz"] {
        assert!(points.attribute(name).is_some(), "{name}");
    }
    let size = points
        .domain_attribute(AttrDomain::Detail, "vdb_voxel_size")
        .and_then(AttributeColumn::as_f32)
        .and_then(|column| column.get(0));
    assert_eq!(size.map(|value| value.get()), Some(0.5));
    Ok(())
}

#[test]
fn a_solver_can_carry_a_volume_from_tick_to_tick() -> Result<(), EvalError> {
    // Three ticks must match three reshapes in a row, voxel for voxel.
    let mut chained = voxelized();
    chained.extend([reshape(3, 2, 0.1), reshape(4, 3, 0.1), reshape(5, 4, 0.1)]);
    let expected = run(&scene(chained), 5, GENEROUS)?;
    let mut nodes = voxelized();
    nodes.extend([
        Node::new("state_prev", NodeId(3)).with_u64("zone", 5),
        reshape(4, 3, 0.1),
        Node::new("solver", NodeId(5))
            .with_input(NodeId(2))
            .with_input(NodeId(4))
            .with_u32("ticks", 3),
    ]);
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    let stepped = cook_cached(&CookCache::default(), &scene(nodes), 0, 1, &ctx)?;
    assert_eq!(stepped.volume_count(), 1);
    assert_eq!(stepped.hash(), expected.hash());
    Ok(())
}

#[test]
fn voxels_count_against_the_element_budget() -> Result<(), EvalError> {
    let graph = scene(voxelized());
    let voxels = run(&graph, 2, GENEROUS)?
        .volume(0)
        .map_or(0, |grid| grid.active_voxel_count());
    let limit = u32::try_from(voxels).map_err(|_| EvalError::InvalidGraph)?;
    let exact = CookBudget {
        max_elements: limit,
        ..GENEROUS
    };
    assert!(run(&graph, 2, exact).is_ok());
    let below = CookBudget {
        max_elements: limit - 1,
        ..GENEROUS
    };
    assert!(matches!(
        run(&graph, 2, below),
        Err(EvalError::ElementLimit { .. })
    ));
    // Reshaping is held to the same limit: dilating turns background voxels active, so a budget
    // that the input just fits must refuse the larger output.
    let mut nodes = voxelized();
    nodes.push(reshape(3, 2, 0.5));
    let dilated = scene(nodes);
    let grown = run(&dilated, 3, GENEROUS)?
        .volume(0)
        .map_or(0, |grid| grid.active_voxel_count());
    assert!(grown > voxels, "{grown} {voxels}");
    assert!(matches!(
        run(&dilated, 3, exact),
        Err(EvalError::ElementLimit { .. })
    ));
    Ok(())
}
