//! Deterministic DAG evaluator with whole-graph and per-node content-addressed caching
//! and early cutoff.
use pcc_geo::{
    AttrDomain as GeoAttrDomain, AttributeColumn, GeoDetail, GeoError, GroupDomain, PAGE_LEN,
    PrimKind, PrimRun, PromoteMode as GeoPromoteMode, UvProjectMode as GeoUvProjectMode,
};
use pcc_graph::{AttrDomain, BooleanMode, GraphSnapshot, Node, PromoteMode, UvProjectMode};
use pcc_id::{NodeId, RevisionId};
use pcc_math::{Philox4x32, Vec3};
use pcc_sop::{
    RemeshParams, SopContext, SurfaceScatter, Trs, attrib_transfer, clip_halfspace, create_line,
    create_sphere, create_torus, create_tube, fuse_points, measure, poly_bevel, poly_reduce,
    remesh, resample_curves, reverse_winding, scatter_surface, sweep_curves, transform_trs,
    triangulate, uv_flatten, uv_layout, voronoi_fracture,
};
use pcc_terrain::{HeightField, HfErodeParams, HfNoiseParams};
use pcc_vdb::{VdbCombineOp, VdbGrid, boolean_meshes_cancellable};
use pcc_vex::{DEFAULT_MAX_FUEL_PER_POINT, VexError, compile_vex};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvalError {
    Cancelled,
    InvalidGraph,
    NodeNotFound(NodeId),
    Resource(GeoError),
    Vex(VexError),
    ElementLimit { requested: u32, limit: u32 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CookBudget {
    pub max_memory_bytes: usize,
    pub max_elements: u32,
}
impl CookBudget {
    /// Enforces that child/job budgets never exceed the parent budget.
    #[must_use]
    pub fn clamp_to(self, parent: Self) -> Self {
        Self {
            max_memory_bytes: self.max_memory_bytes.min(parent.max_memory_bytes),
            max_elements: self.max_elements.min(parent.max_elements),
        }
    }
}
pub struct EvalContext<'a> {
    pub cancelled: &'a AtomicBool,
    pub budget: CookBudget,
}
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CacheKey([u8; 32]);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NodeCacheKey([u8; 32]);

#[derive(Clone, Debug)]
struct CacheEntry {
    revision: RevisionId,
    geo: Arc<GeoDetail>,
}

pub struct CookCache {
    latest_revision: Mutex<RevisionId>,
    entries: Mutex<BTreeMap<CacheKey, CacheEntry>>,
    node_entries: Mutex<BTreeMap<NodeCacheKey, CacheEntry>>,
    node_hits: AtomicU64,
}
impl Default for CookCache {
    fn default() -> Self {
        Self {
            latest_revision: Mutex::new(RevisionId(0)),
            entries: Mutex::new(BTreeMap::new()),
            node_entries: Mutex::new(BTreeMap::new()),
            node_hits: AtomicU64::new(0),
        }
    }
}
impl CookCache {
    #[must_use]
    pub fn key(snapshot: &GraphSnapshot, frame_numerator: i64, frame_denominator: u32) -> CacheKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-eval-cache-v1");
        hasher.update(snapshot.to_ofxs().as_bytes());
        hasher.update(&frame_numerator.to_le_bytes());
        hasher.update(&frame_denominator.to_le_bytes());
        CacheKey(*hasher.finalize().as_bytes())
    }
    #[must_use]
    pub fn node_key(
        node: &Node,
        upstream_hashes: &[blake3::Hash],
        frame_numerator: i64,
        frame_denominator: u32,
    ) -> NodeCacheKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pcc-eval-node-cache-v1");
        match node {
            Node::CreatePoints { count, .. } => {
                hasher.update(b"create");
                hasher.update(&count.to_le_bytes());
            }
            Node::CreateGrid {
                rows,
                cols,
                size_x,
                size_z,
                ..
            } => {
                hasher.update(b"grid");
                hasher.update(&rows.to_le_bytes());
                hasher.update(&cols.to_le_bytes());
                for c in [*size_x, *size_z] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
            }
            Node::CreateBox { size, .. } => {
                hasher.update(b"box");
                for c in [size.x, size.y, size.z] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
            }
            Node::Transform { offset, .. } => {
                hasher.update(b"transform");
                for c in [offset.x, offset.y, offset.z] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
            }
            Node::Normal { .. } => {
                hasher.update(b"normal");
            }
            Node::PointWrangle { snippet, .. } => {
                hasher.update(b"wrangle");
                hasher.update(&(snippet.len() as u64).to_le_bytes());
                hasher.update(snippet.as_bytes());
            }
            Node::Scatter {
                id, count, seed, ..
            } => {
                hasher.update(b"scatter");
                hasher.update(&id.0.to_le_bytes());
                hasher.update(&count.to_le_bytes());
                hasher.update(&seed.to_le_bytes());
            }
            Node::CopyToPoints { .. } => {
                hasher.update(b"copytopoints");
            }
            Node::Merge { .. } => {
                hasher.update(b"merge");
            }
            Node::Subdivide { iterations, .. } => {
                hasher.update(b"subdivide");
                hasher.update(&iterations.to_le_bytes());
            }
            Node::PolyExtrude {
                distance, inset, ..
            } => {
                hasher.update(b"extrude");
                for c in [*distance, *inset] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
            }
            Node::AttribPromote {
                name,
                from_domain,
                to_domain,
                mode,
                ..
            } => {
                hasher.update(b"attribpromote");
                hasher.update(&(name.len() as u64).to_le_bytes());
                hasher.update(name.as_bytes());
                hasher.update(from_domain.as_str().as_bytes());
                hasher.update(b"->");
                hasher.update(to_domain.as_str().as_bytes());
                hasher.update(mode.as_str().as_bytes());
            }
            Node::Blast {
                group_name, negate, ..
            } => {
                hasher.update(b"blast");
                hasher.update(&(group_name.len() as u64).to_le_bytes());
                hasher.update(group_name.as_bytes());
                hasher.update(&[u8::from(*negate)]);
            }
            Node::UvProject { mode, .. } => {
                hasher.update(b"uvproject");
                hasher.update(mode.as_str().as_bytes());
            }
            Node::HeightField {
                rows,
                cols,
                size_x,
                size_z,
                ..
            } => {
                hasher.update(b"heightfield");
                hasher.update(&rows.to_le_bytes());
                hasher.update(&cols.to_le_bytes());
                for c in [*size_x, *size_z] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
            }
            Node::HeightFieldNoise {
                amplitude,
                frequency,
                octaves,
                seed,
                ..
            } => {
                hasher.update(b"hf_noise");
                for c in [*amplitude, *frequency] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
                hasher.update(&octaves.to_le_bytes());
                hasher.update(&seed.to_le_bytes());
            }
            Node::HeightFieldErode {
                iterations,
                rain_rate,
                talus_angle,
                seed,
                ..
            } => {
                hasher.update(b"hf_erode");
                hasher.update(&iterations.to_le_bytes());
                for c in [*rain_rate, *talus_angle] {
                    let canonical = if c == 0.0 { 0.0 } else { c };
                    hasher.update(&canonical.to_bits().to_le_bytes());
                }
                hasher.update(&seed.to_le_bytes());
            }
            Node::VdbFromPolygons { voxel_size, .. } => {
                hasher.update(b"vdb_from_polygons");
                let canonical = if *voxel_size == 0.0 { 0.0 } else { *voxel_size };
                hasher.update(&canonical.to_bits().to_le_bytes());
            }
            Node::VdbReshape {
                offset,
                smooth_iterations,
                ..
            } => {
                hasher.update(b"vdb_reshape");
                let canonical = if *offset == 0.0 { 0.0 } else { *offset };
                hasher.update(&canonical.to_bits().to_le_bytes());
                hasher.update(&smooth_iterations.to_le_bytes());
            }
            Node::ConvertVdb { .. } => {
                hasher.update(b"convert_vdb");
            }
            Node::Boolean {
                mode, voxel_size, ..
            } => {
                hasher.update(b"boolean");
                hasher.update(mode.as_str().as_bytes());
                let canonical = if *voxel_size == 0.0 { 0.0 } else { *voxel_size };
                hasher.update(&canonical.to_bits().to_le_bytes());
            }
            Node::CreateSphere {
                radius, rows, cols, ..
            } => {
                hasher.update(b"sphere");
                hash_f32s(&mut hasher, &[*radius]);
                hasher.update(&rows.to_le_bytes());
                hasher.update(&cols.to_le_bytes());
            }
            Node::CreateTube {
                radius,
                height,
                rows,
                cols,
                ..
            } => {
                hasher.update(b"tube");
                hash_f32s(&mut hasher, &[*radius]);
                hash_f32s(&mut hasher, &[*height]);
                hasher.update(&rows.to_le_bytes());
                hasher.update(&cols.to_le_bytes());
            }
            Node::CreateTorus {
                major_radius,
                minor_radius,
                rows,
                cols,
                ..
            } => {
                hasher.update(b"torus");
                hash_f32s(&mut hasher, &[*major_radius]);
                hash_f32s(&mut hasher, &[*minor_radius]);
                hasher.update(&rows.to_le_bytes());
                hasher.update(&cols.to_le_bytes());
            }
            Node::CreateLine {
                start, end, points, ..
            } => {
                hasher.update(b"line");
                hash_f32s(&mut hasher, &[start.x, start.y, start.z]);
                hash_f32s(&mut hasher, &[end.x, end.y, end.z]);
                hasher.update(&points.to_le_bytes());
            }
            Node::Xform {
                translate,
                rotate_degrees,
                scale,
                ..
            } => {
                hasher.update(b"xform");
                hash_f32s(&mut hasher, &[translate.x, translate.y, translate.z]);
                hash_f32s(
                    &mut hasher,
                    &[rotate_degrees.x, rotate_degrees.y, rotate_degrees.z],
                );
                hash_f32s(&mut hasher, &[scale.x, scale.y, scale.z]);
            }
            Node::Reverse { .. } => {
                hasher.update(b"reverse");
            }
            Node::Triangulate { .. } => {
                hasher.update(b"triangulate");
            }
            Node::Fuse { distance, .. } => {
                hasher.update(b"fuse");
                hash_f32s(&mut hasher, &[*distance]);
            }
            Node::GroupBounds {
                group_name,
                domain,
                min,
                max,
                ..
            } => {
                hasher.update(b"group_bounds");
                hasher.update(&(group_name.len() as u64).to_le_bytes());
                hasher.update(group_name.as_bytes());
                hasher.update(domain.as_str().as_bytes());
                hash_f32s(&mut hasher, &[min.x, min.y, min.z]);
                hash_f32s(&mut hasher, &[max.x, max.y, max.z]);
            }
            Node::Resample { segment_length, .. } => {
                hasher.update(b"resample");
                hash_f32s(&mut hasher, &[*segment_length]);
            }
            Node::Sweep {
                radius,
                sides,
                capped,
                ..
            } => {
                hasher.update(b"sweep");
                hash_f32s(&mut hasher, &[*radius]);
                hasher.update(&sides.to_le_bytes());
                hasher.update(&[u8::from(*capped)]);
            }
            Node::AttribTransfer { name, radius, .. } => {
                hasher.update(b"attribtransfer");
                hasher.update(&(name.len() as u64).to_le_bytes());
                hasher.update(name.as_bytes());
                hash_f32s(&mut hasher, &[*radius]);
            }
            Node::Measure { .. } => {
                hasher.update(b"measure");
            }
            Node::PolyBevel {
                distance,
                min_angle_degrees,
                ..
            } => {
                hasher.update(b"polybevel");
                hash_f32s(&mut hasher, &[*distance]);
                hash_f32s(&mut hasher, &[*min_angle_degrees]);
            }
            Node::PolyReduce { target_ratio, .. } => {
                hasher.update(b"polyreduce");
                hash_f32s(&mut hasher, &[*target_ratio]);
            }
            Node::Remesh {
                target_length,
                iterations,
                feature_angle_degrees,
                ..
            } => {
                hasher.update(b"remesh");
                hash_f32s(&mut hasher, &[*target_length]);
                hasher.update(&iterations.to_le_bytes());
                hash_f32s(&mut hasher, &[*feature_angle_degrees]);
            }
            Node::Clip {
                origin,
                normal,
                cap,
                ..
            } => {
                hasher.update(b"clip");
                hash_f32s(&mut hasher, &[origin.x, origin.y, origin.z]);
                hash_f32s(&mut hasher, &[normal.x, normal.y, normal.z]);
                hasher.update(&[u8::from(*cap)]);
            }
            Node::VoronoiFracture { .. } => {
                hasher.update(b"voronoifracture");
            }
            Node::ScatterSurface {
                count,
                seed,
                min_distance,
                ..
            } => {
                hasher.update(b"scatter_surface");
                hasher.update(&count.to_le_bytes());
                hasher.update(&seed.to_le_bytes());
                hash_f32s(&mut hasher, &[*min_distance]);
            }
            Node::UvFlatten {
                seam_angle_degrees, ..
            } => {
                hasher.update(b"uvflatten");
                hash_f32s(&mut hasher, &[*seam_angle_degrees]);
            }
            Node::UvLayout { padding, tiles, .. } => {
                hasher.update(b"uvlayout");
                hash_f32s(&mut hasher, &[*padding]);
                hasher.update(&tiles.to_le_bytes());
            }
        }
        hasher.update(&(upstream_hashes.len() as u64).to_le_bytes());
        for upstream in upstream_hashes {
            hasher.update(upstream.as_bytes());
        }
        hasher.update(&frame_numerator.to_le_bytes());
        hasher.update(&frame_denominator.to_le_bytes());
        NodeCacheKey(*hasher.finalize().as_bytes())
    }
    pub fn note_published_revision(&self, revision: RevisionId) {
        if let Ok(mut latest) = self.latest_revision.lock()
            && revision > *latest
        {
            *latest = revision;
        }
    }
    pub fn get(&self, key: &CacheKey) -> Option<Arc<GeoDetail>> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(key).map(|entry| Arc::clone(&entry.geo)))
    }
    pub fn get_node(&self, key: &NodeCacheKey) -> Option<Arc<GeoDetail>> {
        let found = self
            .node_entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(key).map(|entry| Arc::clone(&entry.geo)));
        if found.is_some() {
            self.node_hits.fetch_add(1, Ordering::Relaxed);
        }
        found
    }
    #[must_use]
    pub fn node_cache_hits(&self) -> u64 {
        self.node_hits.load(Ordering::Relaxed)
    }
    /// Inserts a cooked result only if `revision` is not stale relative to any
    /// published revision or any existing entry for `key`.
    pub fn insert_if_fresh(
        &self,
        key: CacheKey,
        revision: RevisionId,
        value: Arc<GeoDetail>,
    ) -> bool {
        let Ok(mut latest) = self.latest_revision.lock() else {
            return false;
        };
        if revision < *latest {
            return false;
        }
        let Ok(mut entries) = self.entries.lock() else {
            return false;
        };
        if entries
            .get(&key)
            .is_some_and(|existing| existing.revision > revision)
        {
            return false;
        }
        *latest = (*latest).max(revision);
        entries.insert(
            key,
            CacheEntry {
                revision,
                geo: value,
            },
        );
        true
    }
    pub fn insert_node_if_fresh(
        &self,
        key: NodeCacheKey,
        revision: RevisionId,
        value: Arc<GeoDetail>,
    ) -> bool {
        let Ok(latest) = self.latest_revision.lock() else {
            return false;
        };
        if revision < *latest {
            return false;
        }
        let Ok(mut entries) = self.node_entries.lock() else {
            return false;
        };
        if entries
            .get(&key)
            .is_some_and(|existing| existing.revision > revision)
        {
            return false;
        }
        entries.insert(
            key,
            CacheEntry {
                revision,
                geo: value,
            },
        );
        true
    }
    pub fn insert(&self, key: CacheKey, value: Arc<GeoDetail>) {
        let _ = self.insert_if_fresh(key, RevisionId(0), value);
    }
}
pub fn cook(snapshot: &GraphSnapshot, ctx: &EvalContext<'_>) -> Result<GeoDetail, EvalError> {
    cook_with_progress(snapshot, ctx, |_, _| {})
}
pub fn cook_with_progress<F>(
    snapshot: &GraphSnapshot,
    ctx: &EvalContext<'_>,
    mut on_progress: F,
) -> Result<GeoDetail, EvalError>
where
    F: FnMut(u32, u32),
{
    let arc_geo = eval_dag(snapshot, None, None, 0, 1, ctx, &mut on_progress)?;
    Ok(Arc::try_unwrap(arc_geo).unwrap_or_else(|shared| (*shared).clone()))
}

/// Hashes floats with negative zero canonicalized, matching the `.ofxs` text form.
fn hash_f32s(hasher: &mut blake3::Hasher, values: &[f32]) {
    for &value in values {
        let canonical = if value == 0.0 { 0.0 } else { value };
        hasher.update(&canonical.to_bits().to_le_bytes());
    }
}

/// Rejects a generator's requested size before any allocation.
fn check_element_estimate(requested: u32, ctx: &EvalContext<'_>) -> Result<(), EvalError> {
    if requested > ctx.budget.max_elements {
        return Err(EvalError::ElementLimit {
            requested,
            limit: ctx.budget.max_elements,
        });
    }
    Ok(())
}

/// Maps operator errors onto evaluator errors and enforces the element budget on the output.
fn finish_sop(
    result: Result<GeoDetail, GeoError>,
    ctx: &EvalContext<'_>,
) -> Result<GeoDetail, EvalError> {
    let geo = result.map_err(|error| match error {
        GeoError::Cancelled => EvalError::Cancelled,
        other => EvalError::Resource(other),
    })?;
    let elements = u32::try_from(geo.len()).unwrap_or(u32::MAX);
    check_element_estimate(elements, ctx)?;
    Ok(geo)
}

fn eval_single_node(
    node: &Node,
    outputs: &BTreeMap<NodeId, Arc<GeoDetail>>,
    frame_numerator: i64,
    frame_denominator: u32,
    ctx: &EvalContext<'_>,
) -> Result<GeoDetail, EvalError> {
    if ctx.cancelled.load(Ordering::Acquire) {
        return Err(EvalError::Cancelled);
    }
    match node {
        Node::CreatePoints { count, .. } => {
            if *count > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: *count,
                    limit: ctx.budget.max_elements,
                });
            }
            let bytes = (*count as usize)
                .checked_mul(std::mem::size_of::<Vec3>())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if bytes > ctx.budget.max_memory_bytes {
                return Err(EvalError::Resource(GeoError::LimitExceeded));
            }
            let mut points = Vec::with_capacity(*count as usize);
            for i in 0..*count {
                if (i as usize).is_multiple_of(PAGE_LEN) && ctx.cancelled.load(Ordering::Acquire) {
                    return Err(EvalError::Cancelled);
                }
                let pt = Vec3::checked(i as f32, 0., 0.).map_err(|_| EvalError::InvalidGraph)?;
                points.push(pt);
            }
            Ok(GeoDetail::points(points))
        }
        Node::CreateGrid {
            rows,
            cols,
            size_x,
            size_z,
            ..
        } => {
            if *rows < 2 || *cols < 2 || !size_x.is_finite() || !size_z.is_finite() {
                return Err(EvalError::InvalidGraph);
            }
            let point_count = rows
                .checked_mul(*cols)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if point_count > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: point_count,
                    limit: ctx.budget.max_elements,
                });
            }
            let quad_rows = (*rows - 1) as usize;
            let quad_cols = (*cols - 1) as usize;
            let quad_count = quad_rows
                .checked_mul(quad_cols)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let vtx_count = quad_count
                .checked_mul(4)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let pt_bytes = (point_count as usize)
                .checked_mul(std::mem::size_of::<Vec3>())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let vtx_bytes = vtx_count
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let total_bytes = pt_bytes
                .checked_add(vtx_bytes)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if total_bytes > ctx.budget.max_memory_bytes {
                return Err(EvalError::Resource(GeoError::LimitExceeded));
            }

            let mut points = Vec::with_capacity(point_count as usize);
            let denom_r = (*rows - 1) as f32;
            let denom_c = (*cols - 1) as f32;
            for r in 0..*rows {
                for c in 0..*cols {
                    let idx = points.len();
                    if idx.is_multiple_of(PAGE_LEN) && ctx.cancelled.load(Ordering::Acquire) {
                        return Err(EvalError::Cancelled);
                    }
                    let x = (c as f32 / denom_c) * *size_x;
                    let z = (r as f32 / denom_r) * *size_z;
                    let pt = Vec3::checked(x, 0.0, z).map_err(|_| EvalError::InvalidGraph)?;
                    points.push(pt);
                }
            }
            let mut vertex_points = Vec::with_capacity(vtx_count);
            for r in 0..(*rows - 1) {
                for c in 0..(*cols - 1) {
                    let i0 = r * *cols + c;
                    let i1 = i0 + 1;
                    let i2 = i0 + *cols + 1;
                    let i3 = i0 + *cols;
                    vertex_points.extend_from_slice(&[i0, i1, i2, i3]);
                }
            }
            GeoDetail::points(points)
                .with_primitives(
                    vec![PrimRun {
                        kind: PrimKind::Polygon,
                        arity: 4,
                        prim_count: quad_count,
                    }],
                    vertex_points,
                )
                .map_err(EvalError::Resource)
        }
        Node::CreateBox { size, .. } => {
            if size.x <= 0.0 || size.y <= 0.0 || size.z <= 0.0 {
                return Err(EvalError::InvalidGraph);
            }
            if 8 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: 8,
                    limit: ctx.budget.max_elements,
                });
            }
            let total_bytes = 8 * std::mem::size_of::<Vec3>() + 24 * std::mem::size_of::<u32>();
            if total_bytes > ctx.budget.max_memory_bytes {
                return Err(EvalError::Resource(GeoError::LimitExceeded));
            }
            let sx = size.x;
            let sy = size.y;
            let sz = size.z;
            let points = vec![
                Vec3::checked(0.0, 0.0, 0.0).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(sx, 0.0, 0.0).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(sx, sy, 0.0).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(0.0, sy, 0.0).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(0.0, 0.0, sz).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(sx, 0.0, sz).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(sx, sy, sz).map_err(|_| EvalError::InvalidGraph)?,
                Vec3::checked(0.0, sy, sz).map_err(|_| EvalError::InvalidGraph)?,
            ];
            let vertex_points = vec![
                0, 3, 2, 1, // -Z
                4, 5, 6, 7, // +Z
                0, 1, 5, 4, // -Y
                3, 7, 6, 2, // +Y
                0, 4, 7, 3, // -X
                1, 2, 6, 5, // +X
            ];
            GeoDetail::points(points)
                .with_primitives(
                    vec![PrimRun {
                        kind: PrimKind::Polygon,
                        arity: 4,
                        prim_count: 6,
                    }],
                    vertex_points,
                )
                .map_err(EvalError::Resource)
        }
        Node::Transform { id, input, offset } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            prior_geo
                .translated_cancellable(*offset, ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::Normal { id, input } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            prior_geo
                .compute_normals_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::PointWrangle { id, input, snippet } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let prog = compile_vex(snippet).map_err(EvalError::Vex)?;
            let frame = (frame_numerator as f32) / (frame_denominator.max(1) as f32);
            prog.execute(
                prior_geo,
                frame,
                DEFAULT_MAX_FUEL_PER_POINT,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                VexError::Geo(GeoError::Cancelled) => EvalError::Cancelled,
                VexError::Geo(other) => EvalError::Resource(other),
                other => EvalError::Vex(other),
            })
        }
        Node::Scatter {
            id,
            input,
            count,
            seed,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            if *count > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: *count,
                    limit: ctx.budget.max_elements,
                });
            }
            let bytes = (*count as usize)
                .checked_mul(std::mem::size_of::<Vec3>())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if bytes > ctx.budget.max_memory_bytes {
                return Err(EvalError::Resource(GeoError::LimitExceeded));
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let (lo, hi) = prior_geo.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
            let span_x = hi.x - lo.x;
            let span_y = hi.y - lo.y;
            let span_z = hi.z - lo.z;
            let rng = Philox4x32::new(*seed);
            let mut points = Vec::with_capacity(*count as usize);
            for i in 0..*count {
                if (i as usize).is_multiple_of(PAGE_LEN) && ctx.cancelled.load(Ordering::Acquire) {
                    return Err(EvalError::Cancelled);
                }
                let u = rng.generate_unit_f32([id.0 as u32, i, 0, 0]);
                let pt = Vec3::checked(
                    lo.x + u[0] * span_x,
                    lo.y + u[1] * span_y,
                    lo.z + u[2] * span_z,
                )
                .map_err(|_| EvalError::InvalidGraph)?;
                points.push(pt);
            }
            Ok(GeoDetail::points(points))
        }
        Node::CopyToPoints { id, source, target } => {
            if *id == *source || *id == *target {
                return Err(EvalError::InvalidGraph);
            }
            let src_geo = outputs.get(source).ok_or(EvalError::InvalidGraph)?;
            let tgt_geo = outputs.get(target).ok_or(EvalError::InvalidGraph)?;
            let src_pts = src_geo.len();
            let tgt_pts = tgt_geo.len();
            let total_pts = src_pts
                .checked_mul(tgt_pts)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let total_pts_u32 = u32::try_from(total_pts)
                .map_err(|_| EvalError::Resource(GeoError::LimitExceeded))?;
            if total_pts_u32 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: total_pts_u32,
                    limit: ctx.budget.max_elements,
                });
            }
            let total_vtx = src_geo
                .vertex_count()
                .checked_mul(tgt_pts)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let pt_bytes = total_pts
                .checked_mul(std::mem::size_of::<Vec3>())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            let vtx_bytes = total_vtx
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if pt_bytes.saturating_add(vtx_bytes) > ctx.budget.max_memory_bytes {
                return Err(EvalError::Resource(GeoError::LimitExceeded));
            }
            if total_pts == 0 {
                return Ok(GeoDetail::points(Vec::new()));
            }
            let pscale_col = tgt_geo
                .attribute("pscale")
                .and_then(AttributeColumn::as_f32);
            let mut points = Vec::with_capacity(total_pts);
            let mut runs: Vec<PrimRun> = Vec::new();
            let mut vertex_points = Vec::with_capacity(total_vtx);

            for t_idx in 0..tgt_pts {
                if t_idx.is_multiple_of(PAGE_LEN) && ctx.cancelled.load(Ordering::Acquire) {
                    return Err(EvalError::Cancelled);
                }
                let t_pos = tgt_geo.point(t_idx).unwrap_or(Vec3::ZERO);
                let scale = pscale_col
                    .and_then(|c| c.get(t_idx))
                    .map(|f| f.get())
                    .unwrap_or(1.0);
                let base_pt_offset = (t_idx * src_pts) as u32;
                for s_idx in 0..src_pts {
                    let s_pos = src_geo.point(s_idx).unwrap_or(Vec3::ZERO);
                    let scaled = s_pos
                        .scale_checked(scale)
                        .and_then(|p| p.add_checked(t_pos))
                        .map_err(|_| EvalError::Resource(GeoError::NonFinite))?;
                    points.push(scaled);
                }
                for run in src_geo.prim_runs() {
                    if let Some(last) = runs.last_mut()
                        && last.kind == run.kind
                        && last.arity == run.arity
                    {
                        last.prim_count = last
                            .prim_count
                            .checked_add(run.prim_count)
                            .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
                    } else {
                        runs.push(*run);
                    }
                }
                for v_idx in 0..src_geo.vertex_count() {
                    if let Some(pt) = src_geo.vertex_point(v_idx) {
                        vertex_points.push(pt + base_pt_offset);
                    }
                }
            }
            let base = GeoDetail::points(points);
            if runs.is_empty() {
                Ok(base)
            } else {
                base.with_primitives(runs, vertex_points)
                    .map_err(EvalError::Resource)
            }
        }
        Node::Merge { id, left, right } => {
            if *id == *left || *id == *right {
                return Err(EvalError::InvalidGraph);
            }
            let left_geo = outputs.get(left).ok_or(EvalError::InvalidGraph)?;
            let right_geo = outputs.get(right).ok_or(EvalError::InvalidGraph)?;
            let total_elements = left_geo
                .len()
                .checked_add(right_geo.len())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if total_elements > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: total_elements,
                    limit: ctx.budget.max_elements,
                });
            }
            left_geo
                .merged_cancellable(right_geo, ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::Subdivide {
            id,
            input,
            iterations,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let out = prior_geo
                .subdivide_cancellable(*iterations, ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            let pts_u32 = u32::try_from(out.len())
                .map_err(|_| EvalError::Resource(GeoError::LimitExceeded))?;
            if pts_u32 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: pts_u32,
                    limit: ctx.budget.max_elements,
                });
            }
            Ok(out)
        }
        Node::PolyExtrude {
            id,
            input,
            distance,
            inset,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let out = prior_geo
                .poly_extrude_cancellable(
                    *distance,
                    *inset,
                    ctx.budget.max_memory_bytes,
                    ctx.cancelled,
                )
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            let pts_u32 = u32::try_from(out.len())
                .map_err(|_| EvalError::Resource(GeoError::LimitExceeded))?;
            if pts_u32 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: pts_u32,
                    limit: ctx.budget.max_elements,
                });
            }
            Ok(out)
        }
        Node::AttribPromote {
            id,
            input,
            name,
            from_domain,
            to_domain,
            mode,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let map_domain = |d: AttrDomain| match d {
                AttrDomain::Point => GeoAttrDomain::Point,
                AttrDomain::Prim => GeoAttrDomain::Prim,
                AttrDomain::Detail => GeoAttrDomain::Detail,
            };
            let map_mode = |m: PromoteMode| match m {
                PromoteMode::Average => GeoPromoteMode::Average,
                PromoteMode::Min => GeoPromoteMode::Min,
                PromoteMode::Max => GeoPromoteMode::Max,
                PromoteMode::Sum => GeoPromoteMode::Sum,
            };
            prior_geo
                .promote_attribute(
                    name,
                    map_domain(*from_domain),
                    map_domain(*to_domain),
                    map_mode(*mode),
                )
                .map_err(EvalError::Resource)
        }
        Node::Blast {
            id,
            input,
            group_name,
            negate,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            prior_geo
                .blast_group_cancellable(
                    group_name,
                    *negate,
                    ctx.budget.max_memory_bytes,
                    ctx.cancelled,
                )
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::UvProject { id, input, mode } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let geo_mode = match mode {
                UvProjectMode::PlanarY => GeoUvProjectMode::PlanarY,
                UvProjectMode::PlanarZ => GeoUvProjectMode::PlanarZ,
                UvProjectMode::PlanarX => GeoUvProjectMode::PlanarX,
                UvProjectMode::Cylindrical => GeoUvProjectMode::Cylindrical,
                UvProjectMode::Spherical => GeoUvProjectMode::Spherical,
                UvProjectMode::Box => GeoUvProjectMode::Box,
            };
            prior_geo
                .uv_project_cancellable(geo_mode, ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::HeightField {
            rows,
            cols,
            size_x,
            size_z,
            ..
        } => {
            if *rows < 2 || *cols < 2 || !size_x.is_finite() || !size_z.is_finite() {
                return Err(EvalError::InvalidGraph);
            }
            let point_count = rows
                .checked_mul(*cols)
                .ok_or(EvalError::Resource(GeoError::LimitExceeded))?;
            if point_count > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: point_count,
                    limit: ctx.budget.max_elements,
                });
            }
            let hf = HeightField::flat(*rows as usize, *cols as usize, *size_x, *size_z, 0.0)
                .map_err(EvalError::Resource)?;
            hf.to_geo_detail_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::HeightFieldNoise {
            id,
            input,
            amplitude,
            frequency,
            octaves,
            seed,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let hf = HeightField::from_geo_detail_cancellable(
                prior_geo,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                GeoError::Cancelled => EvalError::Cancelled,
                other => EvalError::Resource(other),
            })?;
            let params = HfNoiseParams {
                amplitude: *amplitude,
                frequency: *frequency,
                octaves: *octaves,
                seed: *seed,
                ..HfNoiseParams::default()
            };
            let noisy = hf
                .apply_fbm_noise_cancellable(&params, ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            noisy
                .to_geo_detail_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::HeightFieldErode {
            id,
            input,
            iterations,
            rain_rate,
            talus_angle,
            seed,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let hf = HeightField::from_geo_detail_cancellable(
                prior_geo,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                GeoError::Cancelled => EvalError::Cancelled,
                other => EvalError::Resource(other),
            })?;
            let params = HfErodeParams {
                iterations: *iterations,
                rain_rate: *rain_rate,
                talus_angle_deg: *talus_angle,
                seed: *seed,
                ..HfErodeParams::default()
            };
            let eroded = hf
                .erode_cancellable(&params, ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            eroded
                .to_geo_detail_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::VdbFromPolygons {
            id,
            input,
            voxel_size,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let vdb = VdbGrid::from_polygons_cancellable(
                prior_geo,
                *voxel_size,
                3,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                GeoError::Cancelled => EvalError::Cancelled,
                other => EvalError::Resource(other),
            })?;
            let out = vdb
                .to_voxel_geo_detail_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            let pts_u32 = u32::try_from(out.len())
                .map_err(|_| EvalError::Resource(GeoError::LimitExceeded))?;
            if pts_u32 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: pts_u32,
                    limit: ctx.budget.max_elements,
                });
            }
            Ok(out)
        }
        Node::VdbReshape {
            id,
            input,
            offset,
            smooth_iterations,
        } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let vdb = VdbGrid::from_voxel_geo_detail_cancellable(
                prior_geo,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                GeoError::Cancelled => EvalError::Cancelled,
                other => EvalError::Resource(other),
            })?;
            let reshaped = vdb
                .reshape_cancellable(
                    *offset,
                    *smooth_iterations,
                    ctx.budget.max_memory_bytes,
                    ctx.cancelled,
                )
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            reshaped
                .to_voxel_geo_detail_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })
        }
        Node::ConvertVdb { id, input } => {
            if *id == *input {
                return Err(EvalError::InvalidGraph);
            }
            let prior_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let vdb = VdbGrid::from_voxel_geo_detail_cancellable(
                prior_geo,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                GeoError::Cancelled => EvalError::Cancelled,
                other => EvalError::Resource(other),
            })?;
            let out = vdb
                .to_polygons_cancellable(ctx.budget.max_memory_bytes, ctx.cancelled)
                .map_err(|e| match e {
                    GeoError::Cancelled => EvalError::Cancelled,
                    other => EvalError::Resource(other),
                })?;
            let pts_u32 = u32::try_from(out.len())
                .map_err(|_| EvalError::Resource(GeoError::LimitExceeded))?;
            if pts_u32 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: pts_u32,
                    limit: ctx.budget.max_elements,
                });
            }
            Ok(out)
        }
        Node::Boolean {
            id,
            left,
            right,
            mode,
            voxel_size,
        } => {
            if *id == *left || *id == *right {
                return Err(EvalError::InvalidGraph);
            }
            let left_geo = outputs.get(left).ok_or(EvalError::InvalidGraph)?;
            let right_geo = outputs.get(right).ok_or(EvalError::InvalidGraph)?;
            let op = match mode {
                BooleanMode::Union => VdbCombineOp::Union,
                BooleanMode::Intersect => VdbCombineOp::Intersection,
                BooleanMode::Subtract => VdbCombineOp::Difference,
            };
            let out = boolean_meshes_cancellable(
                left_geo,
                right_geo,
                op,
                *voxel_size,
                ctx.budget.max_memory_bytes,
                ctx.cancelled,
            )
            .map_err(|e| match e {
                GeoError::Cancelled => EvalError::Cancelled,
                other => EvalError::Resource(other),
            })?;
            let pts_u32 = u32::try_from(out.len())
                .map_err(|_| EvalError::Resource(GeoError::LimitExceeded))?;
            if pts_u32 > ctx.budget.max_elements {
                return Err(EvalError::ElementLimit {
                    requested: pts_u32,
                    limit: ctx.budget.max_elements,
                });
            }
            Ok(out)
        }
        Node::CreateSphere {
            radius, rows, cols, ..
        } => {
            check_element_estimate(rows.saturating_mul(*cols), ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(create_sphere(f64::from(*radius), *rows, *cols, &sop), ctx)
        }
        Node::CreateTube {
            radius,
            height,
            rows,
            cols,
            ..
        } => {
            check_element_estimate(rows.saturating_add(1).saturating_mul(*cols), ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                create_tube(f64::from(*radius), f64::from(*height), *rows, *cols, &sop),
                ctx,
            )
        }
        Node::CreateTorus {
            major_radius,
            minor_radius,
            rows,
            cols,
            ..
        } => {
            check_element_estimate(rows.saturating_mul(*cols), ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                create_torus(
                    f64::from(*major_radius),
                    f64::from(*minor_radius),
                    *rows,
                    *cols,
                    &sop,
                ),
                ctx,
            )
        }
        Node::CreateLine {
            start, end, points, ..
        } => {
            check_element_estimate(*points, ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(create_line(*start, *end, *points, &sop), ctx)
        }
        Node::Xform {
            input,
            translate,
            rotate_degrees,
            scale,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                transform_trs(
                    input_geo,
                    &Trs {
                        translate: *translate,
                        rotate_degrees: *rotate_degrees,
                        scale: *scale,
                    },
                    &sop,
                ),
                ctx,
            )
        }
        Node::Reverse { input, .. } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(reverse_winding(input_geo, &sop), ctx)
        }
        Node::Triangulate { input, .. } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(triangulate(input_geo, &sop), ctx)
        }
        Node::Fuse {
            input, distance, ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(fuse_points(input_geo, f64::from(*distance), &sop), ctx)
        }
        Node::Resample {
            input,
            segment_length,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                resample_curves(input_geo, f64::from(*segment_length), &sop),
                ctx,
            )
        }
        Node::Sweep {
            input,
            radius,
            sides,
            capped,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                sweep_curves(input_geo, f64::from(*radius), *sides, *capped, &sop),
                ctx,
            )
        }
        Node::AttribTransfer {
            input,
            source,
            name,
            radius,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let source_geo = outputs.get(source).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                attrib_transfer(input_geo, source_geo, name, f64::from(*radius), &sop),
                ctx,
            )
        }
        Node::Measure { input, .. } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(measure(input_geo, &sop), ctx)
        }
        Node::PolyBevel {
            input,
            distance,
            min_angle_degrees,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                poly_bevel(
                    input_geo,
                    f64::from(*distance),
                    f64::from(*min_angle_degrees),
                    &sop,
                ),
                ctx,
            )
        }
        Node::PolyReduce {
            input,
            target_ratio,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(poly_reduce(input_geo, f64::from(*target_ratio), &sop), ctx)
        }
        Node::Remesh {
            input,
            target_length,
            iterations,
            feature_angle_degrees,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                remesh(
                    input_geo,
                    &RemeshParams {
                        target_length: f64::from(*target_length),
                        iterations: *iterations,
                        feature_angle_degrees: f64::from(*feature_angle_degrees),
                    },
                    &sop,
                ),
                ctx,
            )
        }
        Node::Clip {
            input,
            origin,
            normal,
            cap,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                clip_halfspace(
                    input_geo,
                    [
                        f64::from(origin.x),
                        f64::from(origin.y),
                        f64::from(origin.z),
                    ],
                    [
                        f64::from(normal.x),
                        f64::from(normal.y),
                        f64::from(normal.z),
                    ],
                    *cap,
                    &sop,
                ),
                ctx,
            )
        }
        Node::VoronoiFracture { input, seeds, .. } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let seeds_geo = outputs.get(seeds).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(voronoi_fracture(input_geo, seeds_geo, &sop), ctx)
        }
        Node::ScatterSurface {
            input,
            count,
            seed,
            min_distance,
            ..
        } => {
            check_element_estimate(*count, ctx)?;
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                scatter_surface(
                    input_geo,
                    &SurfaceScatter {
                        count: *count,
                        seed: *seed,
                        min_distance: f64::from(*min_distance),
                    },
                    &sop,
                ),
                ctx,
            )
        }
        Node::UvFlatten {
            input,
            seam_angle_degrees,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                uv_flatten(input_geo, f64::from(*seam_angle_degrees), &sop),
                ctx,
            )
        }
        Node::UvLayout {
            input,
            padding,
            tiles,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(uv_layout(input_geo, f64::from(*padding), *tiles, &sop), ctx)
        }
        Node::GroupBounds {
            input,
            group_name,
            domain,
            min,
            max,
            ..
        } => {
            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let group_domain = match domain {
                AttrDomain::Point => GroupDomain::Point,
                AttrDomain::Prim => GroupDomain::Prim,
                AttrDomain::Detail => return Err(EvalError::InvalidGraph),
            };
            finish_sop(
                input_geo.group_by_bounds(group_domain, group_name.as_str(), *min, *max),
                ctx,
            )
        }
    }
}

fn eval_dag<F>(
    snapshot: &GraphSnapshot,
    cache: Option<&CookCache>,
    target_node: Option<NodeId>,
    frame_numerator: i64,
    frame_denominator: u32,
    ctx: &EvalContext<'_>,
    on_progress: &mut F,
) -> Result<Arc<GeoDetail>, EvalError>
where
    F: FnMut(u32, u32),
{
    snapshot.validate().map_err(|_| EvalError::InvalidGraph)?;
    if let Some(target) = target_node
        && !snapshot.nodes.iter().any(|n| n.id() == target)
    {
        return Err(EvalError::NodeNotFound(target));
    }
    let total_steps = snapshot.nodes.len() as u32;
    on_progress(0, total_steps);
    let mut outputs: BTreeMap<NodeId, Arc<GeoDetail>> = BTreeMap::new();
    let mut last_output: Option<Arc<GeoDetail>> = None;

    for (step_idx, node) in snapshot.nodes.iter().enumerate() {
        if ctx.cancelled.load(Ordering::Acquire) {
            return Err(EvalError::Cancelled);
        }
        let node_geo = if let Some(cook_cache) = cache {
            let mut upstream_hashes = Vec::new();
            for input_id in node.inputs() {
                let upstream_geo = outputs.get(&input_id).ok_or(EvalError::InvalidGraph)?;
                upstream_hashes.push(upstream_geo.hash());
            }
            let nkey =
                CookCache::node_key(node, &upstream_hashes, frame_numerator, frame_denominator);
            if let Some(cached) = cook_cache.get_node(&nkey) {
                cached
            } else {
                let evaluated = Arc::new(eval_single_node(
                    node,
                    &outputs,
                    frame_numerator,
                    frame_denominator,
                    ctx,
                )?);
                let _ = cook_cache.insert_node_if_fresh(
                    nkey,
                    snapshot.revision,
                    Arc::clone(&evaluated),
                );
                evaluated
            }
        } else {
            Arc::new(eval_single_node(
                node,
                &outputs,
                frame_numerator,
                frame_denominator,
                ctx,
            )?)
        };
        let nid = node.id();
        outputs.insert(nid, Arc::clone(&node_geo));
        last_output = Some(Arc::clone(&node_geo));
        on_progress((step_idx + 1) as u32, total_steps);
        if target_node == Some(nid) {
            return Ok(node_geo);
        }
    }
    last_output.ok_or(EvalError::InvalidGraph)
}

pub fn cook_node_cached(
    cache: &CookCache,
    snapshot: &GraphSnapshot,
    target_node: NodeId,
    frame_numerator: i64,
    frame_denominator: u32,
    ctx: &EvalContext<'_>,
) -> Result<Arc<GeoDetail>, EvalError> {
    eval_dag(
        snapshot,
        Some(cache),
        Some(target_node),
        frame_numerator,
        frame_denominator,
        ctx,
        &mut |_, _| {},
    )
}

pub fn cook_cached(
    cache: &CookCache,
    snapshot: &GraphSnapshot,
    frame_numerator: i64,
    frame_denominator: u32,
    ctx: &EvalContext<'_>,
) -> Result<Arc<GeoDetail>, EvalError> {
    cook_cached_with_progress(
        cache,
        snapshot,
        frame_numerator,
        frame_denominator,
        ctx,
        |_, _| {},
    )
}
pub fn cook_cached_with_progress<F>(
    cache: &CookCache,
    snapshot: &GraphSnapshot,
    frame_numerator: i64,
    frame_denominator: u32,
    ctx: &EvalContext<'_>,
    mut on_progress: F,
) -> Result<Arc<GeoDetail>, EvalError>
where
    F: FnMut(u32, u32),
{
    let key = CookCache::key(snapshot, frame_numerator, frame_denominator);
    if let Some(value) = cache.get(&key) {
        let total = snapshot.nodes.len() as u32;
        on_progress(total, total);
        return Ok(value);
    }
    let value = eval_dag(
        snapshot,
        Some(cache),
        None,
        frame_numerator,
        frame_denominator,
        ctx,
        &mut on_progress,
    )?;
    let _ = cache.insert_if_fresh(key, snapshot.revision, Arc::clone(&value));
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    use pcc_graph::GraphSnapshot;
    #[test]
    fn same_hash_for_repeat_cooks() {
        let c = AtomicBool::new(false);
        let g = GraphSnapshot::demo(100, Vec3::checked(1., 2., 3.).unwrap_or(Vec3::ZERO));
        let x = EvalContext {
            cancelled: &c,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 1_000,
            },
        };
        assert_eq!(
            cook(&g, &x).map(|v| v.hash()),
            cook(&g, &x).map(|v| v.hash())
        );
    }
    #[test]
    fn cancellation_is_observed() {
        let c = AtomicBool::new(true);
        let x = EvalContext {
            cancelled: &c,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 1_000,
            },
        };
        assert!(matches!(
            cook(&GraphSnapshot::demo(1, Vec3::ZERO), &x),
            Err(EvalError::Cancelled)
        ));
    }
    #[test]
    fn cache_key_includes_frame_and_reuses_value() {
        let cancelled = AtomicBool::new(false);
        let context = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 1_000,
            },
        };
        let graph = GraphSnapshot::demo(1, Vec3::ZERO);
        let cache = CookCache::default();
        let first = cook_cached(&cache, &graph, 1, 24, &context);
        let second = cook_cached(&cache, &graph, 1, 24, &context);
        let other = CookCache::key(&graph, 2, 24);
        assert!(matches!((first,second),(Ok(a),Ok(b)) if Arc::ptr_eq(&a,&b)));
        assert!(cache.get(&other).is_none());
    }
    #[test]
    fn stale_revision_does_not_overwrite_newer_cache_state() {
        let cache = CookCache::default();
        let stale_graph = GraphSnapshot::demo(2, Vec3::ZERO);
        let mut fresh_graph = GraphSnapshot::demo(4, Vec3::ZERO);
        fresh_graph.revision = RevisionId(2);
        cache.note_published_revision(RevisionId(2));

        let stale_key = CookCache::key(&stale_graph, 0, 1);
        let stale_geo = Arc::new(GeoDetail::points(vec![Vec3::ZERO; 2]));
        assert!(!cache.insert_if_fresh(stale_key.clone(), stale_graph.revision, stale_geo));
        assert!(cache.get(&stale_key).is_none());

        let fresh_key = CookCache::key(&fresh_graph, 0, 1);
        let fresh_geo = Arc::new(GeoDetail::points(vec![Vec3::ZERO; 4]));
        assert!(cache.insert_if_fresh(fresh_key.clone(), fresh_graph.revision, fresh_geo));
        assert_eq!(cache.get(&fresh_key).map(|g| g.len()), Some(4));
    }
    #[test]
    fn cook_validates_graph_topology_and_supports_distinct_node_ids() {
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 1_000,
            },
        };
        let valid_custom_ids = GraphSnapshot {
            revision: RevisionId(3),
            nodes: vec![
                Node::CreatePoints {
                    id: pcc_id::NodeId(5),
                    count: 4,
                },
                Node::Transform {
                    id: pcc_id::NodeId(9),
                    input: pcc_id::NodeId(5),
                    offset: Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
                },
            ],
        };
        assert_eq!(cook(&valid_custom_ids, &ctx).map(|g| g.len()), Ok(4));

        let invalid_self_loop = GraphSnapshot {
            revision: RevisionId(3),
            nodes: vec![
                Node::CreatePoints {
                    id: pcc_id::NodeId(5),
                    count: 4,
                },
                Node::Transform {
                    id: pcc_id::NodeId(5),
                    input: pcc_id::NodeId(5),
                    offset: Vec3::ZERO,
                },
            ],
        };
        assert!(matches!(
            cook(&invalid_self_loop, &ctx),
            Err(EvalError::InvalidGraph)
        ));
    }

    #[test]
    fn diamond_dag_cook_reuses_unchanged_node_cache_entries_and_early_cutoff() {
        use pcc_graph::GraphEdit;
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 1_000,
            },
        };
        let cache = CookCache::default();
        let diamond_r1 = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreatePoints {
                    id: NodeId(1),
                    count: 10,
                },
                Node::Transform {
                    id: NodeId(2),
                    input: NodeId(1),
                    offset: Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
                },
                Node::Transform {
                    id: NodeId(3),
                    input: NodeId(1),
                    offset: Vec3::checked(0.0, 2.0, 0.0).unwrap_or(Vec3::ZERO),
                },
                Node::Merge {
                    id: NodeId(4),
                    left: NodeId(2),
                    right: NodeId(3),
                },
            ],
        };
        let first = cook_cached(&cache, &diamond_r1, 0, 1, &ctx);
        assert_eq!(first.as_ref().map(|g| g.len()), Ok(20));
        assert_eq!(cache.node_cache_hits(), 0);

        // Edit only branch B (`NodeId(3)`). Nodes 1 (`CreatePoints`) and 2 (`Transform`)
        // should hit the per-node content-addressed cache!
        let edited = diamond_r1.apply_edits(
            RevisionId(2),
            &[GraphEdit::SetTransformOffset {
                id: NodeId(3),
                offset: Vec3::checked(0.0, 5.0, 0.0).unwrap_or(Vec3::ZERO),
            }],
        );
        assert!(edited.is_ok());
        if let Ok(diamond_r2) = edited {
            let second = cook_cached(&cache, &diamond_r2, 0, 1, &ctx);
            assert_eq!(second.as_ref().map(|g| g.len()), Ok(20));
            assert_eq!(cache.node_cache_hits(), 2);
        }
    }

    #[test]
    fn grid_and_scatter_cook_and_intermediate_node_cook() {
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 10_000,
            },
        };
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateGrid {
                    id: NodeId(1),
                    rows: 4,
                    cols: 5,
                    size_x: 10.0,
                    size_z: 8.0,
                },
                Node::Scatter {
                    id: NodeId(2),
                    input: NodeId(1),
                    count: 50,
                    seed: 42,
                },
            ],
        };
        // Cook intermediate node 1 (CreateGrid): 4 * 5 = 20 points, (4-1)*(5-1) = 12 quad polygons, 48 vertices.
        let grid_geo = cook_node_cached(&cache, &snapshot, NodeId(1), 0, 1, &ctx);
        assert!(grid_geo.is_ok());
        if let Ok(grid) = grid_geo {
            assert_eq!(grid.len(), 20);
            assert_eq!(grid.prim_count(), 12);
            assert_eq!(grid.vertex_count(), 48);
        }
        // Cook node 2 (Scatter): 50 deterministic points within [0..10, 0..0, 0..8].
        let scatter_geo = cook_node_cached(&cache, &snapshot, NodeId(2), 0, 1, &ctx);
        assert!(scatter_geo.is_ok());
        if let Ok(scattered) = scatter_geo {
            assert_eq!(scattered.len(), 50);
            assert_eq!(scattered.prim_count(), 0);
            if let Some((bmin, bmax)) = scattered.bounds() {
                assert!(bmin.x >= 0.0 && bmax.x <= 10.0);
                assert_eq!(bmin.y, 0.0);
                assert_eq!(bmax.y, 0.0);
                assert!(bmin.z >= 0.0 && bmax.z <= 8.0);
            }
        }
        // Non-existent node returns NodeNotFound.
        assert!(matches!(
            cook_node_cached(&cache, &snapshot, NodeId(99), 0, 1, &ctx),
            Err(EvalError::NodeNotFound(NodeId(99)))
        ));
    }

    #[test]
    fn box_normal_wrangle_and_copy_to_points_cook() {
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 10_000,
            },
        };
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateBox {
                    id: NodeId(1),
                    size: Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
                },
                Node::Normal {
                    id: NodeId(2),
                    input: NodeId(1),
                },
                Node::CreatePoints {
                    id: NodeId(3),
                    count: 3,
                },
                Node::PointWrangle {
                    id: NodeId(4),
                    input: NodeId(3),
                    snippet: "f@pscale = 0.5; @P.x = float(@ptnum) * 10.0;".to_owned(),
                },
                Node::CopyToPoints {
                    id: NodeId(5),
                    source: NodeId(2),
                    target: NodeId(4),
                },
            ],
        };

        // Node 2 (CreateBox -> Normal): 8 points, 6 quad faces, closed manifold, volume = 8.0, has unit `N`
        let box_geo = cook_node_cached(&cache, &snapshot, NodeId(2), 0, 24, &ctx);
        assert!(box_geo.is_ok());
        if let Ok(box_geo) = box_geo {
            assert_eq!(box_geo.len(), 8);
            assert_eq!(box_geo.prim_count(), 6);
            assert!(box_geo.topology_index().is_closed_manifold());
            assert!((box_geo.enclosed_volume() - 8.0).abs() < 1e-4);
            assert!(box_geo.attribute("N").is_some());
        }

        // Node 5 (CopyToPoints): 3 copies of 8-point, 6-quad box scaled by pscale = 0.5 (volume per copy = 1.0, total volume = 3.0)
        let instanced = cook_cached(&cache, &snapshot, 0, 24, &ctx);
        assert!(instanced.is_ok());
        if let Ok(instanced) = instanced {
            assert_eq!(instanced.len(), 24);
            assert_eq!(instanced.prim_count(), 18);
            assert_eq!(instanced.vertex_count(), 72);
            assert!(instanced.topology_index().is_closed_manifold());
            assert!((instanced.enclosed_volume() - 3.0).abs() < 1e-4);
        }
    }

    #[test]
    fn subdivide_extrude_promote_and_blast_dag_cook() {
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 10_000,
            },
        };
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateBox {
                    id: NodeId(1),
                    size: Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
                },
                Node::Subdivide {
                    id: NodeId(2),
                    input: NodeId(1),
                    iterations: 1,
                },
                Node::Subdivide {
                    id: NodeId(3),
                    input: NodeId(2),
                    iterations: 1,
                },
                Node::PolyExtrude {
                    id: NodeId(4),
                    input: NodeId(1),
                    distance: 0.5,
                    inset: 0.1,
                },
                Node::PointWrangle {
                    id: NodeId(5),
                    input: NodeId(1),
                    snippet: "f@elev = @P.y; i@top = @P.y > 1.0;".to_owned(),
                },
                Node::AttribPromote {
                    id: NodeId(6),
                    input: NodeId(5),
                    name: "elev".to_owned(),
                    from_domain: AttrDomain::Point,
                    to_domain: AttrDomain::Prim,
                    mode: PromoteMode::Average,
                },
                Node::AttribPromote {
                    id: NodeId(7),
                    input: NodeId(6),
                    name: "elev".to_owned(),
                    from_domain: AttrDomain::Prim,
                    to_domain: AttrDomain::Detail,
                    mode: PromoteMode::Max,
                },
                Node::Blast {
                    id: NodeId(8),
                    input: NodeId(5),
                    group_name: "top".to_owned(),
                    negate: false,
                },
            ],
        };

        // Subdivide iteration 1: 6 -> 24 quads, closed manifold, positive volume
        let sub1 = cook_node_cached(&cache, &snapshot, NodeId(2), 0, 24, &ctx);
        assert!(sub1.is_ok());
        if let Ok(sub1) = sub1 {
            assert_eq!(sub1.prim_count(), 24);
            assert_eq!(sub1.len(), 26);
            assert!(sub1.topology_index().is_closed_manifold());
            assert!(sub1.enclosed_volume() > 2.0);
        }

        // Subdivide iteration 2: 24 -> 96 quads, closed manifold
        let sub2 = cook_node_cached(&cache, &snapshot, NodeId(3), 0, 24, &ctx);
        assert!(sub2.is_ok());
        if let Ok(sub2) = sub2 {
            assert_eq!(sub2.prim_count(), 96);
            assert_eq!(sub2.len(), 98);
            assert!(sub2.topology_index().is_closed_manifold());
            assert!(sub2.enclosed_volume() > 2.0);
        }

        // PolyExtrude on box: 6 original faces extruded -> 6 caps + 24 side walls = 30 quads, closed manifold, larger volume/area
        let ext = cook_node_cached(&cache, &snapshot, NodeId(4), 0, 24, &ctx);
        assert!(ext.is_ok());
        if let Ok(ext) = ext {
            assert_eq!(ext.prim_count(), 30);
            assert!(ext.topology_index().is_closed_manifold());
            assert!(ext.enclosed_volume() > 8.0);
            assert!(ext.surface_area() > 24.0);
        }

        // AttribPromote Point -> Prim (Average) -> Detail (Max) on 2x2x2 box where @P.y in {0.0, 2.0}
        let prom_prim = cook_node_cached(&cache, &snapshot, NodeId(6), 0, 24, &ctx);
        assert!(prom_prim.is_ok());
        if let Ok(prom_prim) = prom_prim {
            assert!(
                prom_prim
                    .domain_attribute(GeoAttrDomain::Point, "elev")
                    .is_none()
            );
            assert!(
                prom_prim
                    .domain_attribute(GeoAttrDomain::Prim, "elev")
                    .is_some()
            );
        }
        let prom_detail = cook_node_cached(&cache, &snapshot, NodeId(7), 0, 24, &ctx);
        assert!(prom_detail.is_ok());
        if let Ok(prom_detail) = prom_detail {
            let detail_val = prom_detail
                .domain_attribute(GeoAttrDomain::Detail, "elev")
                .and_then(AttributeColumn::as_f32)
                .and_then(|c| c.get(0))
                .map(|v| v.get());
            assert_eq!(detail_val, Some(2.0));
        }

        // Blast top points (`i@top = @P.y > 1.0`): removes 4 top points and 5 faces touching them, leaving only the bottom -Y face (4 points, 1 quad)
        let blasted = cook_node_cached(&cache, &snapshot, NodeId(8), 0, 24, &ctx);
        assert!(blasted.is_ok());
        if let Ok(blasted) = blasted {
            assert_eq!(blasted.len(), 4);
            assert_eq!(blasted.prim_count(), 1);
            assert!(blasted.topology_index().is_manifold());
        }
    }

    #[test]
    fn heightfield_noise_erode_and_uv_project_dag_cook() {
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 2 << 20,
                max_elements: 10_000,
            },
        };
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::HeightField {
                    id: NodeId(1),
                    rows: 12,
                    cols: 12,
                    size_x: 24.0,
                    size_z: 24.0,
                },
                Node::HeightFieldNoise {
                    id: NodeId(2),
                    input: NodeId(1),
                    amplitude: 6.0,
                    frequency: 0.1,
                    octaves: 4,
                    seed: 42,
                },
                Node::HeightFieldErode {
                    id: NodeId(3),
                    input: NodeId(2),
                    iterations: 10,
                    rain_rate: 0.06,
                    talus_angle: 35.0,
                    seed: 7,
                },
                Node::UvProject {
                    id: NodeId(4),
                    input: NodeId(3),
                    mode: UvProjectMode::PlanarY,
                },
                Node::Normal {
                    id: NodeId(5),
                    input: NodeId(4),
                },
            ],
        };

        let cooked = cook_cached(&cache, &snapshot, 0, 24, &ctx);
        assert!(cooked.is_ok());
        if let Ok(terrain) = cooked {
            assert_eq!(terrain.len(), 144);
            assert_eq!(terrain.prim_count(), 121);
            assert_eq!(terrain.vertex_count(), 484);
            assert!(terrain.topology_index().is_manifold());
            assert!(terrain.surface_area() > 576.0);
            assert!(terrain.attribute("height").is_some());
            assert!(terrain.attribute("water").is_some());
            assert!(terrain.attribute("sediment").is_some());
            assert!(terrain.attribute("mask").is_some());
            assert!(terrain.attribute("uv").is_some());
            assert!(terrain.attribute("N").is_some());
        }
    }

    #[test]
    fn vdb_pipeline_and_boolean_dag_cook_produce_closed_manifolds() {
        let cancelled = AtomicBool::new(false);
        let ctx = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 8 << 20,
                max_elements: 50_000,
            },
        };
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateBox {
                    id: NodeId(1),
                    size: Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
                },
                Node::Transform {
                    id: NodeId(2),
                    input: NodeId(1),
                    offset: Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
                },
                Node::VdbFromPolygons {
                    id: NodeId(3),
                    input: NodeId(1),
                    voxel_size: 0.5,
                },
                Node::VdbReshape {
                    id: NodeId(4),
                    input: NodeId(3),
                    offset: 0.25,
                    smooth_iterations: 1,
                },
                Node::ConvertVdb {
                    id: NodeId(5),
                    input: NodeId(4),
                },
                Node::Boolean {
                    id: NodeId(6),
                    left: NodeId(1),
                    right: NodeId(2),
                    mode: BooleanMode::Union,
                    voxel_size: 0.5,
                },
            ],
        };

        // Intermediate VdbFromPolygons (Node 3) exposes active narrow-band voxels (`phi`, `vx`, `vy`, `vz`)
        let vdb_pts = cook_node_cached(&cache, &snapshot, NodeId(3), 0, 24, &ctx);
        assert!(vdb_pts.is_ok());
        if let Ok(vdb_pts) = vdb_pts {
            assert!(vdb_pts.len() > 10);
            assert!(vdb_pts.attribute("phi").is_some());
            assert!(vdb_pts.attribute("vx").is_some());
        }

        // ConvertVDB (Node 5) extracts a closed 2-manifold quad mesh
        let surfaced = cook_node_cached(&cache, &snapshot, NodeId(5), 0, 24, &ctx);
        assert!(surfaced.is_ok());
        if let Ok(surfaced) = surfaced {
            assert!(surfaced.prim_count() > 6);
            assert!(surfaced.topology_index().is_closed_manifold());
            assert!(surfaced.enclosed_volume() > 6.0);
        }

        // Boolean Union (Node 6) of two overlapping 2x2x2 boxes produces a closed 2-manifold quad mesh
        let boolean_out = cook_cached(&cache, &snapshot, 0, 24, &ctx);
        assert!(boolean_out.is_ok());
        if let Ok(boolean_out) = boolean_out {
            assert!(boolean_out.topology_index().is_closed_manifold());
            assert!(boolean_out.enclosed_volume() > 8.0);
        }
    }
    fn sop_ctx(cancelled: &AtomicBool) -> EvalContext<'_> {
        EvalContext {
            cancelled,
            budget: CookBudget {
                max_memory_bytes: 256 << 20,
                max_elements: 2_000_000,
            },
        }
    }

    fn v3(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    /// Cooks one node, turning every failure into a readable test failure message.
    fn cook_at(
        cache: &CookCache,
        snapshot: &GraphSnapshot,
        id: u64,
        ctx: &EvalContext<'_>,
    ) -> Result<Arc<GeoDetail>, String> {
        cook_node_cached(cache, snapshot, NodeId(id), 0, 1, ctx)
            .map_err(|error| format!("node {id}: {error:?}"))
    }

    #[test]
    fn hard_surface_pipeline_keeps_solids_closed_and_volumes_consistent() -> Result<(), String> {
        let cancelled = AtomicBool::new(false);
        let ctx = sop_ctx(&cancelled);
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateTube {
                    id: NodeId(1),
                    radius: 1.0,
                    height: 2.0,
                    rows: 1,
                    cols: 32,
                },
                Node::PolyBevel {
                    id: NodeId(2),
                    input: NodeId(1),
                    distance: 0.05,
                    min_angle_degrees: 45.0,
                },
                Node::Xform {
                    id: NodeId(3),
                    input: NodeId(2),
                    translate: v3(0.0, 1.0, 0.0),
                    rotate_degrees: v3(0.0, 30.0, 0.0),
                    scale: v3(1.0, 1.0, 1.0),
                },
                Node::Clip {
                    id: NodeId(4),
                    input: NodeId(3),
                    origin: v3(0.0, 1.0, 0.0),
                    normal: v3(0.0, 1.0, 0.0),
                    cap: true,
                },
                Node::Triangulate {
                    id: NodeId(5),
                    input: NodeId(4),
                },
                Node::Fuse {
                    id: NodeId(6),
                    input: NodeId(5),
                    distance: 1e-5,
                },
                Node::PolyReduce {
                    id: NodeId(7),
                    input: NodeId(6),
                    target_ratio: 0.5,
                },
                Node::Remesh {
                    id: NodeId(8),
                    input: NodeId(7),
                    target_length: 0.25,
                    iterations: 3,
                    feature_angle_degrees: 30.0,
                },
                Node::UvFlatten {
                    id: NodeId(9),
                    input: NodeId(8),
                    seam_angle_degrees: 60.0,
                },
                Node::UvLayout {
                    id: NodeId(10),
                    input: NodeId(9),
                    padding: 0.01,
                    tiles: 2,
                },
                Node::Measure {
                    id: NodeId(11),
                    input: NodeId(10),
                },
            ],
        };
        let volume = |id| -> Result<f64, String> {
            let geo = cook_at(&cache, &snapshot, id, &ctx)?;
            assert!(
                geo.topology_index().is_closed_manifold(),
                "node {id} is not closed"
            );
            Ok(pcc_sop::signed_volume(&geo))
        };
        let tube = volume(1)?;
        let bevelled = volume(2)?;
        assert!(
            bevelled < tube && bevelled > 0.97 * tube,
            "{bevelled} vs {tube}"
        );
        // A rigid transform preserves volume, and the mid-plane clip keeps exactly half.
        assert!((volume(3)? - bevelled).abs() < 1e-4 * bevelled);
        let half = volume(4)?;
        assert!(
            (half - bevelled / 2.0).abs() < 1e-3 * bevelled,
            "{half} vs {bevelled}"
        );
        assert!((volume(5)? - half).abs() < 1e-5);
        let reduced = volume(7)?;
        assert!((reduced - half).abs() < 0.02 * half, "{reduced} vs {half}");
        let remeshed = volume(8)?;
        assert!(
            (remeshed - half).abs() < 0.03 * half,
            "{remeshed} vs {half}"
        );

        let measured = cook_at(&cache, &snapshot, 11, &ctx)?;
        assert!(
            measured
                .domain_attribute(GeoAttrDomain::Vertex, "uv")
                .is_some()
        );
        assert!(
            measured
                .domain_attribute(GeoAttrDomain::Prim, "uv_island")
                .is_some()
        );
        let total = measured
            .domain_attribute(GeoAttrDomain::Detail, "total_volume")
            .and_then(AttributeColumn::as_f32)
            .and_then(|column| column.get(0))
            .map(|value| f64::from(value.get()));
        assert_eq!(
            total.map(|t| (t - remeshed).abs() < 1e-3),
            Some(true),
            "{total:?}"
        );
        Ok(())
    }

    #[test]
    fn scattered_seeds_fracture_a_sphere_conserving_volume() -> Result<(), String> {
        let cancelled = AtomicBool::new(false);
        let ctx = sop_ctx(&cancelled);
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateSphere {
                    id: NodeId(1),
                    radius: 1.0,
                    rows: 16,
                    cols: 24,
                },
                Node::ScatterSurface {
                    id: NodeId(2),
                    input: NodeId(1),
                    count: 12,
                    seed: 5,
                    min_distance: 0.3,
                },
                Node::Xform {
                    id: NodeId(3),
                    input: NodeId(2),
                    translate: Vec3::ZERO,
                    rotate_degrees: Vec3::ZERO,
                    scale: v3(0.7, 0.7, 0.7),
                },
                Node::VoronoiFracture {
                    id: NodeId(4),
                    input: NodeId(1),
                    seeds: NodeId(3),
                },
            ],
        };
        let whole = pcc_sop::signed_volume(&*cook_at(&cache, &snapshot, 1, &ctx)?);
        let seeds = cook_at(&cache, &snapshot, 3, &ctx)?;
        assert_eq!(seeds.len(), 12);
        let shards = cook_at(&cache, &snapshot, 4, &ctx)?;
        assert!(shards.topology_index().is_closed_manifold());
        let total = pcc_sop::signed_volume(&shards);
        assert!((total - whole).abs() < 1e-3 * whole, "{total} vs {whole}");
        Ok(())
    }

    #[test]
    fn curves_groups_and_attribute_transfer_compose_in_one_graph() -> Result<(), String> {
        let cancelled = AtomicBool::new(false);
        let ctx = sop_ctx(&cancelled);
        let cache = CookCache::default();
        let snapshot = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateLine {
                    id: NodeId(1),
                    start: Vec3::ZERO,
                    end: v3(0.0, 4.0, 0.0),
                    points: 2,
                },
                Node::Resample {
                    id: NodeId(2),
                    input: NodeId(1),
                    segment_length: 0.5,
                },
                Node::Sweep {
                    id: NodeId(3),
                    input: NodeId(2),
                    radius: 0.25,
                    sides: 8,
                    capped: true,
                },
                Node::PointWrangle {
                    id: NodeId(4),
                    input: NodeId(3),
                    snippet: "@heat = @P.y;".to_owned(),
                },
                Node::CreateSphere {
                    id: NodeId(5),
                    radius: 3.0,
                    rows: 16,
                    cols: 24,
                },
                Node::AttribTransfer {
                    id: NodeId(6),
                    input: NodeId(5),
                    source: NodeId(4),
                    name: "heat".to_owned(),
                    radius: 2.0,
                },
                Node::GroupBounds {
                    id: NodeId(7),
                    input: NodeId(6),
                    group_name: "upper".to_owned(),
                    domain: AttrDomain::Prim,
                    min: v3(-4.0, 0.0, -4.0),
                    max: v3(4.0, 4.0, 4.0),
                },
                Node::Blast {
                    id: NodeId(8),
                    input: NodeId(7),
                    group_name: "upper".to_owned(),
                    negate: false,
                },
            ],
        };
        let resampled = cook_at(&cache, &snapshot, 2, &ctx)?;
        assert_eq!(resampled.len(), 9);
        let tube = cook_at(&cache, &snapshot, 3, &ctx)?;
        assert_eq!(tube.len(), 9 * 8);
        assert!(tube.topology_index().is_closed_manifold());
        let transferred = cook_at(&cache, &snapshot, 6, &ctx)?;
        let heat = transferred
            .attribute("heat")
            .and_then(AttributeColumn::as_f32)
            .ok_or("heat was not transferred")?;
        // Top pole (y=3) is near the tube's upper end; bottom pole (y=-3) has no source in range.
        let top = heat.get(0).map(|value| value.get()).unwrap_or(0.0);
        assert!(top > 2.5 && top < 4.0, "top heat {top}");
        let bottom = heat
            .get(transferred.len() - 1)
            .map(|value| value.get())
            .unwrap_or(1.0);
        assert_eq!(bottom, 0.0);
        let blasted = cook_at(&cache, &snapshot, 8, &ctx)?;
        assert!(blasted.prim_count() > 0 && blasted.prim_count() < transferred.prim_count());
        let (_, high) = blasted.bounds().ok_or("no bounds")?;
        assert!(high.y < 0.5, "upper half survived: {high:?}");
        Ok(())
    }

    #[test]
    fn sop_nodes_share_the_cache_contract_and_budgets() -> Result<(), String> {
        let cancelled = AtomicBool::new(false);
        let ctx = sop_ctx(&cancelled);
        let cache = CookCache::default();
        let graph = |ratio: f32| GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateSphere {
                    id: NodeId(1),
                    radius: 1.0,
                    rows: 12,
                    cols: 16,
                },
                Node::PolyReduce {
                    id: NodeId(2),
                    input: NodeId(1),
                    target_ratio: ratio,
                },
            ],
        };
        let first = cook_at(&cache, &graph(0.5), 2, &ctx)?;
        let hits_before = cache.node_cache_hits();
        let again = cook_at(&cache, &graph(0.5), 2, &ctx)?;
        assert_eq!(first.hash(), again.hash());
        assert_eq!(
            cache.node_cache_hits(),
            hits_before + 2,
            "both nodes must hit"
        );
        // A changed parameter re-cooks only the node it belongs to.
        let other = cook_at(&cache, &graph(0.25), 2, &ctx)?;
        assert_eq!(cache.node_cache_hits(), hits_before + 3);
        assert_ne!(other.hash(), first.hash());

        let tight = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 30,
                max_elements: 1_000,
            },
        };
        let oversized = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateSphere {
                    id: NodeId(1),
                    radius: 1.0,
                    rows: 2_000,
                    cols: 2_000,
                },
                Node::Reverse {
                    id: NodeId(2),
                    input: NodeId(1),
                },
            ],
        };
        assert_eq!(
            cook(&oversized, &tight).err(),
            Some(EvalError::ElementLimit {
                requested: 4_000_000,
                limit: 1_000
            })
        );
        let starved = EvalContext {
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 64,
                max_elements: 1_000_000,
            },
        };
        assert_eq!(
            cook(&graph(0.5), &starved).err(),
            Some(EvalError::Resource(GeoError::LimitExceeded))
        );
        let stopped = AtomicBool::new(true);
        assert_eq!(
            cook(&graph(0.5), &sop_ctx(&stopped)).err(),
            Some(EvalError::Cancelled)
        );
        // Fracturing a surface that is not a closed solid surfaces the operator's error.
        let open_input = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreateGrid {
                    id: NodeId(1),
                    rows: 3,
                    cols: 3,
                    size_x: 1.0,
                    size_z: 1.0,
                },
                Node::VoronoiFracture {
                    id: NodeId(2),
                    input: NodeId(1),
                    seeds: NodeId(1),
                },
            ],
        };
        assert_eq!(
            cook(&open_input, &ctx).err(),
            Some(EvalError::Resource(GeoError::InvalidTopology))
        );
        Ok(())
    }
}
