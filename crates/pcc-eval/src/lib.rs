//! Deterministic DAG evaluator with whole-graph and per-node content-addressed caching
//! and early cutoff.
use pcc_geo::{
    AttrDomain as GeoAttrDomain, AttributeColumn, GeoDetail, GeoError, GroupDomain, PAGE_LEN,
    PrimKind, PrimRun, PromoteMode as GeoPromoteMode, UvProjectMode as GeoUvProjectMode,
};
use pcc_graph::{
    AttrDomain, BooleanMode, GraphSnapshot, Node, NodeDef, NodeRegistrationError, PromoteMode,
    UvProjectMode, node_def, register_node_def,
};
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
        Arc, Mutex, OnceLock, RwLock,
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
    Asset(AssetError),
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
    pub assets: &'a dyn AssetResolver,
}

/// Why an `asset` node's content could not be supplied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetError {
    NotFound,
    /// Stored data did not match its content hash.
    Corrupt,
}

/// Supplies imported geometry by content hash, keeping the evaluator free of file access.
pub trait AssetResolver: Sync {
    fn resolve(&self, content_hash: &str) -> Result<Arc<GeoDetail>, AssetError>;
}

/// For cooks in which no scene may refer to imported geometry.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAssets;

impl AssetResolver for NoAssets {
    fn resolve(&self, _content_hash: &str) -> Result<Arc<GeoDetail>, AssetError> {
        Err(AssetError::NotFound)
    }
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
        hasher.update(b"pcc-eval-node-cache-v2");
        hasher.update(&(node.op.len() as u64).to_le_bytes());
        hasher.update(node.op.as_bytes());
        if node_def(&node.op).is_some_and(|def| def.seeded_by_node_id) {
            hasher.update(&node.id.0.to_le_bytes());
        }
        for (name, value) in &node.params {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            value.update_hash(|bytes| {
                hasher.update(bytes);
            });
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

fn param_f32(node: &Node, name: &str) -> Result<f32, EvalError> {
    node.f32(name).ok_or(EvalError::InvalidGraph)
}

fn param_u32(node: &Node, name: &str) -> Result<u32, EvalError> {
    node.u32(name).ok_or(EvalError::InvalidGraph)
}

fn param_u64(node: &Node, name: &str) -> Result<u64, EvalError> {
    node.u64(name).ok_or(EvalError::InvalidGraph)
}

fn param_bool(node: &Node, name: &str) -> Result<bool, EvalError> {
    node.boolean(name).ok_or(EvalError::InvalidGraph)
}

fn param_vec3(node: &Node, name: &str) -> Result<Vec3, EvalError> {
    node.vec3(name).ok_or(EvalError::InvalidGraph)
}

fn param_text<'a>(node: &'a Node, name: &str) -> Result<&'a str, EvalError> {
    node.text(name).ok_or(EvalError::InvalidGraph)
}

fn param_choice<T, E>(
    node: &Node,
    name: &str,
    from_token: impl Fn(&str) -> Result<T, E>,
) -> Result<T, EvalError> {
    from_token(param_text(node, name)?).map_err(|_| EvalError::InvalidGraph)
}

fn input_slot(node: &Node, slot: usize) -> Result<NodeId, EvalError> {
    node.inputs
        .get(slot)
        .copied()
        .ok_or(EvalError::InvalidGraph)
}

/// Cooks one node of a runtime-registered operator from its evaluated inputs.
pub type CookFn = fn(&Node, &[Arc<GeoDetail>], &EvalContext<'_>) -> Result<GeoDetail, EvalError>;

fn cook_table() -> &'static RwLock<BTreeMap<&'static str, CookFn>> {
    static TABLE: OnceLock<RwLock<BTreeMap<&'static str, CookFn>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn registered_cook(op: &str) -> Option<CookFn> {
    cook_table().read().ok()?.get(op).copied()
}

/// Adds an operator at runtime: its definition makes it part of the catalog, validation, and
/// `.ofxs`, and `cook` evaluates it. This is the seam that external plugins will use.
pub fn register_operator(def: NodeDef, cook: CookFn) -> Result<(), NodeRegistrationError> {
    let name = def.type_name;
    register_node_def(def)?;
    if let Ok(mut table) = cook_table().write() {
        table.insert(name, cook);
    }
    Ok(())
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
    match node.op.as_str() {
        "create" => {
            let count = &param_u32(node, "count")?;

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
        "grid" => {
            let rows = &param_u32(node, "rows")?;
            let cols = &param_u32(node, "cols")?;
            let size_x = &param_f32(node, "size_x")?;
            let size_z = &param_f32(node, "size_z")?;

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
        "box" => {
            let size = &param_vec3(node, "size")?;

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
        "transform" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let offset = &param_vec3(node, "offset")?;

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
        "normal" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;

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
        "wrangle" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let snippet = param_text(node, "snippet")?;

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
        "scatter" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let count = &param_u32(node, "count")?;
            let seed = &param_u64(node, "seed")?;

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
        "copytopoints" => {
            let id = &node.id;
            let source = &input_slot(node, 0)?;
            let target = &input_slot(node, 1)?;

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
        "merge" => {
            let id = &node.id;
            let left = &input_slot(node, 0)?;
            let right = &input_slot(node, 1)?;

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
        "subdivide" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let iterations = &param_u32(node, "iterations")?;

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
        "extrude" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let distance = &param_f32(node, "distance")?;
            let inset = &param_f32(node, "inset")?;

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
        "attribpromote" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let name = param_text(node, "name")?;
            let from_domain = &param_choice(node, "from_domain", AttrDomain::from_token)?;
            let to_domain = &param_choice(node, "to_domain", AttrDomain::from_token)?;
            let mode = &param_choice(node, "mode", PromoteMode::from_token)?;

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
        "blast" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let group_name = param_text(node, "group_name")?;
            let negate = &param_bool(node, "negate")?;

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
        "uvproject" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let mode = &param_choice(node, "mode", UvProjectMode::from_token)?;

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
        "heightfield" => {
            let rows = &param_u32(node, "rows")?;
            let cols = &param_u32(node, "cols")?;
            let size_x = &param_f32(node, "size_x")?;
            let size_z = &param_f32(node, "size_z")?;

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
        "hf_noise" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let amplitude = &param_f32(node, "amplitude")?;
            let frequency = &param_f32(node, "frequency")?;
            let octaves = &param_u32(node, "octaves")?;
            let seed = &param_u64(node, "seed")?;

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
        "hf_erode" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let iterations = &param_u32(node, "iterations")?;
            let rain_rate = &param_f32(node, "rain_rate")?;
            let talus_angle = &param_f32(node, "talus_angle")?;
            let seed = &param_u64(node, "seed")?;

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
        "vdb_from_polygons" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let voxel_size = &param_f32(node, "voxel_size")?;

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
        "vdb_reshape" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;
            let offset = &param_f32(node, "offset")?;
            let smooth_iterations = &param_u32(node, "smooth_iterations")?;

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
        "convert_vdb" => {
            let id = &node.id;
            let input = &input_slot(node, 0)?;

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
        "boolean" => {
            let id = &node.id;
            let left = &input_slot(node, 0)?;
            let right = &input_slot(node, 1)?;
            let mode = &param_choice(node, "mode", BooleanMode::from_token)?;
            let voxel_size = &param_f32(node, "voxel_size")?;

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
        "asset" => {
            let content_hash = param_text(node, "content_hash")?;

            let geo = ctx.assets.resolve(content_hash).map_err(EvalError::Asset)?;
            let bytes = geo
                .len()
                .saturating_mul(std::mem::size_of::<Vec3>())
                .saturating_add(
                    geo.vertex_count()
                        .saturating_mul(std::mem::size_of::<u32>()),
                );
            if bytes > ctx.budget.max_memory_bytes {
                return Err(EvalError::Resource(GeoError::LimitExceeded));
            }
            finish_sop(Ok((*geo).clone()), ctx)
        }
        "sphere" => {
            let radius = &param_f32(node, "radius")?;
            let rows = &param_u32(node, "rows")?;
            let cols = &param_u32(node, "cols")?;

            check_element_estimate(rows.saturating_mul(*cols), ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(create_sphere(f64::from(*radius), *rows, *cols, &sop), ctx)
        }
        "tube" => {
            let radius = &param_f32(node, "radius")?;
            let height = &param_f32(node, "height")?;
            let rows = &param_u32(node, "rows")?;
            let cols = &param_u32(node, "cols")?;

            check_element_estimate(rows.saturating_add(1).saturating_mul(*cols), ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                create_tube(f64::from(*radius), f64::from(*height), *rows, *cols, &sop),
                ctx,
            )
        }
        "torus" => {
            let major_radius = &param_f32(node, "major_radius")?;
            let minor_radius = &param_f32(node, "minor_radius")?;
            let rows = &param_u32(node, "rows")?;
            let cols = &param_u32(node, "cols")?;

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
        "line" => {
            let start = &param_vec3(node, "start")?;
            let end = &param_vec3(node, "end")?;
            let points = &param_u32(node, "points")?;

            check_element_estimate(*points, ctx)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(create_line(*start, *end, *points, &sop), ctx)
        }
        "xform" => {
            let input = &input_slot(node, 0)?;
            let translate = &param_vec3(node, "translate")?;
            let rotate_degrees = &param_vec3(node, "rotate_degrees")?;
            let scale = &param_vec3(node, "scale")?;

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
        "reverse" => {
            let input = &input_slot(node, 0)?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(reverse_winding(input_geo, &sop), ctx)
        }
        "triangulate" => {
            let input = &input_slot(node, 0)?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(triangulate(input_geo, &sop), ctx)
        }
        "fuse" => {
            let input = &input_slot(node, 0)?;
            let distance = &param_f32(node, "distance")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(fuse_points(input_geo, f64::from(*distance), &sop), ctx)
        }
        "resample" => {
            let input = &input_slot(node, 0)?;
            let segment_length = &param_f32(node, "segment_length")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                resample_curves(input_geo, f64::from(*segment_length), &sop),
                ctx,
            )
        }
        "sweep" => {
            let input = &input_slot(node, 0)?;
            let radius = &param_f32(node, "radius")?;
            let sides = &param_u32(node, "sides")?;
            let capped = &param_bool(node, "capped")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                sweep_curves(input_geo, f64::from(*radius), *sides, *capped, &sop),
                ctx,
            )
        }
        "attribtransfer" => {
            let input = &input_slot(node, 0)?;
            let source = &input_slot(node, 1)?;
            let name = param_text(node, "name")?;
            let radius = &param_f32(node, "radius")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let source_geo = outputs.get(source).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                attrib_transfer(input_geo, source_geo, name, f64::from(*radius), &sop),
                ctx,
            )
        }
        "measure" => {
            let input = &input_slot(node, 0)?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(measure(input_geo, &sop), ctx)
        }
        "polybevel" => {
            let input = &input_slot(node, 0)?;
            let distance = &param_f32(node, "distance")?;
            let min_angle_degrees = &param_f32(node, "min_angle_degrees")?;

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
        "polyreduce" => {
            let input = &input_slot(node, 0)?;
            let target_ratio = &param_f32(node, "target_ratio")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(poly_reduce(input_geo, f64::from(*target_ratio), &sop), ctx)
        }
        "remesh" => {
            let input = &input_slot(node, 0)?;
            let target_length = &param_f32(node, "target_length")?;
            let iterations = &param_u32(node, "iterations")?;
            let feature_angle_degrees = &param_f32(node, "feature_angle_degrees")?;

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
        "clip" => {
            let input = &input_slot(node, 0)?;
            let origin = &param_vec3(node, "origin")?;
            let normal = &param_vec3(node, "normal")?;
            let cap = &param_bool(node, "cap")?;

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
        "voronoifracture" => {
            let input = &input_slot(node, 0)?;
            let seeds = &input_slot(node, 1)?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let seeds_geo = outputs.get(seeds).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(voronoi_fracture(input_geo, seeds_geo, &sop), ctx)
        }
        "scatter_surface" => {
            let input = &input_slot(node, 0)?;
            let count = &param_u32(node, "count")?;
            let seed = &param_u64(node, "seed")?;
            let min_distance = &param_f32(node, "min_distance")?;

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
        "uvflatten" => {
            let input = &input_slot(node, 0)?;
            let seam_angle_degrees = &param_f32(node, "seam_angle_degrees")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(
                uv_flatten(input_geo, f64::from(*seam_angle_degrees), &sop),
                ctx,
            )
        }
        "uvlayout" => {
            let input = &input_slot(node, 0)?;
            let padding = &param_f32(node, "padding")?;
            let tiles = &param_u32(node, "tiles")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let sop = SopContext::new(ctx.budget.max_memory_bytes, ctx.cancelled);
            finish_sop(uv_layout(input_geo, f64::from(*padding), *tiles, &sop), ctx)
        }
        "group_bounds" => {
            let input = &input_slot(node, 0)?;
            let group_name = param_text(node, "group_name")?;
            let domain = &param_choice(node, "domain", AttrDomain::from_token)?;
            let min = &param_vec3(node, "min")?;
            let max = &param_vec3(node, "max")?;

            let input_geo = outputs.get(input).ok_or(EvalError::InvalidGraph)?;
            let group_domain = match domain {
                AttrDomain::Point => GroupDomain::Point,
                AttrDomain::Prim => GroupDomain::Prim,
                AttrDomain::Detail => return Err(EvalError::InvalidGraph),
            };
            finish_sop(
                input_geo.group_by_bounds(group_domain, group_name, *min, *max),
                ctx,
            )
        }
        other => {
            let cook = registered_cook(other).ok_or(EvalError::InvalidGraph)?;
            let mut inputs = Vec::with_capacity(node.inputs.len());
            for id in &node.inputs {
                inputs.push(Arc::clone(outputs.get(id).ok_or(EvalError::InvalidGraph)?));
            }
            cook(node, &inputs, ctx)
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
            assets: &NoAssets,
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
            assets: &NoAssets,
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
            assets: &NoAssets,
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
            assets: &NoAssets,
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 20,
                max_elements: 1_000,
            },
        };
        let valid_custom_ids = GraphSnapshot {
            revision: RevisionId(3),
            nodes: vec![
                Node::new("create", pcc_id::NodeId(5)).with_u32("count", 4),
                Node::new("transform", pcc_id::NodeId(9))
                    .with_input(pcc_id::NodeId(5))
                    .with_vec3("offset", Vec3::checked(2.0, 0.0, 0.0).unwrap_or(Vec3::ZERO)),
            ],
        };
        assert_eq!(cook(&valid_custom_ids, &ctx).map(|g| g.len()), Ok(4));

        let invalid_self_loop = GraphSnapshot {
            revision: RevisionId(3),
            nodes: vec![
                Node::new("create", pcc_id::NodeId(5)).with_u32("count", 4),
                Node::new("transform", pcc_id::NodeId(5))
                    .with_input(pcc_id::NodeId(5))
                    .with_vec3("offset", Vec3::ZERO),
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
            assets: &NoAssets,
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
                Node::new("create", NodeId(1)).with_u32("count", 10),
                Node::new("transform", NodeId(2))
                    .with_input(NodeId(1))
                    .with_vec3("offset", Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO)),
                Node::new("transform", NodeId(3))
                    .with_input(NodeId(1))
                    .with_vec3("offset", Vec3::checked(0.0, 2.0, 0.0).unwrap_or(Vec3::ZERO)),
                Node::new("merge", NodeId(4))
                    .with_input(NodeId(2))
                    .with_input(NodeId(3)),
            ],
        };
        let first = cook_cached(&cache, &diamond_r1, 0, 1, &ctx);
        assert_eq!(first.as_ref().map(|g| g.len()), Ok(20));
        assert_eq!(cache.node_cache_hits(), 0);

        // Edit only branch B (`NodeId(3)`). Nodes 1 (`CreatePoints`) and 2 (`Transform`)
        // should hit the per-node content-addressed cache!
        let edited = diamond_r1.apply_edits(
            RevisionId(2),
            &[GraphEdit::SetParam {
                id: NodeId(3),
                name: "offset".to_owned(),
                value: pcc_graph::ParamValue::Vec3(
                    Vec3::checked(0.0, 5.0, 0.0).unwrap_or(Vec3::ZERO),
                ),
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
            assets: &NoAssets,
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
                Node::new("grid", NodeId(1))
                    .with_u32("rows", 4)
                    .with_u32("cols", 5)
                    .with_f32("size_x", 10.0)
                    .with_f32("size_z", 8.0),
                Node::new("scatter", NodeId(2))
                    .with_input(NodeId(1))
                    .with_u32("count", 50)
                    .with_u64("seed", 42),
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
            assets: &NoAssets,
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
                Node::new("box", NodeId(1))
                    .with_vec3("size", Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO)),
                Node::new("normal", NodeId(2)).with_input(NodeId(1)),
                Node::new("create", NodeId(3)).with_u32("count", 3),
                Node::new("wrangle", NodeId(4))
                    .with_input(NodeId(3))
                    .with_text(
                        "snippet",
                        "f@pscale = 0.5; @P.x = float(@ptnum) * 10.0;".to_owned(),
                    ),
                Node::new("copytopoints", NodeId(5))
                    .with_input(NodeId(2))
                    .with_input(NodeId(4)),
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
            assets: &NoAssets,
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
                Node::new("box", NodeId(1))
                    .with_vec3("size", Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO)),
                Node::new("subdivide", NodeId(2))
                    .with_input(NodeId(1))
                    .with_u32("iterations", 1),
                Node::new("subdivide", NodeId(3))
                    .with_input(NodeId(2))
                    .with_u32("iterations", 1),
                Node::new("extrude", NodeId(4))
                    .with_input(NodeId(1))
                    .with_f32("distance", 0.5)
                    .with_f32("inset", 0.1),
                Node::new("wrangle", NodeId(5))
                    .with_input(NodeId(1))
                    .with_text("snippet", "f@elev = @P.y; i@top = @P.y > 1.0;".to_owned()),
                Node::new("attribpromote", NodeId(6))
                    .with_input(NodeId(5))
                    .with_text("name", "elev".to_owned())
                    .with_text("from_domain", AttrDomain::Point.as_str())
                    .with_text("to_domain", AttrDomain::Prim.as_str())
                    .with_text("mode", PromoteMode::Average.as_str()),
                Node::new("attribpromote", NodeId(7))
                    .with_input(NodeId(6))
                    .with_text("name", "elev".to_owned())
                    .with_text("from_domain", AttrDomain::Prim.as_str())
                    .with_text("to_domain", AttrDomain::Detail.as_str())
                    .with_text("mode", PromoteMode::Max.as_str()),
                Node::new("blast", NodeId(8))
                    .with_input(NodeId(5))
                    .with_text("group_name", "top".to_owned())
                    .with_bool("negate", false),
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
            assets: &NoAssets,
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
                Node::new("heightfield", NodeId(1))
                    .with_u32("rows", 12)
                    .with_u32("cols", 12)
                    .with_f32("size_x", 24.0)
                    .with_f32("size_z", 24.0),
                Node::new("hf_noise", NodeId(2))
                    .with_input(NodeId(1))
                    .with_f32("amplitude", 6.0)
                    .with_f32("frequency", 0.1)
                    .with_u32("octaves", 4)
                    .with_u64("seed", 42),
                Node::new("hf_erode", NodeId(3))
                    .with_input(NodeId(2))
                    .with_u32("iterations", 10)
                    .with_f32("rain_rate", 0.06)
                    .with_f32("talus_angle", 35.0)
                    .with_u64("seed", 7),
                Node::new("uvproject", NodeId(4))
                    .with_input(NodeId(3))
                    .with_text("mode", UvProjectMode::PlanarY.as_str()),
                Node::new("normal", NodeId(5)).with_input(NodeId(4)),
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
            assets: &NoAssets,
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
                Node::new("box", NodeId(1))
                    .with_vec3("size", Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO)),
                Node::new("transform", NodeId(2))
                    .with_input(NodeId(1))
                    .with_vec3("offset", Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO)),
                Node::new("vdb_from_polygons", NodeId(3))
                    .with_input(NodeId(1))
                    .with_f32("voxel_size", 0.5),
                Node::new("vdb_reshape", NodeId(4))
                    .with_input(NodeId(3))
                    .with_f32("offset", 0.25)
                    .with_u32("smooth_iterations", 1),
                Node::new("convert_vdb", NodeId(5)).with_input(NodeId(4)),
                Node::new("boolean", NodeId(6))
                    .with_input(NodeId(1))
                    .with_input(NodeId(2))
                    .with_text("mode", BooleanMode::Union.as_str())
                    .with_f32("voxel_size", 0.5),
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
            assets: &NoAssets,
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
                Node::new("tube", NodeId(1))
                    .with_f32("radius", 1.0)
                    .with_f32("height", 2.0)
                    .with_u32("rows", 1)
                    .with_u32("cols", 32),
                Node::new("polybevel", NodeId(2))
                    .with_input(NodeId(1))
                    .with_f32("distance", 0.05)
                    .with_f32("min_angle_degrees", 45.0),
                Node::new("xform", NodeId(3))
                    .with_input(NodeId(2))
                    .with_vec3("translate", v3(0.0, 1.0, 0.0))
                    .with_vec3("rotate_degrees", v3(0.0, 30.0, 0.0))
                    .with_vec3("scale", v3(1.0, 1.0, 1.0)),
                Node::new("clip", NodeId(4))
                    .with_input(NodeId(3))
                    .with_vec3("origin", v3(0.0, 1.0, 0.0))
                    .with_vec3("normal", v3(0.0, 1.0, 0.0))
                    .with_bool("cap", true),
                Node::new("triangulate", NodeId(5)).with_input(NodeId(4)),
                Node::new("fuse", NodeId(6))
                    .with_input(NodeId(5))
                    .with_f32("distance", 1e-5),
                Node::new("polyreduce", NodeId(7))
                    .with_input(NodeId(6))
                    .with_f32("target_ratio", 0.5),
                Node::new("remesh", NodeId(8))
                    .with_input(NodeId(7))
                    .with_f32("target_length", 0.25)
                    .with_u32("iterations", 3)
                    .with_f32("feature_angle_degrees", 30.0),
                Node::new("uvflatten", NodeId(9))
                    .with_input(NodeId(8))
                    .with_f32("seam_angle_degrees", 60.0),
                Node::new("uvlayout", NodeId(10))
                    .with_input(NodeId(9))
                    .with_f32("padding", 0.01)
                    .with_u32("tiles", 2),
                Node::new("measure", NodeId(11)).with_input(NodeId(10)),
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
                Node::new("sphere", NodeId(1))
                    .with_f32("radius", 1.0)
                    .with_u32("rows", 16)
                    .with_u32("cols", 24),
                Node::new("scatter_surface", NodeId(2))
                    .with_input(NodeId(1))
                    .with_u32("count", 12)
                    .with_u64("seed", 5)
                    .with_f32("min_distance", 0.3),
                Node::new("xform", NodeId(3))
                    .with_input(NodeId(2))
                    .with_vec3("translate", Vec3::ZERO)
                    .with_vec3("rotate_degrees", Vec3::ZERO)
                    .with_vec3("scale", v3(0.7, 0.7, 0.7)),
                Node::new("voronoifracture", NodeId(4))
                    .with_input(NodeId(1))
                    .with_input(NodeId(3)),
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
                Node::new("line", NodeId(1))
                    .with_vec3("start", Vec3::ZERO)
                    .with_vec3("end", v3(0.0, 4.0, 0.0))
                    .with_u32("points", 2),
                Node::new("resample", NodeId(2))
                    .with_input(NodeId(1))
                    .with_f32("segment_length", 0.5),
                Node::new("sweep", NodeId(3))
                    .with_input(NodeId(2))
                    .with_f32("radius", 0.25)
                    .with_u32("sides", 8)
                    .with_bool("capped", true),
                Node::new("wrangle", NodeId(4))
                    .with_input(NodeId(3))
                    .with_text("snippet", "@heat = @P.y;".to_owned()),
                Node::new("sphere", NodeId(5))
                    .with_f32("radius", 3.0)
                    .with_u32("rows", 16)
                    .with_u32("cols", 24),
                Node::new("attribtransfer", NodeId(6))
                    .with_input(NodeId(5))
                    .with_input(NodeId(4))
                    .with_text("name", "heat".to_owned())
                    .with_f32("radius", 2.0),
                Node::new("group_bounds", NodeId(7))
                    .with_input(NodeId(6))
                    .with_text("group_name", "upper".to_owned())
                    .with_text("domain", AttrDomain::Prim.as_str())
                    .with_vec3("min", v3(-4.0, 0.0, -4.0))
                    .with_vec3("max", v3(4.0, 4.0, 4.0)),
                Node::new("blast", NodeId(8))
                    .with_input(NodeId(7))
                    .with_text("group_name", "upper".to_owned())
                    .with_bool("negate", false),
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
                Node::new("sphere", NodeId(1))
                    .with_f32("radius", 1.0)
                    .with_u32("rows", 12)
                    .with_u32("cols", 16),
                Node::new("polyreduce", NodeId(2))
                    .with_input(NodeId(1))
                    .with_f32("target_ratio", ratio),
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
            assets: &NoAssets,
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1 << 30,
                max_elements: 1_000,
            },
        };
        let oversized = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("sphere", NodeId(1))
                    .with_f32("radius", 1.0)
                    .with_u32("rows", 2_000)
                    .with_u32("cols", 2_000),
                Node::new("reverse", NodeId(2)).with_input(NodeId(1)),
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
            assets: &NoAssets,
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
                Node::new("grid", NodeId(1))
                    .with_u32("rows", 3)
                    .with_u32("cols", 3)
                    .with_f32("size_x", 1.0)
                    .with_f32("size_z", 1.0),
                Node::new("voronoifracture", NodeId(2))
                    .with_input(NodeId(1))
                    .with_input(NodeId(1)),
            ],
        };
        assert_eq!(
            cook(&open_input, &ctx).err(),
            Some(EvalError::Resource(GeoError::InvalidTopology))
        );
        Ok(())
    }

    /// Serves one fixed geometry under one hash.
    struct OneAsset(String, Arc<GeoDetail>);

    impl AssetResolver for OneAsset {
        fn resolve(&self, content_hash: &str) -> Result<Arc<GeoDetail>, AssetError> {
            if content_hash == self.0 {
                Ok(Arc::clone(&self.1))
            } else {
                Err(AssetError::NotFound)
            }
        }
    }

    #[test]
    fn asset_nodes_cook_through_the_resolver_and_key_the_cache_by_content() -> Result<(), String> {
        let cancelled = AtomicBool::new(false);
        let point = Vec3::checked(7.0, 0.0, 0.0).map_err(|e| format!("{e:?}"))?;
        let geo = Arc::new(GeoDetail::points(vec![point]));
        let hash = geo.hash().to_hex().to_string();
        let resolver = OneAsset(hash.clone(), Arc::clone(&geo));
        let graph = |content_hash: String| GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("asset", NodeId(1)).with_text("content_hash", content_hash),
                Node::new("normal", NodeId(2)).with_input(NodeId(1)),
            ],
        };
        let budget = CookBudget {
            max_memory_bytes: 1 << 20,
            max_elements: 1000,
        };
        let ctx = EvalContext {
            assets: &resolver,
            cancelled: &cancelled,
            budget,
        };
        let cache = CookCache::default();
        let cooked = cook_node_cached(&cache, &graph(hash.clone()), NodeId(1), 0, 1, &ctx);
        assert_eq!(cooked.map(|g| g.hash()).ok(), Some(geo.hash()));
        // A different hash is a different cache entry and must not reuse the first result.
        let other = "f".repeat(64);
        assert_eq!(
            cook_node_cached(&cache, &graph(other), NodeId(1), 0, 1, &ctx).err(),
            Some(EvalError::Asset(AssetError::NotFound))
        );
        // Without a resolver, a scene with assets fails with a clear error.
        let no_assets = EvalContext {
            assets: &NoAssets,
            cancelled: &cancelled,
            budget,
        };
        assert_eq!(
            cook(&graph(hash.clone()), &no_assets).err(),
            Some(EvalError::Asset(AssetError::NotFound))
        );
        // Budgets apply to imported geometry as they do to generated geometry.
        let tight = EvalContext {
            assets: &resolver,
            cancelled: &cancelled,
            budget: CookBudget {
                max_memory_bytes: 1,
                max_elements: 1000,
            },
        };
        // Cooks the asset node alone, so that no later node can be what enforces the limit.
        assert_eq!(
            cook_node_cached(&CookCache::default(), &graph(hash), NodeId(1), 0, 1, &tight).err(),
            Some(EvalError::Resource(GeoError::LimitExceeded))
        );
        Ok(())
    }

    /// Moves every point up by `height`, reading only its definition's parameters.
    fn cook_lift(
        node: &Node,
        inputs: &[Arc<GeoDetail>],
        ctx: &EvalContext<'_>,
    ) -> Result<GeoDetail, EvalError> {
        let height = param_f32(node, "height")?;
        let offset = Vec3::checked(0.0, height, 0.0).map_err(|_| EvalError::InvalidGraph)?;
        let input = inputs.first().ok_or(EvalError::InvalidGraph)?;
        input
            .translated_cancellable(offset, ctx.budget.max_memory_bytes, ctx.cancelled)
            .map_err(EvalError::Resource)
    }

    static LIFT_INPUTS: [pcc_graph::NodeInput; 1] = [pcc_graph::NodeInput {
        name: "input",
        description: "Geometry to lift.",
    }];
    static LIFT_PARAMS: [pcc_graph::NodeParam; 1] = [pcc_graph::NodeParam {
        name: "height",
        kind: pcc_graph::ParamKind::F32,
        check: pcc_graph::Check::Any,
        description: "Distance to move up.",
    }];

    #[test]
    fn a_runtime_operator_cooks_and_caches_like_a_built_in() -> Result<(), String> {
        let def = NodeDef::new(
            "test:lift",
            "deform",
            "Moves points up, defined entirely outside the built-in table.",
            &LIFT_INPUTS,
            &LIFT_PARAMS,
        );
        assert_eq!(register_operator(def, cook_lift), Ok(()));
        assert_eq!(
            register_operator(def, cook_lift),
            Err(NodeRegistrationError::AlreadyDefined)
        );
        let cancelled = AtomicBool::new(false);
        let ctx = sop_ctx(&cancelled);
        let cache = CookCache::default();
        let scene = |height: f32| GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("create", NodeId(1)).with_u32("count", 2),
                Node::new("test:lift", NodeId(2))
                    .with_input(NodeId(1))
                    .with_f32("height", height),
                // A built-in downstream of a custom operator.
                Node::new("normal", NodeId(3)).with_input(NodeId(2)),
            ],
        };
        let lifted = cook_at(&cache, &scene(2.5), 3, &ctx)?;
        assert_eq!(lifted.point(1).map(|p| (p.x, p.y)), Some((1.0, 2.5)));
        let hits = cache.node_cache_hits();
        let again = cook_at(&cache, &scene(2.5), 3, &ctx)?;
        assert_eq!(again.hash(), lifted.hash());
        assert_eq!(
            cache.node_cache_hits(),
            hits + 3,
            "all three nodes must hit"
        );
        // A parameter change invalidates the entry, as it does for a built-in.
        let higher = cook_at(&cache, &scene(9.0), 3, &ctx)?;
        assert_eq!(higher.point(0).map(|p| p.y), Some(9.0));
        Ok(())
    }

    #[test]
    fn cache_keys_ignore_ids_except_for_operators_seeded_by_their_id() {
        let key = |node: &Node| CookCache::node_key(node, &[], 0, 1);
        let box_at = |id| Node::new("box", NodeId(id)).with_vec3("size", v3(1.0, 1.0, 1.0));
        assert_eq!(
            key(&box_at(1)),
            key(&box_at(2)),
            "same work must share a cache entry"
        );
        let scatter_at = |id| {
            Node::new("scatter", NodeId(id))
                .with_input(NodeId(9))
                .with_u32("count", 5)
                .with_u64("seed", 1)
        };
        assert_ne!(
            key(&scatter_at(1)),
            key(&scatter_at(2)),
            "its output depends on its id"
        );
        // Parameters and their values both matter, and negative zero equals zero.
        let sized = |x: f32| Node::new("box", NodeId(1)).with_vec3("size", v3(x, 1.0, 1.0));
        assert_ne!(key(&sized(1.0)), key(&sized(2.0)));
        let offset = |x: f32| {
            Node::new("transform", NodeId(1))
                .with_input(NodeId(9))
                .with_vec3("offset", Vec3 { x, y: 0.0, z: 0.0 })
        };
        assert_eq!(key(&offset(0.0)), key(&offset(-0.0)));
        // Operators with the same parameters are still different operators.
        let grid = Node::new("grid", NodeId(1))
            .with_u32("rows", 2)
            .with_u32("cols", 2)
            .with_f32("size_x", 1.0)
            .with_f32("size_z", 1.0);
        let field = Node::new("heightfield", NodeId(1))
            .with_u32("rows", 2)
            .with_u32("cols", 2)
            .with_f32("size_x", 1.0)
            .with_f32("size_z", 1.0);
        assert_ne!(key(&grid), key(&field));
    }

    #[test]
    fn unregistered_operators_cannot_cook() {
        let cancelled = AtomicBool::new(false);
        let ctx = sop_ctx(&cancelled);
        let scene = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::new("create", NodeId(1)).with_u32("count", 2),
                Node::new("test:never_registered", NodeId(2)).with_input(NodeId(1)),
            ],
        };
        assert_eq!(cook(&scene, &ctx).err(), Some(EvalError::InvalidGraph));
    }
}
