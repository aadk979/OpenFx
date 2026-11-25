//! Exit evidence for Phase A1: memoized Merkle hashing, `P` as an ordinary point column,
//! lazy page hashes, and constant-page storage.

use pcc_eval::{
    CookBudget, CookCache, DEFAULT_MAX_STATE_TICKS, EvalContext, EvalError, NoAssets,
    cook_node_cached,
};
use pcc_geo::{AttributeColumn, GeoDetail, GeoError, PAGE_LEN, PagedColumn, hash_stats};
use pcc_graph::{GraphSnapshot, Node};
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use std::sync::atomic::AtomicBool;

#[test]
fn ten_million_point_recook_after_downstream_edit_does_zero_upstream_hashing()
-> Result<(), EvalError> {
    let make_graph = |rev: u64, dx: f32| GraphSnapshot {
        revision: RevisionId(rev),
        nodes: vec![
            Node::new("create", NodeId(1)).with_u32("count", 10_000_000),
            Node::new("transform", NodeId(2))
                .with_input(NodeId(1))
                .with_vec3(
                    "offset",
                    Vec3 {
                        x: 1.0,
                        y: 0.0,
                        z: 0.0,
                    },
                ),
            Node::new("transform", NodeId(3))
                .with_input(NodeId(2))
                .with_vec3(
                    "offset",
                    Vec3 {
                        x: 0.0,
                        y: 1.0,
                        z: 0.0,
                    },
                ),
            Node::new("pack", NodeId(4)).with_input(NodeId(3)),
            Node::new("transform", NodeId(5))
                .with_input(NodeId(4))
                .with_vec3(
                    "offset",
                    Vec3 {
                        x: dx,
                        y: 0.0,
                        z: 0.0,
                    },
                ),
        ],
    };
    let cache = CookCache::default();
    let cancelled = AtomicBool::new(false);
    let ctx = EvalContext {
        assets: &NoAssets,
        cancelled: &cancelled,
        budget: CookBudget {
            max_memory_bytes: 4 << 30,
            max_elements: 20_000_000,
            max_state_ticks: DEFAULT_MAX_STATE_TICKS,
        },
    };

    let first = cook_node_cached(&cache, &make_graph(1, 0.0), NodeId(5), 0, 1, &ctx)?;
    let _ = first.hash();

    let before = hash_stats();
    let second = cook_node_cached(&cache, &make_graph(2, 5.0), NodeId(5), 0, 1, &ctx)?;
    let during_recook = hash_stats().delta(before);

    // Every upstream node (create, transform 2, transform 3, pack) was a cache hit whose
    // memoized Merkle root answered in O(1) without hashing a single page, column, or detail.
    assert_eq!(during_recook.page_hashes, 0);
    assert_eq!(during_recook.column_hashes, 0);
    assert_eq!(during_recook.detail_hashes, 0);
    assert_ne!(first.hash(), second.hash());
    Ok(())
}

#[test]
fn page_hashing_is_lazy_and_single_element_edits_hash_only_the_dirty_page() -> Result<(), GeoError>
{
    let count = PAGE_LEN * 3 + 17;
    let points: Vec<Vec3> = (0..count)
        .map(|i| Vec3 {
            x: i as f32,
            y: 0.0,
            z: 0.0,
        })
        .collect();

    let start = hash_stats();
    let geo = GeoDetail::points(points);
    assert_eq!(hash_stats().delta(start).page_hashes, 0);
    assert_eq!(hash_stats().delta(start).column_hashes, 0);
    assert_eq!(hash_stats().delta(start).detail_hashes, 0);

    // First hash hashes all 4 pages, 1 column ("P"), and 1 detail.
    let first_hash = geo.hash();
    let after_first = hash_stats().delta(start);
    assert_eq!(after_first.page_hashes, 4);
    assert_eq!(after_first.column_hashes, 1);
    assert_eq!(after_first.detail_hashes, 1);

    // Repeat hash is O(1) with zero hash computations.
    let before_repeat = hash_stats();
    assert_eq!(geo.hash(), first_hash);
    let repeat_delta = hash_stats().delta(before_repeat);
    assert_eq!(repeat_delta.page_hashes, 0);
    assert_eq!(repeat_delta.column_hashes, 0);
    assert_eq!(repeat_delta.detail_hashes, 0);

    // Mutating one point in page 1 hashes only page 1 (not pages 0, 2, or 3).
    let edited = geo.with_point(
        PAGE_LEN + 5,
        Vec3 {
            x: -1.0,
            y: 2.0,
            z: 3.0,
        },
    )?;
    let before_edit_hash = hash_stats();
    let edited_hash = edited.hash();
    let edit_delta = hash_stats().delta(before_edit_hash);
    assert_eq!(edit_delta.page_hashes, 1);
    assert_eq!(edit_delta.column_hashes, 1);
    assert_eq!(edit_delta.detail_hashes, 1);
    assert_ne!(edited_hash, first_hash);
    Ok(())
}

#[test]
fn constant_pages_match_dense_hashes_and_p_is_an_ordinary_column() -> Result<(), GeoError> {
    let count = PAGE_LEN * 2 + 5;
    let constant_col = PagedColumn::constant(42_i32, count);
    let dense_col = PagedColumn::from_vec(vec![42_i32; count]);
    assert!(constant_col.is_constant_page(0));
    assert!(constant_col.is_constant_page(1));
    assert!(constant_col.is_constant_page(2));
    assert!(dense_col.is_constant_page(0));
    assert_eq!(constant_col.hash(), dense_col.hash());

    // Editing one value and changing it back to 42 yields the exact same hash.
    let perturbed = constant_col.with_element(10, 99)?;
    assert!(!perturbed.is_constant_page(0));
    assert!(perturbed.is_constant_page(1));
    assert_ne!(perturbed.hash(), constant_col.hash());
    let restored = perturbed.with_element(10, 42)?;
    assert!(restored.is_constant_page(0));
    assert_eq!(restored.hash(), constant_col.hash());

    // `P` lives in the point attribute map and can be read and replaced like any Vec3 column.
    let base = GeoDetail::points(vec![Vec3::ZERO; 4]);
    let base_hash = base.hash();
    assert_eq!(base.attribute_names(), vec!["P"]);
    assert!(matches!(
        base.attribute("P"),
        Some(AttributeColumn::Vec3(_))
    ));

    let shifted = vec![
        Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        4
    ];
    let replaced = base.with_attribute("P", AttributeColumn::vec3_values(shifted.clone()))?;
    let via_with_positions = base.with_positions(shifted)?;
    assert_eq!(replaced.hash(), via_with_positions.hash());
    assert_ne!(replaced.hash(), base_hash);
    assert_eq!(replaced.point(2), via_with_positions.point(2));
    Ok(())
}

#[test]
fn mutating_a_hashed_detail_always_invalidates_its_memoized_hash() -> Result<(), GeoError> {
    use pcc_geo::{AttrDomain, GroupDomain, PrimKind, PrimRun, PromoteMode, VdbGrid};
    use std::sync::Arc;

    let tri = GeoDetail::points(vec![
        Vec3::ZERO,
        Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
        Vec3 {
            x: 0.0,
            y: 1.0,
            z: 0.0,
        },
    ])
    .with_primitives(
        vec![PrimRun {
            kind: PrimKind::Polygon,
            arity: 3,
            prim_count: 1,
        }],
        vec![0, 1, 2],
    )?;
    let h0 = tri.hash();

    let with_f32 = tri.with_attribute("w", AttributeColumn::f32_checked(vec![1.0, 2.0, 3.0])?)?;
    assert_ne!(with_f32.hash(), h0);

    let h_f32 = with_f32.hash();
    let edited_f32 = with_f32.with_f32_attribute_element("w", 1, 9.0)?;
    assert_ne!(edited_f32.hash(), h_f32);

    let with_v3 = tri.with_attribute("N", AttributeColumn::vec3_values(vec![Vec3::UP; 3]))?;
    let h_v3 = with_v3.hash();
    let edited_v3 = with_v3.with_vec3_attribute_element(
        "N",
        0,
        Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
    )?;
    assert_ne!(edited_v3.hash(), h_v3);

    let promoted =
        with_f32.promote_attribute("w", AttrDomain::Point, AttrDomain::Detail, PromoteMode::Sum)?;
    assert_ne!(promoted.hash(), h_f32);

    let grouped = tri.with_group(GroupDomain::Point, "sel", &[0])?;
    assert_ne!(grouped.hash(), h0);

    let reversed = tri.with_reversed_winding()?;
    assert_ne!(reversed.hash(), h0);

    let translated = tri.translated(
        Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
        1 << 20,
    )?;
    assert_ne!(translated.hash(), h0);

    let packed = GeoDetail::packed_whole(Arc::new(tri))?;
    let h_packed = packed.hash();
    let scaled_packed = packed.with_packed_linear(&[
        Vec3 {
            x: 2.0,
            y: 0.0,
            z: 0.0,
        },
        Vec3 {
            x: 0.0,
            y: 2.0,
            z: 0.0,
        },
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 2.0,
        },
    ])?;
    assert_ne!(scaled_packed.hash(), h_packed);

    let grid = Arc::new(VdbGrid::new(0.5, 1.0)?);
    let vol = GeoDetail::from_volumes(vec![grid])?;
    let h_vol = vol.hash();
    let grid2 = Arc::new(VdbGrid::new(0.25, 1.0)?);
    let vol2 = vol.with_volume_grids(vec![grid2])?;
    assert_ne!(vol2.hash(), h_vol);

    Ok(())
}
