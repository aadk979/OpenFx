//! Protocol-neutral MCP-shaped request/response values; transport follows after the core is proven.
use pcc_cmd::{CommandError, EngineCommand, execute};
mod exchange;

pub use exchange::{AssetStore, FileError, MeshFormat, Workspace};

use pcc_eval::{
    AssetError, CookBudget, CookCache, EvalContext, EvalError, cook_cached, cook_node_cached,
    cook_with_progress,
};
use pcc_geo::{GeoDetail, GeoError};
use pcc_graph::{
    GraphEdit, GraphSnapshot, NodeTypeSpec, SceneError, inspect_node_type, search_node_catalog,
};
use pcc_id::{JobId, NodeId, RevisionId, TransactionId};
use pcc_math::Vec3;
use pcc_store::{SceneStore, StoreError};
pub use pcc_vex::{VexCheckReport, VexDiagnostic, check_vex};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

pub const DEFAULT_MAX_COOK_ELEMENTS: u32 = 10_000_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeoBounds {
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeoProbe {
    pub revision: RevisionId,
    pub point_count: usize,
    pub prim_count: usize,
    pub vertex_count: usize,
    pub surface_area: f32,
    pub enclosed_volume: f32,
    pub is_manifold: bool,
    pub is_closed_manifold: bool,
    pub point_attributes: Vec<String>,
    pub hash: String,
    pub bounds: Option<(Vec3, Vec3)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeoImportResponse {
    /// Pass this to an `asset` node to use the geometry in a scene.
    pub content_hash: String,
    pub point_count: usize,
    pub prim_count: usize,
    pub vertex_count: usize,
    pub bounds: Option<(Vec3, Vec3)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GeoExportResponse {
    /// The scene revision that was cooked.
    pub revision: RevisionId,
    pub format: String,
    pub bytes: usize,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceSceneResponse {
    pub committed_revision: RevisionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeCatalogSearchResponse {
    pub node_types: Vec<NodeTypeSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeoInvariantSpec {
    #[serde(default)]
    pub min_points: Option<usize>,
    #[serde(default)]
    pub max_points: Option<usize>,
    #[serde(default)]
    pub min_prims: Option<usize>,
    #[serde(default)]
    pub max_prims: Option<usize>,
    #[serde(default)]
    pub expected_bounds_min: Option<Vec3>,
    #[serde(default)]
    pub expected_bounds_max: Option<Vec3>,
    #[serde(default)]
    pub bounds_tolerance: Option<f32>,
    #[serde(default)]
    pub require_manifold: Option<bool>,
    #[serde(default)]
    pub require_closed_manifold: Option<bool>,
    #[serde(default)]
    pub expected_volume: Option<f32>,
    #[serde(default)]
    pub volume_tolerance: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeoInvariantReport {
    pub passed: bool,
    pub probe: GeoProbe,
    pub violations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CookJobBudget {
    pub max_memory_bytes: usize,
    pub max_elements: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CookJobState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CookJobSnapshot {
    pub job_id: JobId,
    pub revision: RevisionId,
    pub frame_numerator: i64,
    pub frame_denominator: u32,
    pub state: CookJobState,
    pub completed_steps: u32,
    pub total_steps: u32,
    pub effective_budget: CookJobBudget,
    pub stale: bool,
    pub cache_published: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<GeoProbe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelJobResponse {
    pub job_id: JobId,
    pub cancelled: bool,
    pub state: CookJobState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TxSessionResponse {
    pub tx_id: TransactionId,
    pub base_revision: RevisionId,
    pub node_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TxRollbackResponse {
    pub tx_id: TransactionId,
    pub rolled_back: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCode {
    RevisionConflict,
    InvalidFrameDenominator,
    SceneHeaderInvalid,
    SceneFieldInvalid,
    SceneNodeOrderInvalid,
    NonFiniteInput,
    InvalidGraph,
    CookCancelled,
    ElementLimitExceeded,
    MemoryLimitExceeded,
    IndexOutOfBounds,
    StorePoisoned,
    IoFailure,
    JobNotFound,
    NothingToUndo,
    NothingToRedo,
    NodeTypeNotFound,
    NodeNotFound,
    TransactionNotFound,
    VexCompileOrRuntimeError,
    FileAccessDisabled,
    PathRejected,
    FileNotFound,
    FileAlreadyExists,
    UnsupportedFileFormat,
    MeshParseFailed,
    AssetNotFound,
    AssetCorrupt,
    ParamEvaluationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolErrorPayload {
    pub code: ToolErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<RevisionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<RevisionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_elements: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_elements: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<JobId>,
}

#[must_use]
pub fn geo_probe(revision: RevisionId, geo: &GeoDetail) -> GeoProbe {
    let topo = geo.topology_index();
    let point_attributes = geo
        .attribute_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    GeoProbe {
        revision,
        point_count: geo.len(),
        prim_count: geo.prim_count(),
        vertex_count: geo.vertex_count(),
        surface_area: geo.surface_area(),
        enclosed_volume: geo.enclosed_volume(),
        is_manifold: topo.is_manifold(),
        is_closed_manifold: topo.is_closed_manifold(),
        point_attributes,
        hash: geo.hash().to_hex().to_string(),
        bounds: geo.bounds(),
    }
}

struct JobOutcome {
    state: CookJobState,
    stale: bool,
    cache_published: bool,
    probe: Option<GeoProbe>,
    error: Option<ToolErrorPayload>,
}

struct JobRecord {
    job_id: JobId,
    snapshot: Arc<GraphSnapshot>,
    frame_numerator: i64,
    frame_denominator: u32,
    effective_budget: CookBudget,
    cancelled: Arc<AtomicBool>,
    completed_steps: AtomicU32,
    total_steps: u32,
    outcome: Mutex<JobOutcome>,
}

struct SandboxRecord {
    base_revision: RevisionId,
    staged_snapshot: GraphSnapshot,
}

/// Engine-facing root service. A future `rmcp` transport only adapts requests to this type.
pub struct RootService {
    store: SceneStore,
    cache: Arc<CookCache>,
    max_cook_memory_bytes: usize,
    max_cook_elements: u32,
    next_job_id: AtomicU64,
    jobs: Mutex<BTreeMap<JobId, Arc<JobRecord>>>,
    next_tx_id: AtomicU64,
    sandboxes: Mutex<BTreeMap<TransactionId, SandboxRecord>>,
    assets: AssetStore,
}
#[derive(Debug)]
pub enum McpError {
    File(FileError),
    Command(CommandError),
    Store(StoreError),
    Cook(EvalError),
    Io(io::Error),
    Scene(SceneError),
    InvalidFrameDenominator,
    JobNotFound(JobId),
    NodeTypeNotFound(String),
    TransactionNotFound(TransactionId),
}

impl McpError {
    #[must_use]
    pub fn to_tool_error(&self, max_memory_bytes: usize) -> ToolErrorPayload {
        let plain = |code: ToolErrorCode, message: String| ToolErrorPayload {
            code,
            message,
            expected_revision: None,
            actual_revision: None,
            requested_elements: None,
            max_elements: None,
            max_memory_bytes: None,
            job_id: None,
        };
        match self {
            Self::File(error) => {
                let message = error.message();
                match error {
                    FileError::Disabled => plain(ToolErrorCode::FileAccessDisabled, message),
                    FileError::PathRejected(_) => plain(ToolErrorCode::PathRejected, message),
                    FileError::NotFound => plain(ToolErrorCode::FileNotFound, message),
                    FileError::AlreadyExists => plain(ToolErrorCode::FileAlreadyExists, message),
                    FileError::UnsupportedFormat => {
                        plain(ToolErrorCode::UnsupportedFileFormat, message)
                    }
                    FileError::Parse(_) => plain(ToolErrorCode::MeshParseFailed, message),
                    FileError::TooLarge => ToolErrorPayload {
                        max_memory_bytes: Some(max_memory_bytes),
                        ..plain(ToolErrorCode::MemoryLimitExceeded, message)
                    },
                    FileError::Io(_) => plain(ToolErrorCode::IoFailure, message),
                }
            }
            Self::Command(CommandError::Conflict { expected, actual })
            | Self::Store(StoreError::Conflict { expected, actual }) => ToolErrorPayload {
                code: ToolErrorCode::RevisionConflict,
                message: format!("revision conflict: expected {expected}, actual {actual}"),
                expected_revision: Some(*expected),
                actual_revision: Some(*actual),
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::Store(StoreError::NothingToUndo) => ToolErrorPayload {
                code: ToolErrorCode::NothingToUndo,
                message: "undo stack is empty".to_owned(),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::Store(StoreError::NothingToRedo) => ToolErrorPayload {
                code: ToolErrorCode::NothingToRedo,
                message: "redo stack is empty".to_owned(),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::Command(CommandError::Scene(scene_err)) | Self::Scene(scene_err) => {
                let (code, message) = match scene_err {
                    SceneError::Header => (
                        ToolErrorCode::SceneHeaderInvalid,
                        "scene header must be `ofxs 1`".to_owned(),
                    ),
                    SceneError::Field => (
                        ToolErrorCode::SceneFieldInvalid,
                        "scene contains malformed field or syntax".to_owned(),
                    ),
                    SceneError::NodeOrder => (
                        ToolErrorCode::SceneNodeOrderInvalid,
                        "scene node ordering or topology reference is invalid".to_owned(),
                    ),
                    SceneError::NonFinite => (
                        ToolErrorCode::NonFiniteInput,
                        "scene contains non-finite (NaN or infinite) numeric values".to_owned(),
                    ),
                };
                ToolErrorPayload {
                    code,
                    message,
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: None,
                    job_id: None,
                }
            }
            Self::Store(StoreError::Poisoned) => ToolErrorPayload {
                code: ToolErrorCode::StorePoisoned,
                message: "scene store lock is poisoned".to_owned(),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::Cook(eval_err) => match eval_err {
                EvalError::Param {
                    node,
                    param,
                    message,
                } => plain(
                    ToolErrorCode::ParamEvaluationFailed,
                    format!("node {} parameter `{param}`: {message}", node.0),
                ),
                EvalError::Asset(AssetError::NotFound) => plain(
                    ToolErrorCode::AssetNotFound,
                    "the scene refers to an asset that was never imported into this workspace"
                        .to_owned(),
                ),
                EvalError::Asset(AssetError::Corrupt) => plain(
                    ToolErrorCode::AssetCorrupt,
                    "a stored asset does not match its content hash".to_owned(),
                ),
                EvalError::Cancelled | EvalError::Resource(GeoError::Cancelled) => {
                    ToolErrorPayload {
                        code: ToolErrorCode::CookCancelled,
                        message: "cook was cooperatively cancelled".to_owned(),
                        expected_revision: None,
                        actual_revision: None,
                        requested_elements: None,
                        max_elements: None,
                        max_memory_bytes: None,
                        job_id: None,
                    }
                }
                EvalError::InvalidGraph => ToolErrorPayload {
                    code: ToolErrorCode::InvalidGraph,
                    message: "graph snapshot failed cook validation".to_owned(),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: None,
                    job_id: None,
                },
                EvalError::ElementLimit { requested, limit } => ToolErrorPayload {
                    code: ToolErrorCode::ElementLimitExceeded,
                    message: format!(
                        "requested element count {requested} exceeds budget limit {limit}"
                    ),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: Some(*requested),
                    max_elements: Some(*limit),
                    max_memory_bytes: None,
                    job_id: None,
                },
                EvalError::Resource(GeoError::LimitExceeded) => ToolErrorPayload {
                    code: ToolErrorCode::MemoryLimitExceeded,
                    message: format!("cook exceeded memory budget of {max_memory_bytes} bytes"),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: Some(max_memory_bytes),
                    job_id: None,
                },
                EvalError::Resource(GeoError::NonFinite) => ToolErrorPayload {
                    code: ToolErrorCode::NonFiniteInput,
                    message: "cook produced non-finite geometry values".to_owned(),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: None,
                    job_id: None,
                },
                EvalError::Resource(GeoError::IndexOutOfBounds) => ToolErrorPayload {
                    code: ToolErrorCode::IndexOutOfBounds,
                    message: "geometry index out of bounds during cook".to_owned(),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: None,
                    job_id: None,
                },
                EvalError::NodeNotFound(node_id) => ToolErrorPayload {
                    code: ToolErrorCode::NodeNotFound,
                    message: format!("node {node_id} was not found in graph snapshot"),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: None,
                    job_id: None,
                },
                EvalError::Vex(vex_err) => {
                    let diag = vex_err.diagnostic();
                    ToolErrorPayload {
                        code: ToolErrorCode::VexCompileOrRuntimeError,
                        message: format!(
                            "VEX error at line {}, column {}: {}",
                            diag.line, diag.column, diag.message
                        ),
                        expected_revision: None,
                        actual_revision: None,
                        requested_elements: None,
                        max_elements: None,
                        max_memory_bytes: None,
                        job_id: None,
                    }
                }
                EvalError::Resource(
                    GeoError::AttributeLengthMismatch { .. }
                    | GeoError::AttributeNotFound
                    | GeoError::AttributeTypeMismatch
                    | GeoError::InvalidAttributeName
                    | GeoError::InvalidTopology
                    | GeoError::ReservedAttributeName,
                ) => ToolErrorPayload {
                    code: ToolErrorCode::InvalidGraph,
                    message: "geometry attribute or topology validation failed during cook"
                        .to_owned(),
                    expected_revision: None,
                    actual_revision: None,
                    requested_elements: None,
                    max_elements: None,
                    max_memory_bytes: None,
                    job_id: None,
                },
            },
            Self::Io(io_err) => ToolErrorPayload {
                code: ToolErrorCode::IoFailure,
                message: format!("io error: {io_err}"),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::InvalidFrameDenominator => ToolErrorPayload {
                code: ToolErrorCode::InvalidFrameDenominator,
                message: "frame_denominator must be non-zero".to_owned(),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::JobNotFound(job_id) => ToolErrorPayload {
                code: ToolErrorCode::JobNotFound,
                message: format!("cook job {job_id} was not found"),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: Some(*job_id),
            },
            Self::NodeTypeNotFound(name) => ToolErrorPayload {
                code: ToolErrorCode::NodeTypeNotFound,
                message: format!("node type `{name}` was not found in catalog"),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
            Self::TransactionNotFound(tx_id) => ToolErrorPayload {
                code: ToolErrorCode::TransactionNotFound,
                message: format!("sandbox transaction {tx_id} was not found"),
                expected_revision: None,
                actual_revision: None,
                requested_elements: None,
                max_elements: None,
                max_memory_bytes: None,
                job_id: None,
            },
        }
    }
}

impl RootService {
    #[must_use]
    pub fn new(initial: GraphSnapshot, max_cook_memory_bytes: usize) -> Self {
        Self::with_limits(initial, max_cook_memory_bytes, DEFAULT_MAX_COOK_ELEMENTS)
    }
    #[must_use]
    pub fn with_limits(
        initial: GraphSnapshot,
        max_cook_memory_bytes: usize,
        max_cook_elements: u32,
    ) -> Self {
        let cache = CookCache::default();
        cache.note_published_revision(initial.revision);
        Self {
            store: SceneStore::new(initial),
            cache: Arc::new(cache),
            max_cook_memory_bytes,
            max_cook_elements,
            next_job_id: AtomicU64::new(1),
            jobs: Mutex::new(BTreeMap::new()),
            next_tx_id: AtomicU64::new(1),
            sandboxes: Mutex::new(BTreeMap::new()),
            assets: AssetStore::new(None, max_cook_memory_bytes),
        }
    }

    /// Enables `geo_import` and `geo_export`, confined to this existing directory. File access is
    /// otherwise off, because paths come from agents.
    pub fn with_workspace(mut self, root: &Path) -> Result<Self, McpError> {
        let workspace = Workspace::open(root).map_err(McpError::File)?;
        self.assets = AssetStore::new(Some(workspace), self.max_cook_memory_bytes);
        Ok(self)
    }

    /// Reads a mesh from the workspace into the content-addressed asset store. This does not change
    /// the scene: add an `asset` node with the returned hash through the usual edit commands.
    pub fn geo_import(&self, path: &str) -> Result<GeoImportResponse, McpError> {
        let (content_hash, geo) = self.assets.import(path).map_err(McpError::File)?;
        Ok(GeoImportResponse {
            content_hash,
            point_count: geo.len(),
            prim_count: geo.prim_count(),
            vertex_count: geo.vertex_count(),
            bounds: geo.bounds(),
        })
    }

    /// Cooks a node, or the scene's final node when none is named, and writes it to the workspace.
    /// The format follows the path's extension: `.obj`, `.stl`, or `.ofxg`.
    pub fn geo_export(
        &self,
        path: &str,
        node_id: Option<NodeId>,
        overwrite: bool,
        frame_numerator: i64,
        frame_denominator: u32,
    ) -> Result<GeoExportResponse, McpError> {
        if frame_denominator == 0 {
            return Err(McpError::InvalidFrameDenominator);
        }
        let snapshot = self.store.snapshot().map_err(McpError::Store)?;
        let cancelled = AtomicBool::new(false);
        let context = EvalContext {
            assets: &self.assets,
            cancelled: &cancelled,
            budget: self.default_budget(),
        };
        let geo = match node_id {
            Some(id) => cook_node_cached(
                &self.cache,
                &snapshot,
                id,
                frame_numerator,
                frame_denominator,
                &context,
            ),
            None => cook_cached(
                &self.cache,
                &snapshot,
                frame_numerator,
                frame_denominator,
                &context,
            ),
        }
        .map_err(McpError::Cook)?;
        let (format, bytes) = self
            .assets
            .export(&geo, path, overwrite)
            .map_err(McpError::File)?;
        Ok(GeoExportResponse {
            revision: snapshot.revision,
            format: format.as_str().to_owned(),
            bytes,
            content_hash: geo.hash().to_hex().to_string(),
        })
    }
    #[must_use]
    pub const fn max_cook_memory_bytes(&self) -> usize {
        self.max_cook_memory_bytes
    }
    #[must_use]
    pub const fn max_cook_elements(&self) -> u32 {
        self.max_cook_elements
    }
    #[must_use]
    pub const fn default_budget(&self) -> CookBudget {
        CookBudget {
            max_memory_bytes: self.max_cook_memory_bytes,
            max_elements: self.max_cook_elements,
        }
    }
    fn on_revision_published(&self, committed: RevisionId) {
        self.cache.note_published_revision(committed);
        if let Ok(jobs) = self.jobs.lock() {
            for record in jobs.values() {
                if record.snapshot.revision < committed {
                    record.cancelled.store(true, Ordering::Release);
                    if let Ok(mut outcome) = record.outcome.lock() {
                        outcome.stale = true;
                        if matches!(outcome.state, CookJobState::Queued | CookJobState::Running) {
                            outcome.state = CookJobState::Cancelled;
                            outcome.error = Some(
                                McpError::Cook(EvalError::Cancelled)
                                    .to_tool_error(record.effective_budget.max_memory_bytes),
                            );
                        }
                    }
                }
            }
        }
    }
    pub fn replace_scene(&self, base: RevisionId, ofxs: &str) -> Result<RevisionId, McpError> {
        let current = self.store.snapshot().map_err(McpError::Store)?;
        let next = execute(&current, EngineCommand::ReplaceScene { base, ofxs })
            .map_err(McpError::Command)?;
        let committed = self
            .store
            .commit(
                &pcc_store::Transaction {
                    base_revision: base,
                    read_set: Arc::from([base]),
                },
                next,
            )
            .map(|snapshot| snapshot.revision)
            .map_err(McpError::Store)?;
        self.on_revision_published(committed);
        Ok(committed)
    }
    pub fn batch_edit(
        &self,
        base: RevisionId,
        edits: &[GraphEdit],
    ) -> Result<RevisionId, McpError> {
        let current = self.store.snapshot().map_err(McpError::Store)?;
        let next = execute(&current, EngineCommand::BatchEdit { base, edits })
            .map_err(McpError::Command)?;
        let committed = self
            .store
            .commit(
                &pcc_store::Transaction {
                    base_revision: base,
                    read_set: Arc::from([base]),
                },
                next,
            )
            .map(|snapshot| snapshot.revision)
            .map_err(McpError::Store)?;
        self.on_revision_published(committed);
        Ok(committed)
    }
    pub fn undo(&self, expected: RevisionId) -> Result<RevisionId, McpError> {
        let committed = self
            .store
            .undo(expected)
            .map(|snapshot| snapshot.revision)
            .map_err(McpError::Store)?;
        self.on_revision_published(committed);
        Ok(committed)
    }
    pub fn redo(&self, expected: RevisionId) -> Result<RevisionId, McpError> {
        let committed = self
            .store
            .redo(expected)
            .map(|snapshot| snapshot.revision)
            .map_err(McpError::Store)?;
        self.on_revision_published(committed);
        Ok(committed)
    }
    #[must_use]
    pub fn search_node_catalog(&self, query: &str) -> Vec<NodeTypeSpec> {
        search_node_catalog(query)
    }
    pub fn inspect_node_type(&self, type_name: &str) -> Result<NodeTypeSpec, McpError> {
        inspect_node_type(type_name).ok_or_else(|| McpError::NodeTypeNotFound(type_name.to_owned()))
    }
    pub fn probe(
        &self,
        frame_numerator: i64,
        frame_denominator: u32,
    ) -> Result<GeoProbe, McpError> {
        if frame_denominator == 0 {
            return Err(McpError::InvalidFrameDenominator);
        }
        let snapshot = self.store.snapshot().map_err(McpError::Store)?;
        let cancelled = AtomicBool::new(false);
        let context = EvalContext {
            assets: &self.assets,
            cancelled: &cancelled,
            budget: self.default_budget(),
        };
        let geo = cook_cached(
            &self.cache,
            &snapshot,
            frame_numerator,
            frame_denominator,
            &context,
        )
        .map_err(McpError::Cook)?;
        Ok(geo_probe(snapshot.revision, &geo))
    }

    pub fn probe_node(
        &self,
        node_id: NodeId,
        frame_numerator: i64,
        frame_denominator: u32,
    ) -> Result<GeoProbe, McpError> {
        if frame_denominator == 0 {
            return Err(McpError::InvalidFrameDenominator);
        }
        let snapshot = self.store.snapshot().map_err(McpError::Store)?;
        let cancelled = AtomicBool::new(false);
        let context = EvalContext {
            assets: &self.assets,
            cancelled: &cancelled,
            budget: self.default_budget(),
        };
        let geo = cook_node_cached(
            &self.cache,
            &snapshot,
            node_id,
            frame_numerator,
            frame_denominator,
            &context,
        )
        .map_err(McpError::Cook)?;
        Ok(geo_probe(snapshot.revision, &geo))
    }

    /// Forks the current immutable scene snapshot into an isolated Copy-on-Write sandbox transaction.
    pub fn tx_begin(&self) -> Result<TxSessionResponse, McpError> {
        let current = self.store.snapshot().map_err(McpError::Store)?;
        let tx_id = TransactionId(self.next_tx_id.fetch_add(1, Ordering::Relaxed));
        let base_revision = current.revision;
        let node_count = current.nodes.len();
        let record = SandboxRecord {
            base_revision,
            staged_snapshot: (*current).clone(),
        };
        let mut sandboxes = self
            .sandboxes
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        sandboxes.insert(tx_id, record);
        Ok(TxSessionResponse {
            tx_id,
            base_revision,
            node_count,
        })
    }

    /// Stages a batch of graph edits inside an isolated sandbox transaction without mutating the live store.
    pub fn tx_stage_edits(
        &self,
        tx_id: TransactionId,
        edits: &[GraphEdit],
    ) -> Result<TxSessionResponse, McpError> {
        let mut sandboxes = self
            .sandboxes
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        let record = sandboxes
            .get_mut(&tx_id)
            .ok_or(McpError::TransactionNotFound(tx_id))?;
        let next = record
            .staged_snapshot
            .apply_edits(record.base_revision, edits)
            .map_err(McpError::Scene)?;
        let node_count = next.nodes.len();
        let base_revision = record.base_revision;
        record.staged_snapshot = next;
        Ok(TxSessionResponse {
            tx_id,
            base_revision,
            node_count,
        })
    }

    /// Cooks and probes the staged graph inside a sandbox transaction without publishing to the live store.
    pub fn tx_probe(
        &self,
        tx_id: TransactionId,
        frame_numerator: i64,
        frame_denominator: u32,
    ) -> Result<GeoProbe, McpError> {
        if frame_denominator == 0 {
            return Err(McpError::InvalidFrameDenominator);
        }
        let staged = {
            let sandboxes = self
                .sandboxes
                .lock()
                .map_err(|_| McpError::Store(StoreError::Poisoned))?;
            let record = sandboxes
                .get(&tx_id)
                .ok_or(McpError::TransactionNotFound(tx_id))?;
            record.staged_snapshot.clone()
        };
        let cancelled = AtomicBool::new(false);
        let context = EvalContext {
            assets: &self.assets,
            cancelled: &cancelled,
            budget: self.default_budget(),
        };
        let geo = if let Some(last) = staged.nodes.last() {
            cook_node_cached(
                &self.cache,
                &staged,
                last.id(),
                frame_numerator,
                frame_denominator,
                &context,
            )
            .map_err(McpError::Cook)?
        } else {
            Arc::new(GeoDetail::points(Vec::new()))
        };
        Ok(geo_probe(staged.revision, &geo))
    }

    /// Atomically commits a sandbox transaction to the live scene store if `base_revision` is still current.
    pub fn tx_commit(&self, tx_id: TransactionId) -> Result<RevisionId, McpError> {
        let record = {
            let mut sandboxes = self
                .sandboxes
                .lock()
                .map_err(|_| McpError::Store(StoreError::Poisoned))?;
            sandboxes
                .remove(&tx_id)
                .ok_or(McpError::TransactionNotFound(tx_id))?
        };
        let committed = self
            .store
            .commit(
                &pcc_store::Transaction {
                    base_revision: record.base_revision,
                    read_set: Arc::from([record.base_revision]),
                },
                record.staged_snapshot,
            )
            .map(|snapshot| snapshot.revision)
            .map_err(McpError::Store)?;
        self.on_revision_published(committed);
        Ok(committed)
    }

    /// Discards an uncommitted sandbox transaction without touching the live scene store.
    pub fn tx_rollback(&self, tx_id: TransactionId) -> Result<TxRollbackResponse, McpError> {
        let mut sandboxes = self
            .sandboxes
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        let removed = sandboxes.remove(&tx_id);
        if removed.is_none() {
            return Err(McpError::TransactionNotFound(tx_id));
        }
        Ok(TxRollbackResponse {
            tx_id,
            rolled_back: true,
        })
    }

    pub fn assert_invariants(
        &self,
        frame_numerator: i64,
        frame_denominator: u32,
        spec: &GeoInvariantSpec,
    ) -> Result<GeoInvariantReport, McpError> {
        if frame_denominator == 0 {
            return Err(McpError::InvalidFrameDenominator);
        }
        let snapshot = self.store.snapshot().map_err(McpError::Store)?;
        let cancelled = AtomicBool::new(false);
        let context = EvalContext {
            assets: &self.assets,
            cancelled: &cancelled,
            budget: self.default_budget(),
        };
        let geo = cook_cached(
            &self.cache,
            &snapshot,
            frame_numerator,
            frame_denominator,
            &context,
        )
        .map_err(McpError::Cook)?;
        let probe = geo_probe(snapshot.revision, &geo);
        let mut violations = Vec::new();
        if let Some(min_pts) = spec.min_points
            && geo.len() < min_pts
        {
            violations.push(format!(
                "point_count {} is below min_points {min_pts}",
                geo.len()
            ));
        }
        if let Some(max_pts) = spec.max_points
            && geo.len() > max_pts
        {
            violations.push(format!(
                "point_count {} exceeds max_points {max_pts}",
                geo.len()
            ));
        }
        if let Some(min_prims) = spec.min_prims
            && geo.prim_count() < min_prims
        {
            violations.push(format!(
                "prim_count {} is below min_prims {min_prims}",
                geo.prim_count()
            ));
        }
        if let Some(max_prims) = spec.max_prims
            && geo.prim_count() > max_prims
        {
            violations.push(format!(
                "prim_count {} exceeds max_prims {max_prims}",
                geo.prim_count()
            ));
        }
        let tol = spec.bounds_tolerance.unwrap_or(1e-4);
        if !tol.is_finite() || tol < 0.0 {
            return Err(McpError::Scene(SceneError::NonFinite));
        }
        if let Some(expected_min) = spec.expected_bounds_min {
            Vec3::checked(expected_min.x, expected_min.y, expected_min.z)
                .map_err(|_| McpError::Scene(SceneError::NonFinite))?;
            if let Some((actual_min, _)) = probe.bounds {
                if (actual_min.x - expected_min.x).abs() > tol
                    || (actual_min.y - expected_min.y).abs() > tol
                    || (actual_min.z - expected_min.z).abs() > tol
                {
                    violations.push(format!(
                        "bounds min ({}, {}, {}) differs from expected ({}, {}, {}) beyond tolerance {tol}",
                        actual_min.x,
                        actual_min.y,
                        actual_min.z,
                        expected_min.x,
                        expected_min.y,
                        expected_min.z
                    ));
                }
            } else {
                violations
                    .push("bounds are empty but expected_bounds_min was specified".to_owned());
            }
        }
        if let Some(expected_max) = spec.expected_bounds_max {
            Vec3::checked(expected_max.x, expected_max.y, expected_max.z)
                .map_err(|_| McpError::Scene(SceneError::NonFinite))?;
            if let Some((_, actual_max)) = probe.bounds {
                if (actual_max.x - expected_max.x).abs() > tol
                    || (actual_max.y - expected_max.y).abs() > tol
                    || (actual_max.z - expected_max.z).abs() > tol
                {
                    violations.push(format!(
                        "bounds max ({}, {}, {}) differs from expected ({}, {}, {}) beyond tolerance {tol}",
                        actual_max.x,
                        actual_max.y,
                        actual_max.z,
                        expected_max.x,
                        expected_max.y,
                        expected_max.z
                    ));
                }
            } else {
                violations
                    .push("bounds are empty but expected_bounds_max was specified".to_owned());
            }
        }
        if spec.require_manifold == Some(true) || spec.require_closed_manifold == Some(true) {
            let topo = geo.topology_index();
            if spec.require_manifold == Some(true) && !topo.is_manifold() {
                violations.push(format!(
                    "mesh is non-manifold ({} non-manifold edges)",
                    topo.non_manifold_edge_count()
                ));
            }
            if spec.require_closed_manifold == Some(true) && !topo.is_closed_manifold() {
                violations.push(format!(
                    "mesh is not a closed manifold ({} boundary edges, {} non-manifold edges)",
                    topo.boundary_edge_count(),
                    topo.non_manifold_edge_count()
                ));
            }
        }
        if let Some(expected_vol) = spec.expected_volume {
            if !expected_vol.is_finite() || expected_vol < 0.0 {
                return Err(McpError::Scene(SceneError::NonFinite));
            }
            let vol_tol = spec.volume_tolerance.unwrap_or(1e-3);
            if !vol_tol.is_finite() || vol_tol < 0.0 {
                return Err(McpError::Scene(SceneError::NonFinite));
            }
            let actual_vol = geo.enclosed_volume();
            if (actual_vol - expected_vol).abs() > vol_tol {
                violations.push(format!(
                    "enclosed_volume {actual_vol} differs from expected {expected_vol} beyond tolerance {vol_tol}"
                ));
            }
        }
        Ok(GeoInvariantReport {
            passed: violations.is_empty(),
            probe,
            violations,
        })
    }

    #[must_use]
    pub fn vex_check(&self, snippet: &str) -> VexCheckReport {
        check_vex(snippet)
    }

    /// Creates a new immutable cook job bound to the current scene snapshot,
    /// strictly clamping any requested budget overrides to the root service's policy limits.
    pub fn enqueue_cook_job(
        &self,
        frame_numerator: i64,
        frame_denominator: u32,
        requested_max_memory_bytes: Option<usize>,
        requested_max_elements: Option<u32>,
    ) -> Result<CookJobSnapshot, McpError> {
        if frame_denominator == 0 {
            return Err(McpError::InvalidFrameDenominator);
        }
        let snapshot = self.store.snapshot().map_err(McpError::Store)?;
        let parent_budget = self.default_budget();
        let candidate_budget = CookBudget {
            max_memory_bytes: requested_max_memory_bytes.unwrap_or(parent_budget.max_memory_bytes),
            max_elements: requested_max_elements.unwrap_or(parent_budget.max_elements),
        };
        let effective_budget = candidate_budget.clamp_to(parent_budget);
        let job_id = JobId(self.next_job_id.fetch_add(1, Ordering::Relaxed));
        let total_steps = snapshot.nodes.len() as u32;
        let record = Arc::new(JobRecord {
            job_id,
            snapshot,
            frame_numerator,
            frame_denominator,
            effective_budget,
            cancelled: Arc::new(AtomicBool::new(false)),
            completed_steps: AtomicU32::new(0),
            total_steps,
            outcome: Mutex::new(JobOutcome {
                state: CookJobState::Queued,
                stale: false,
                cache_published: false,
                probe: None,
                error: None,
            }),
        });
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        jobs.insert(job_id, Arc::clone(&record));
        drop(jobs);
        self.snapshot_from_record(&record)
    }

    /// Enqueues and immediately executes a cook job to completion (or cancellation/limit error).
    pub fn start_cook_job(
        &self,
        frame_numerator: i64,
        frame_denominator: u32,
        requested_max_memory_bytes: Option<usize>,
        requested_max_elements: Option<u32>,
    ) -> Result<CookJobSnapshot, McpError> {
        let created = self.enqueue_cook_job(
            frame_numerator,
            frame_denominator,
            requested_max_memory_bytes,
            requested_max_elements,
        )?;
        self.run_cook_job(created.job_id)
    }

    /// Executes an enqueued cook job against its captured immutable snapshot.
    /// If a newer scene revision has been published before or during execution,
    /// the job is marked stale and will never overwrite the shared cook cache.
    pub fn run_cook_job(&self, job_id: JobId) -> Result<CookJobSnapshot, McpError> {
        let record = self.find_job_record(job_id)?;
        {
            let mut outcome = record
                .outcome
                .lock()
                .map_err(|_| McpError::Store(StoreError::Poisoned))?;
            if matches!(
                outcome.state,
                CookJobState::Completed | CookJobState::Cancelled | CookJobState::Failed
            ) {
                drop(outcome);
                return self.snapshot_from_record(&record);
            }
            if record.cancelled.load(Ordering::Acquire) {
                outcome.state = CookJobState::Cancelled;
                outcome.error = Some(
                    McpError::Cook(EvalError::Cancelled)
                        .to_tool_error(record.effective_budget.max_memory_bytes),
                );
                drop(outcome);
                return self.snapshot_from_record(&record);
            }
            outcome.state = CookJobState::Running;
        }

        let key = CookCache::key(
            &record.snapshot,
            record.frame_numerator,
            record.frame_denominator,
        );
        let ctx = EvalContext {
            assets: &self.assets,
            cancelled: &record.cancelled,
            budget: record.effective_budget,
        };

        let result = if let Some(cached) = self.cache.get(&key) {
            record
                .completed_steps
                .store(record.total_steps, Ordering::Release);
            Ok((cached, true))
        } else {
            cook_with_progress(&record.snapshot, &ctx, |done, _total| {
                record.completed_steps.store(done, Ordering::Release);
            })
            .map(|geo| (Arc::new(geo), false))
        };

        let current_rev = self
            .store
            .snapshot()
            .map(|s| s.revision)
            .unwrap_or(record.snapshot.revision);
        let is_stale_revision = current_rev > record.snapshot.revision;

        let mut outcome = record
            .outcome
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        outcome.stale = outcome.stale || is_stale_revision;

        match result {
            Ok((geo, from_cache)) => {
                if record.cancelled.load(Ordering::Acquire) {
                    outcome.state = CookJobState::Cancelled;
                    outcome.cache_published = false;
                    outcome.error = Some(
                        McpError::Cook(EvalError::Cancelled)
                            .to_tool_error(record.effective_budget.max_memory_bytes),
                    );
                } else {
                    let published = if outcome.stale {
                        false
                    } else if from_cache {
                        true
                    } else {
                        self.cache
                            .insert_if_fresh(key, record.snapshot.revision, Arc::clone(&geo))
                    };
                    if !published && !from_cache {
                        outcome.stale = true;
                    }
                    outcome.state = CookJobState::Completed;
                    outcome.cache_published = published;
                    outcome.probe = Some(geo_probe(record.snapshot.revision, &geo));
                    outcome.error = None;
                }
            }
            Err(eval_err) => {
                outcome.cache_published = false;
                outcome.probe = None;
                outcome.state = match eval_err {
                    EvalError::Cancelled | EvalError::Resource(GeoError::Cancelled) => {
                        CookJobState::Cancelled
                    }
                    _ => CookJobState::Failed,
                };
                outcome.error = Some(
                    McpError::Cook(eval_err)
                        .to_tool_error(record.effective_budget.max_memory_bytes),
                );
            }
        }
        drop(outcome);
        self.snapshot_from_record(&record)
    }

    pub fn cancel_cook_job(&self, job_id: JobId) -> Result<CancelJobResponse, McpError> {
        let record = self.find_job_record(job_id)?;
        record.cancelled.store(true, Ordering::Release);
        let mut outcome = record
            .outcome
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        let cancelled = match outcome.state {
            CookJobState::Queued | CookJobState::Running => {
                outcome.state = CookJobState::Cancelled;
                outcome.error = Some(
                    McpError::Cook(EvalError::Cancelled)
                        .to_tool_error(record.effective_budget.max_memory_bytes),
                );
                true
            }
            CookJobState::Cancelled => true,
            CookJobState::Completed | CookJobState::Failed => false,
        };
        Ok(CancelJobResponse {
            job_id,
            cancelled,
            state: outcome.state,
        })
    }

    pub fn cook_job_status(&self, job_id: JobId) -> Result<CookJobSnapshot, McpError> {
        let record = self.find_job_record(job_id)?;
        self.snapshot_from_record(&record)
    }

    fn find_job_record(&self, job_id: JobId) -> Result<Arc<JobRecord>, McpError> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        jobs.get(&job_id)
            .cloned()
            .ok_or(McpError::JobNotFound(job_id))
    }

    fn snapshot_from_record(&self, record: &JobRecord) -> Result<CookJobSnapshot, McpError> {
        let current_rev = self
            .store
            .snapshot()
            .map(|s| s.revision)
            .unwrap_or(record.snapshot.revision);
        let outcome = record
            .outcome
            .lock()
            .map_err(|_| McpError::Store(StoreError::Poisoned))?;
        Ok(CookJobSnapshot {
            job_id: record.job_id,
            revision: record.snapshot.revision,
            frame_numerator: record.frame_numerator,
            frame_denominator: record.frame_denominator,
            state: outcome.state,
            completed_steps: record.completed_steps.load(Ordering::Acquire),
            total_steps: record.total_steps,
            effective_budget: CookJobBudget {
                max_memory_bytes: record.effective_budget.max_memory_bytes,
                max_elements: record.effective_budget.max_elements,
            },
            stale: outcome.stale || current_rev > record.snapshot.revision,
            cache_published: outcome.cache_published,
            probe: outcome.probe.clone(),
            error: outcome.error.clone(),
        })
    }

    /// Writes a complete new scene then atomically publishes it at an unused destination.
    pub fn save_new(&self, destination: &Path) -> Result<(), McpError> {
        let snapshot = self.store.snapshot().map_err(McpError::Store)?;
        let temporary = destination.with_extension("ofxs.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(McpError::Io)?;
        let result = (|| -> Result<(), io::Error> {
            file.write_all(snapshot.to_ofxs().as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(McpError::Io)
    }
    pub fn load(path: &Path, max_cook_memory_bytes: usize) -> Result<Self, McpError> {
        let text = fs::read_to_string(path).map_err(McpError::Io)?;
        let snapshot = GraphSnapshot::from_ofxs(&text).map_err(McpError::Scene)?;
        Ok(Self::new(snapshot, max_cook_memory_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_revisioned_command_is_visible_to_probe() {
        let initial = GraphSnapshot::demo(1, Vec3::ZERO);
        let service = RootService::new(initial.clone(), 1 << 20);
        let revision = service.replace_scene(
            initial.revision,
            &GraphSnapshot::demo(100, Vec3::checked(1., 2., 3.).unwrap_or(Vec3::ZERO)).to_ofxs(),
        );
        assert!(matches!(revision, Ok(RevisionId(2))));
        assert!(matches!(
            service.probe(0, 1),
            Ok(GeoProbe {
                revision: RevisionId(2),
                point_count: 100,
                ..
            })
        ));
    }
    #[test]
    fn save_and_load_preserve_probe_hash() {
        let service = RootService::new(
            GraphSnapshot::demo(3, Vec3::checked(1., 2., 3.).unwrap_or(Vec3::ZERO)),
            1 << 20,
        );
        let path = std::env::temp_dir().join(format!("pcc-mcp-{}-scene.ofxs", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(service.save_new(&path).is_ok());
        let loaded = RootService::load(&path, 1 << 20);
        let original = service.probe(0, 1);
        assert!(
            matches!((loaded,original),(Ok(other),Ok(probe)) if matches!(other.probe(0,1),Ok(reloaded) if reloaded.hash==probe.hash))
        );
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn structured_tool_errors_describe_conflict_and_invalid_denominator() {
        let initial = GraphSnapshot::demo(1, Vec3::ZERO);
        let service = RootService::new(initial.clone(), 1 << 20);
        let conflict = service.replace_scene(RevisionId(99), &initial.to_ofxs());
        assert!(matches!(
            conflict
                .as_ref()
                .map_err(|e| e.to_tool_error(service.max_cook_memory_bytes())),
            Err(ToolErrorPayload {
                code: ToolErrorCode::RevisionConflict,
                expected_revision: Some(RevisionId(99)),
                actual_revision: Some(RevisionId(1)),
                ..
            })
        ));
        let invalid_denom = service.probe(0, 0);
        assert!(matches!(
            invalid_denom
                .as_ref()
                .map_err(|e| e.to_tool_error(service.max_cook_memory_bytes())),
            Err(ToolErrorPayload {
                code: ToolErrorCode::InvalidFrameDenominator,
                ..
            })
        ));
    }
    #[test]
    fn cook_jobs_have_immutable_ids_progress_and_strict_inherited_limits() {
        let initial = GraphSnapshot::demo(16, Vec3::checked(1., 0., 0.).unwrap_or(Vec3::ZERO));
        let service = RootService::with_limits(initial, 1024, 32);

        // Requesting higher limits than policy clamps to service policy (1024 bytes, 32 elements).
        let first = service.start_cook_job(0, 24, Some(1 << 20), Some(10_000));
        assert!(matches!(
            first,
            Ok(CookJobSnapshot {
                job_id: JobId(1),
                revision: RevisionId(1),
                state: CookJobState::Completed,
                completed_steps: 2,
                total_steps: 2,
                effective_budget: CookJobBudget {
                    max_memory_bytes: 1024,
                    max_elements: 32,
                },
                stale: false,
                cache_published: true,
                probe: Some(GeoProbe {
                    point_count: 16,
                    ..
                }),
                error: None,
                ..
            })
        ));

        // Requesting a tighter element budget (8 < 16 points) fails with ElementLimitExceeded and does not cache.
        let second = service.start_cook_job(1, 24, None, Some(8));
        assert!(matches!(
            second,
            Ok(CookJobSnapshot {
                job_id: JobId(2),
                state: CookJobState::Failed,
                effective_budget: CookJobBudget {
                    max_memory_bytes: 1024,
                    max_elements: 8,
                },
                cache_published: false,
                probe: None,
                error: Some(ToolErrorPayload {
                    code: ToolErrorCode::ElementLimitExceeded,
                    requested_elements: Some(16),
                    max_elements: Some(8),
                    ..
                }),
                ..
            })
        ));
    }
    #[test]
    fn cooperative_cancellation_and_stale_job_protection() {
        let initial = GraphSnapshot::demo(8, Vec3::ZERO);
        let service = RootService::new(initial.clone(), 1 << 20);

        // Enqueue a job on revision 1, then cancel it before running.
        let queued = service.enqueue_cook_job(0, 24, None, None);
        assert!(matches!(
            queued,
            Ok(CookJobSnapshot {
                job_id: JobId(1),
                state: CookJobState::Queued,
                completed_steps: 0,
                ..
            })
        ));
        let cancel_res = service.cancel_cook_job(JobId(1));
        assert_eq!(
            cancel_res.ok(),
            Some(CancelJobResponse {
                job_id: JobId(1),
                cancelled: true,
                state: CookJobState::Cancelled,
            })
        );
        let after_run = service.run_cook_job(JobId(1));
        assert!(matches!(
            after_run,
            Ok(CookJobSnapshot {
                job_id: JobId(1),
                state: CookJobState::Cancelled,
                cache_published: false,
                probe: None,
                error: Some(ToolErrorPayload {
                    code: ToolErrorCode::CookCancelled,
                    ..
                }),
                ..
            })
        ));

        // Enqueue a second job on revision 1, then publish revision 2 before the job runs.
        // Replacing the scene must mark the r1 job stale and cancel it so it cannot overwrite newer state/cache.
        let stale_job = service.enqueue_cook_job(2, 24, None, None);
        assert!(matches!(
            stale_job,
            Ok(CookJobSnapshot {
                job_id: JobId(2),
                revision: RevisionId(1),
                state: CookJobState::Queued,
                ..
            })
        ));
        let next_scene = GraphSnapshot::demo(32, Vec3::ZERO);
        assert!(matches!(
            service.replace_scene(RevisionId(1), &next_scene.to_ofxs()),
            Ok(RevisionId(2))
        ));

        let stale_status = service.run_cook_job(JobId(2));
        assert!(matches!(
            stale_status,
            Ok(CookJobSnapshot {
                job_id: JobId(2),
                revision: RevisionId(1),
                state: CookJobState::Cancelled,
                stale: true,
                cache_published: false,
                ..
            })
        ));

        // Also verify that if a job started on revision 1 finishes after revision 2 is published
        // (without being pre-cancelled), its stale completion still computes its r1 probe but
        // never overwrites the shared cache for revision 2.
        let r2_snapshot = service.store.snapshot();
        assert!(r2_snapshot.is_ok());
        let late_r1_record = Arc::new(JobRecord {
            job_id: JobId(99),
            snapshot: Arc::new(initial),
            frame_numerator: 5,
            frame_denominator: 24,
            effective_budget: service.default_budget(),
            cancelled: Arc::new(AtomicBool::new(false)),
            completed_steps: AtomicU32::new(0),
            total_steps: 2,
            outcome: Mutex::new(JobOutcome {
                state: CookJobState::Queued,
                stale: false,
                cache_published: false,
                probe: None,
                error: None,
            }),
        });
        if let Ok(mut jobs) = service.jobs.lock() {
            jobs.insert(JobId(99), late_r1_record);
        }
        let late_r1_done = service.run_cook_job(JobId(99));
        assert!(matches!(
            late_r1_done,
            Ok(CookJobSnapshot {
                job_id: JobId(99),
                revision: RevisionId(1),
                state: CookJobState::Completed,
                stale: true,
                cache_published: false,
                probe: Some(GeoProbe {
                    revision: RevisionId(1),
                    point_count: 8,
                    ..
                }),
                ..
            })
        ));
    }

    #[test]
    fn transactional_sandbox_and_node_probe_lifecycle() {
        use pcc_graph::Node;
        let initial = GraphSnapshot::demo(4, Vec3::ZERO);
        let service = RootService::new(initial, 1 << 20);

        // 1. Begin a sandbox transaction on revision 1
        let tx = service.tx_begin();
        assert_eq!(
            tx.ok(),
            Some(TxSessionResponse {
                tx_id: TransactionId(1),
                base_revision: RevisionId(1),
                node_count: 2,
            })
        );

        // 2. Stage edits in the sandbox (replace with CreateGrid(10) -> Scatter(20))
        let staged = service.tx_stage_edits(
            TransactionId(1),
            &[
                GraphEdit::RemoveNode { id: NodeId(2) },
                GraphEdit::RemoveNode { id: NodeId(1) },
                GraphEdit::AddNode {
                    node: Node::new("grid", NodeId(10))
                        .with_u32("rows", 3)
                        .with_u32("cols", 3)
                        .with_f32("size_x", 4.0)
                        .with_f32("size_z", 4.0),
                },
                GraphEdit::AddNode {
                    node: Node::new("scatter", NodeId(20))
                        .with_input(NodeId(10))
                        .with_u32("count", 25)
                        .with_u64("seed", 7),
                },
            ],
        );
        assert_eq!(
            staged.ok(),
            Some(TxSessionResponse {
                tx_id: TransactionId(1),
                base_revision: RevisionId(1),
                node_count: 2,
            })
        );

        // 3. Probe the sandbox: sees 25 scattered points, while live scene still has 4 points at revision 1!
        let sandbox_probe = service.tx_probe(TransactionId(1), 0, 24);
        assert!(matches!(
            sandbox_probe,
            Ok(GeoProbe {
                revision: RevisionId(1),
                point_count: 25,
                ..
            })
        ));
        let live_probe = service.probe(0, 24);
        assert!(matches!(
            live_probe,
            Ok(GeoProbe {
                revision: RevisionId(1),
                point_count: 4,
                ..
            })
        ));

        // 4. Commit the sandbox: advances live scene to revision 2
        assert!(matches!(
            service.tx_commit(TransactionId(1)),
            Ok(RevisionId(2))
        ));

        // 5. Probe intermediate node 10 (`CreateGrid`, 3x3 = 9 points) and terminal node 20 (`Scatter`, 25 points)
        assert!(matches!(
            service.probe_node(NodeId(10), 0, 24),
            Ok(GeoProbe {
                revision: RevisionId(2),
                point_count: 9,
                ..
            })
        ));
        assert!(matches!(
            service.probe_node(NodeId(20), 0, 24),
            Ok(GeoProbe {
                revision: RevisionId(2),
                point_count: 25,
                ..
            })
        ));

        // 6. Test rollback on a second sandbox transaction
        let tx2 = service.tx_begin();
        assert!(tx2.is_ok());
        assert_eq!(
            service.tx_rollback(TransactionId(2)).ok(),
            Some(TxRollbackResponse {
                tx_id: TransactionId(2),
                rolled_back: true,
            })
        );
        assert!(matches!(
            service
                .tx_commit(TransactionId(2))
                .map_err(|e| e.to_tool_error(service.max_cook_memory_bytes())),
            Err(ToolErrorPayload {
                code: ToolErrorCode::TransactionNotFound,
                ..
            })
        ));
    }
}
