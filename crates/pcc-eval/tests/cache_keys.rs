//! Cache entries are addressed by content, so a revision number must neither split nor block them.

use pcc_eval::{
    CookBudget, CookCache, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets,
    cook_node_cached,
};
use pcc_graph::GraphSnapshot;
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::sync::atomic::AtomicBool;

fn scene(revision: u64, count: u32) -> GraphSnapshot {
    let mut scene = GraphSnapshot::demo(
        count,
        Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
    );
    scene.revision = RevisionId(revision);
    scene
}

#[test]
fn a_scene_keeps_its_key_across_revisions() {
    // Undo publishes an earlier scene under a new revision. It must find its earlier result.
    assert_eq!(
        CookCache::key(&scene(1, 100), 5, 1),
        CookCache::key(&scene(9, 100), 5, 1)
    );
    assert_ne!(
        CookCache::key(&scene(1, 100), 5, 1),
        CookCache::key(&scene(1, 101), 5, 1)
    );
    assert_ne!(
        CookCache::key(&scene(1, 100), 5, 1),
        CookCache::key(&scene(1, 100), 6, 1)
    );
    assert_ne!(
        CookCache::key(&scene(1, 100), 5, 1),
        CookCache::key(&scene(1, 100), 5, 2)
    );
}

#[test]
fn a_cook_behind_the_latest_revision_still_fills_the_node_cache() -> Result<(), EvalError> {
    let cache = CookCache::default();
    // The live scene has moved on to revision 7, as it has for any sandbox that began earlier.
    cache.note_published_revision(RevisionId(7));
    let behind = scene(3, 100);
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        cancelled: &cancelled,
        assets: &NoAssets,
        budget: CookBudget {
            max_memory_bytes: 1 << 20,
            max_elements: 1_000,
            max_state_ticks: DEFAULT_MAX_STATE_TICKS,
        },
    };
    let first = cook_node_cached(&cache, &behind, NodeId(2), 0, 1, &ctx)?;
    assert_eq!(cache.node_cache_hits(), 0);
    let second = cook_node_cached(&cache, &behind, NodeId(2), 0, 1, &ctx)?;
    // Both nodes of the scene are answered from the cache the second time.
    assert_eq!(cache.node_cache_hits(), 2);
    assert_eq!(first.hash(), second.hash());
    Ok(())
}
