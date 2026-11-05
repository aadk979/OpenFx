//! Solvers and foreach, driven through the public evaluator API.

use pcc_eval::{
    CookBudget, CookCache, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets, cook_cached,
    cook_node_cached,
};
use pcc_geo::{AttrDomain, AttributeColumn, GeoDetail};
use pcc_graph::{GraphSnapshot, Node};
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const GENEROUS: CookBudget = CookBudget {
    max_memory_bytes: 1 << 30,
    max_elements: 5_000_000,
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

fn unit_box(id: u64) -> Node {
    Node::new("box", NodeId(id)).with_vec3("size", v3(1.0, 1.0, 1.0))
}

fn lift(id: u64, input: u64, y: &str) -> Node {
    Node::new("xform", NodeId(id))
        .with_input(NodeId(input))
        .with_vec3_expr("translate", ["0", y, "0"])
        .with_vec3("rotate_degrees", Vec3::ZERO)
        .with_vec3("scale", v3(1.0, 1.0, 1.0))
}

fn state_prev(id: u64, zone: u64) -> Node {
    Node::new("state_prev", NodeId(id)).with_u64("zone", zone)
}

fn solver(id: u64, initial: u64, output: u64, ticks: &str) -> Node {
    Node::new("solver", NodeId(id))
        .with_input(NodeId(initial))
        .with_input(NodeId(output))
        .with_expr("ticks", ticks)
}

/// A box that rises by `step` each tick, advancing with the timeline.
fn rising(step: &str) -> GraphSnapshot {
    scene(vec![
        unit_box(1),
        state_prev(2, 4),
        lift(3, 2, step),
        solver(4, 1, 3, "$F"),
    ])
}

fn run(
    cache: &CookCache,
    graph: &GraphSnapshot,
    frame: i64,
    budget: CookBudget,
) -> Result<Arc<GeoDetail>, EvalError> {
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget,
    };
    cook_cached(cache, graph, frame, 1, &ctx)
}

fn lowest_y(geo: &GeoDetail) -> f32 {
    geo.bounds().map_or(f32::NAN, |(low, _)| low.y)
}

#[test]
fn state_accumulates_exactly_as_an_analytic_sum() -> Result<(), EvalError> {
    let cache = CookCache::default();
    let start = lowest_y(&*run(&cache, &rising("1"), 0, GENEROUS)?);
    for ticks in [1i64, 2, 3, 10, 50] {
        // A constant step gives n, and a step of the tick number gives n(n + 1) / 2.
        let constant = lowest_y(&*run(&cache, &rising("1"), ticks, GENEROUS)?);
        assert!(
            (constant - start - ticks as f32).abs() < 1e-3,
            "{ticks}: {constant}"
        );
        let triangular = lowest_y(&*run(&cache, &rising("$F"), ticks, GENEROUS)?);
        let expected = (ticks * (ticks + 1) / 2) as f32;
        assert!(
            (triangular - start - expected).abs() < 1e-2,
            "{ticks}: {triangular}"
        );
    }
    Ok(())
}

#[test]
fn zero_ticks_is_the_initial_state_untouched() -> Result<(), EvalError> {
    let cache = CookCache::default();
    let graph = rising("1");
    let initial = cook_node_for_test(&cache, &graph, 1, 5)?;
    let at_zero = run(&cache, &graph, 0, GENEROUS)?;
    assert_eq!(at_zero.hash(), initial.hash());
    assert_eq!(cache.zone_ticks_run(), 0);
    Ok(())
}

fn cook_node_for_test(
    cache: &CookCache,
    graph: &GraphSnapshot,
    id: u64,
    frame: i64,
) -> Result<Arc<GeoDetail>, EvalError> {
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    cook_node_cached(cache, graph, NodeId(id), frame, 1, &ctx)
}

#[test]
fn seeking_restores_the_latest_checkpoint_and_matches_a_linear_run() -> Result<(), EvalError> {
    let graph = rising("$F * 0.5 + 1");
    let linear = |frame: i64| -> Result<blake3::Hash, EvalError> {
        Ok(run(&CookCache::default(), &graph, frame, GENEROUS)?.hash())
    };
    let cache = CookCache::default();
    // Cooks the solver node directly, since the whole-scene cache would answer a repeated frame
    // before the solver ever ran. After each seek: the ticks that seek must have computed.
    // Checkpoints land on multiples of eight and on each seek's final tick.
    for (frame, computed) in [
        (5i64, 5u64), // From nothing.
        (20, 15),     // From the checkpoint at 5.
        (7, 2),       // Backwards: from 5, the latest at or before 7.
        (7, 0),       // Exactly on a checkpoint.
        (19, 3),      // From the one at 16, a multiple of eight on the way to 20.
        (3, 3),       // Before every checkpoint: from the start.
        (21, 1),      // From the one at 20.
    ] {
        let before = cache.zone_ticks_run();
        let sought = cook_node_for_test(&cache, &graph, 4, frame)?;
        assert_eq!(
            sought.hash(),
            linear(frame)?,
            "frame {frame} differs from a linear run"
        );
        assert_eq!(cache.zone_ticks_run() - before, computed, "frame {frame}");
    }
    Ok(())
}

#[test]
fn a_changed_body_or_initial_state_never_restores_stale_checkpoints() -> Result<(), EvalError> {
    let cache = CookCache::default();
    let at_ten = run(&cache, &rising("1"), 10, GENEROUS)?;
    let before = cache.zone_ticks_run();
    // A different body recomputes everything rather than restoring the other's states.
    let other = run(&cache, &rising("2"), 10, GENEROUS)?;
    assert_eq!(cache.zone_ticks_run() - before, 10);
    assert_ne!(other.hash(), at_ten.hash());
    // A different initial state does too.
    let mut moved = rising("1");
    moved.nodes[0] = Node::new("box", NodeId(1)).with_vec3("size", v3(2.0, 2.0, 2.0));
    let before = cache.zone_ticks_run();
    let resized = run(&cache, &moved, 10, GENEROUS)?;
    assert_eq!(cache.zone_ticks_run() - before, 10);
    assert_ne!(resized.hash(), at_ten.hash());
    // The original is still restored whole, since its content is unchanged.
    let before = cache.zone_ticks_run();
    let again = cook_node_for_test(&cache, &rising("1"), 4, 10)?;
    assert_eq!(again.hash(), at_ten.hash());
    assert_eq!(cache.zone_ticks_run(), before);
    Ok(())
}

#[test]
fn the_body_reads_outside_geometry_and_values_and_tracks_their_changes() -> Result<(), EvalError> {
    // Adds a box each tick, risen by a value read from a node outside the body.
    let graph = |radius: &str, extra_size: f32| {
        scene(vec![
            unit_box(1),
            Node::new("box", NodeId(2)).with_vec3("size", v3(extra_size, 1.0, 1.0)),
            Node::new("sphere", NodeId(3))
                .with_expr("radius", radius)
                .with_u32("rows", 4)
                .with_u32("cols", 4),
            state_prev(4, 7),
            lift(5, 4, "ch(3, \"radius\")"),
            Node::new("merge", NodeId(6))
                .with_input(NodeId(5))
                .with_input(NodeId(2)),
            solver(7, 1, 6, "3"),
        ])
    };
    let cache = CookCache::default();
    let base = run(&cache, &graph("2", 1.0), 0, GENEROUS)?;
    assert_eq!(base.len(), 8 * 4, "the initial box plus three added ones");
    // Rising by 2 per tick, the initial box ends 6 up.
    assert_eq!(base.bounds().map(|(_, high)| high.y), Some(7.0));

    // Changing a value or geometry read from outside must not restore the old result.
    let raised = run(&cache, &graph("3", 1.0), 0, GENEROUS)?;
    assert_eq!(raised.bounds().map(|(_, high)| high.y), Some(10.0));
    let wider = run(&cache, &graph("2", 5.0), 0, GENEROUS)?;
    assert_ne!(wider.hash(), base.hash());
    assert_eq!(wider.bounds().map(|(_, high)| high.x), Some(5.0));
    Ok(())
}

/// Each part of a zone's identity is tested alone, so that none hides behind another.
#[test]
fn checkpoints_track_outside_geometry_and_outside_values_independently() -> Result<(), EvalError> {
    let body_with = |outside: Vec<Node>, rise: &str| {
        let mut nodes = vec![unit_box(1)];
        nodes.extend(outside);
        nodes.extend([
            state_prev(10, 13),
            lift(11, 10, rise),
            Node::new("merge", NodeId(12))
                .with_input(NodeId(11))
                .with_input(NodeId(3)),
            solver(13, 1, 12, "2"),
        ]);
        scene(nodes)
    };
    // Node 3 has an identical description in both scenes, but different geometry upstream.
    let reversed_box = |size: f32| {
        vec![
            Node::new("box", NodeId(2)).with_vec3("size", v3(size, 1.0, 1.0)),
            Node::new("reverse", NodeId(3)).with_input(NodeId(2)),
        ]
    };
    let cache = CookCache::default();
    let small = cook_node_for_test(&cache, &body_with(reversed_box(1.0), "1"), 13, 0)?;
    let large = cook_node_for_test(&cache, &body_with(reversed_box(9.0), "1"), 13, 0)?;
    assert_eq!(large.bounds().map(|(_, high)| high.x), Some(9.0));
    assert_ne!(small.hash(), large.hash());

    // Here node 3's geometry is identical in both, and only a value read by `ch` differs.
    let welded = |distance: f32| {
        vec![
            Node::new("box", NodeId(2)).with_vec3("size", v3(1.0, 1.0, 1.0)),
            Node::new("fuse", NodeId(3))
                .with_input(NodeId(2))
                .with_f32("distance", distance),
        ]
    };
    let cache = CookCache::default();
    let reads = "ch(3, \"distance\")";
    let low = cook_node_for_test(&cache, &body_with(welded(0.001), reads), 13, 0)?;
    let high = cook_node_for_test(&cache, &body_with(welded(0.002), reads), 13, 0)?;
    assert_eq!(
        cook_node_for_test(&cache, &body_with(welded(0.001), "0"), 3, 0)?.hash(),
        cook_node_for_test(&cache, &body_with(welded(0.002), "0"), 3, 0)?.hash(),
        "the outside geometry must really be the same for this case to mean anything"
    );
    assert_ne!(low.hash(), high.hash(), "a stale checkpoint was restored");
    Ok(())
}

#[test]
fn a_body_is_not_evaluated_at_the_scene_frame() -> Result<(), EvalError> {
    // This formula fails at $F = 3, a value no tick reaches when there are only two ticks. Were the
    // main pass to run the body too, it would fail at frame 3 for no reason.
    let graph = scene(vec![
        unit_box(1),
        state_prev(2, 4),
        lift(3, 2, "1 / ($F - 3)"),
        solver(4, 1, 3, "2"),
    ]);
    let cache = CookCache::default();
    assert!(run(&cache, &graph, 3, GENEROUS).is_ok());
    // The same holds when the solver is the node asked for.
    assert!(cook_node_for_test(&CookCache::default(), &graph, 4, 3).is_ok());
    // Asked for directly, the same node does evaluate at that frame, and says what went wrong.
    assert!(matches!(
        cook_node_for_test(&cache, &graph, 3, 3),
        Err(EvalError::Param { .. })
    ));
    Ok(())
}

#[test]
fn operators_reading_time_see_the_tick_inside_a_body() -> Result<(), EvalError> {
    let graph = scene(vec![
        Node::new("create", NodeId(1)).with_u32("count", 1),
        state_prev(2, 4),
        Node::new("wrangle", NodeId(3))
            .with_input(NodeId(2))
            .with_text("snippet", "@P.y = @P.y + @Frame;"),
        solver(4, 1, 3, "$F"),
    ]);
    let cache = CookCache::default();
    let at_four = run(&cache, &graph, 4, GENEROUS)?;
    // 1 + 2 + 3 + 4.
    assert_eq!(at_four.point(0).map(|p| p.y), Some(10.0));
    // Seeking back and replaying sees the same ticks, whatever frame is asked for.
    assert_eq!(
        run(&cache, &graph, 2, GENEROUS)?.point(0).map(|p| p.y),
        Some(3.0)
    );
    assert_eq!(run(&cache, &graph, 4, GENEROUS)?.hash(), at_four.hash());
    Ok(())
}

#[test]
fn zones_share_the_cook_budget_and_stop_when_cancelled() {
    let cache = CookCache::default();
    let tight = CookBudget {
        max_state_ticks: 5,
        ..GENEROUS
    };
    let graph = rising("1");
    assert!(run(&cache, &graph, 5, tight).is_ok());
    assert_eq!(
        run(&cache, &graph, 6, tight).err(),
        Some(EvalError::StateTickLimit {
            requested: 6,
            limit: 5
        })
    );
    // State that doubles every tick hits the element limit rather than exhausting memory.
    let doubling = scene(vec![
        unit_box(1),
        state_prev(2, 4),
        Node::new("merge", NodeId(3))
            .with_input(NodeId(2))
            .with_input(NodeId(2)),
        solver(4, 1, 3, "1000"),
    ]);
    let few = CookBudget {
        max_elements: 1000,
        ..GENEROUS
    };
    assert!(matches!(
        run(&cache, &doubling, 0, few),
        Err(EvalError::ElementLimit { .. })
    ));
    let starved = CookBudget {
        max_memory_bytes: 2000,
        ..GENEROUS
    };
    assert!(matches!(
        run(&CookCache::default(), &doubling, 0, starved),
        Err(EvalError::Resource(_))
    ));
    // Cancellation is observed in the middle of a long run, with little of it done.
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    cancelled.store(true, Ordering::Release);
    let fresh = CookCache::default();
    assert_eq!(
        cook_cached(&fresh, &rising("1"), 100_000, 1, &ctx).err(),
        Some(EvalError::Cancelled)
    );
    assert_eq!(fresh.zone_ticks_run(), 0);
}

#[test]
fn a_body_runs_only_when_needed_and_its_nodes_can_be_inspected() -> Result<(), EvalError> {
    let mut graph = rising("1");
    graph
        .nodes
        .insert(1, Node::new("normal", NodeId(9)).with_input(NodeId(1)));
    let cache = CookCache::default();
    // A node outside the zone needs no ticks at all.
    cook_node_for_test(&cache, &graph, 9, 100)?;
    assert_eq!(cache.zone_ticks_run(), 0);
    // A node inside shows one evaluation from the source, also without running the solver.
    let initial = cook_node_for_test(&cache, &graph, 1, 100)?;
    let entry = cook_node_for_test(&cache, &graph, 2, 100)?;
    assert_eq!(entry.hash(), initial.hash());
    let inside = cook_node_for_test(&cache, &graph, 3, 100)?;
    assert!((lowest_y(&inside) - lowest_y(&initial) - 1.0).abs() < 1e-5);
    assert_eq!(cache.zone_ticks_run(), 0);
    Ok(())
}

#[test]
fn results_do_not_depend_on_the_cache() -> Result<(), EvalError> {
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: GENEROUS,
    };
    let graph = rising("sin($F) + 2");
    let plain = pcc_eval::cook_with_progress(&graph, &ctx, |_, _| {})?;
    // cook_with_progress evaluates at frame 0, so ask for the same frame with a cache.
    let cached = cook_cached(&CookCache::default(), &graph, 0, 1, &ctx)?;
    assert_eq!(plain.hash(), cached.hash());
    Ok(())
}

/// Three Voronoi cells of a box, each lifted by a hundred times its ordinal.
fn exploded(attribute: &str) -> GraphSnapshot {
    let seed = |x: f32| Vec3 { x, y: 0.5, z: 0.5 };
    scene(vec![
        unit_box(1),
        Node::new("line", NodeId(2))
            .with_vec3("start", seed(0.2))
            .with_vec3("end", seed(0.8))
            .with_u32("points", 3),
        Node::new("voronoifracture", NodeId(3))
            .with_input(NodeId(1))
            .with_input(NodeId(2)),
        Node::new("foreach_piece", NodeId(4)).with_u64("zone", 6),
        lift(5, 4, "$F * 100"),
        Node::new("foreach", NodeId(6))
            .with_input(NodeId(3))
            .with_input(NodeId(5))
            .with_text("attribute", attribute),
    ])
}

#[test]
fn foreach_processes_pieces_in_order_and_merges_them() -> Result<(), EvalError> {
    let cache = CookCache::default();
    let graph = exploded("piece");
    let source = cook_node_for_test(&cache, &graph, 3, 0)?;
    let merged = run(&cache, &graph, 0, GENEROUS)?;
    assert_eq!(merged.prim_count(), source.prim_count());
    assert_eq!(merged.len(), source.len());
    assert!((merged.enclosed_volume() - source.enclosed_volume()).abs() < 1e-3);
    // Each primitive sits at a height set by its piece, and pieces run in ascending order.
    let tags = merged
        .domain_attribute(AttrDomain::Prim, "piece")
        .and_then(AttributeColumn::as_i32)
        .ok_or(EvalError::InvalidGraph)?;
    let mut vertex = 0usize;
    for prim in 0..merged.prim_count() {
        let tag = tags.get(prim).ok_or(EvalError::InvalidGraph)?;
        let point = merged
            .vertex_point(vertex)
            .and_then(|p| merged.point(p as usize))
            .ok_or(EvalError::InvalidGraph)?;
        assert_eq!((point.y / 100.0).floor() as i32, tag, "primitive {prim}");
        vertex += 3;
    }
    // Pieces with the same seed ordering always produce the same result.
    assert_eq!(
        run(&CookCache::default(), &graph, 0, GENEROUS)?.hash(),
        merged.hash()
    );
    Ok(())
}

#[test]
fn foreach_reports_bad_attributes_and_obeys_the_budget() {
    let cache = CookCache::default();
    let fails_on_attribute = |result: Result<Arc<GeoDetail>, EvalError>| matches!(result, Err(EvalError::Param { param, .. }) if param == "attribute");
    assert!(fails_on_attribute(run(
        &cache,
        &exploded("missing"),
        0,
        GENEROUS
    )));
    // A float attribute cannot name a piece.
    let mut floats = exploded("area");
    floats.nodes[2] = Node::new("box", NodeId(3)).with_vec3("size", v3(1.0, 1.0, 1.0));
    floats
        .nodes
        .insert(3, Node::new("measure", NodeId(7)).with_input(NodeId(3)));
    floats.nodes[6].inputs[0] = NodeId(7);
    assert!(fails_on_attribute(run(&cache, &floats, 0, GENEROUS)));
    // Each piece is small enough on its own, but the merged total is not.
    let mut padded = exploded("piece");
    padded
        .nodes
        .insert(1, Node::new("create", NodeId(8)).with_u32("count", 1000));
    let body = padded.nodes.iter().position(|n| n.id == NodeId(5));
    padded.nodes.insert(
        body.map_or(0, |at| at + 1),
        Node::new("merge", NodeId(9))
            .with_input(NodeId(5))
            .with_input(NodeId(8)),
    );
    if let Some(owner) = padded.nodes.iter_mut().find(|n| n.id == NodeId(6)) {
        owner.inputs[1] = NodeId(9);
    }
    let small_total = CookBudget {
        max_elements: 1500,
        ..GENEROUS
    };
    assert!(run(&cache, &padded, 0, GENEROUS).is_ok());
    assert!(matches!(
        run(&CookCache::default(), &padded, 0, small_total),
        Err(EvalError::ElementLimit { .. })
    ));
    let one_piece = CookBudget {
        max_state_ticks: 1,
        ..GENEROUS
    };
    assert_eq!(
        run(&cache, &exploded("piece"), 0, one_piece).err(),
        Some(EvalError::StateTickLimit {
            requested: 3,
            limit: 1
        })
    );
}
