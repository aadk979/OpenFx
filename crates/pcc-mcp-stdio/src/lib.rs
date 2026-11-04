//! Stdio MCP server adapter; all engine state remains in `pcc_mcp::RootService`.
use pcc_graph::{GraphEdit, NodeTypeSpec};
use pcc_id::{JobId, NodeId, RevisionId, TransactionId};
use pcc_mcp::{
    CancelJobResponse, CookJobSnapshot, GeoExportResponse, GeoImportResponse, GeoInvariantReport,
    GeoInvariantSpec, GeoProbe, NodeCatalogSearchResponse, ReplaceSceneResponse, RootService,
    ToolErrorPayload, TxRollbackResponse, TxSessionResponse, VexCheckReport,
};
use rmcp::{Json, handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VexCheckParams {
    pub snippet: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProbeParams {
    pub frame_numerator: i64,
    pub frame_denominator: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NodeCookParams {
    pub node_id: u64,
    pub frame_numerator: i64,
    pub frame_denominator: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TxStageEditsParams {
    pub tx_id: u64,
    pub edits: Vec<GraphEdit>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TxProbeParams {
    pub tx_id: u64,
    pub frame_numerator: i64,
    pub frame_denominator: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TxIdParams {
    pub tx_id: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplaceSceneParams {
    pub base_revision: u64,
    pub ofxs: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BatchEditParams {
    pub base_revision: u64,
    pub edits: Vec<GraphEdit>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RevisionParams {
    pub expected_revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CatalogSearchParams {
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NodeTypeInspectParams {
    pub type_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AssertInvariantsParams {
    pub frame_numerator: i64,
    pub frame_denominator: u32,
    pub spec: GeoInvariantSpec,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StartCookJobParams {
    pub frame_numerator: i64,
    pub frame_denominator: u32,
    pub max_memory_bytes: Option<usize>,
    pub max_elements: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct JobIdParams {
    pub job_id: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GeoImportParams {
    /// Path relative to the server's workspace, ending in `.stl`, `.obj`, or `.ofxg`.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GeoExportParams {
    /// Path relative to the server's workspace, ending in `.stl`, `.obj`, or `.ofxg`. Its parent
    /// directory must already exist.
    pub path: String,
    /// The node to cook. Defaults to the scene's final node.
    pub node_id: Option<u64>,
    /// Replace an existing file instead of failing.
    #[serde(default)]
    pub overwrite: bool,
    pub frame_numerator: i64,
    pub frame_denominator: u32,
}

#[derive(Clone)]
pub struct PccMcp {
    engine: Arc<RootService>,
}

impl PccMcp {
    #[must_use]
    pub fn new(engine: Arc<RootService>) -> Self {
        Self { engine }
    }
}

#[tool_router(server_handler)]
impl PccMcp {
    #[tool(
        description = "Cook the current immutable scene snapshot and return point count, bounds, hash, and revision."
    )]
    pub fn geo_probe(
        &self,
        Parameters(params): Parameters<ProbeParams>,
    ) -> Result<Json<GeoProbe>, Json<ToolErrorPayload>> {
        self.engine
            .probe(params.frame_numerator, params.frame_denominator)
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Cook the current scene snapshot and evaluate declarative geometry invariants (point/primitive count ranges and bounding box tolerances)."
    )]
    pub fn geo_assert_invariants(
        &self,
        Parameters(params): Parameters<AssertInvariantsParams>,
    ) -> Result<Json<GeoInvariantReport>, Json<ToolErrorPayload>> {
        self.engine
            .assert_invariants(
                params.frame_numerator,
                params.frame_denominator,
                &params.spec,
            )
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Replace the .ofxs scene only when base_revision matches the current revision."
    )]
    pub fn replace_scene(
        &self,
        Parameters(params): Parameters<ReplaceSceneParams>,
    ) -> Result<Json<ReplaceSceneResponse>, Json<ToolErrorPayload>> {
        self.engine
            .replace_scene(RevisionId(params.base_revision), &params.ofxs)
            .map(|committed_revision| Json(ReplaceSceneResponse { committed_revision }))
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Apply an atomic, revision-checked batch of graph edits (add/remove nodes, set parameters, connect inputs) with automatic topological sorting and cycle rejection."
    )]
    pub fn graph_batch_edit(
        &self,
        Parameters(params): Parameters<BatchEditParams>,
    ) -> Result<Json<ReplaceSceneResponse>, Json<ToolErrorPayload>> {
        self.engine
            .batch_edit(RevisionId(params.base_revision), &params.edits)
            .map(|committed_revision| Json(ReplaceSceneResponse { committed_revision }))
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Undo the most recent committed scene mutation when expected_revision matches the current revision."
    )]
    pub fn scene_undo(
        &self,
        Parameters(params): Parameters<RevisionParams>,
    ) -> Result<Json<ReplaceSceneResponse>, Json<ToolErrorPayload>> {
        self.engine
            .undo(RevisionId(params.expected_revision))
            .map(|committed_revision| Json(ReplaceSceneResponse { committed_revision }))
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Redo the most recently undone scene mutation when expected_revision matches the current revision."
    )]
    pub fn scene_redo(
        &self,
        Parameters(params): Parameters<RevisionParams>,
    ) -> Result<Json<ReplaceSceneResponse>, Json<ToolErrorPayload>> {
        self.engine
            .redo(RevisionId(params.expected_revision))
            .map(|committed_revision| Json(ReplaceSceneResponse { committed_revision }))
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Search the operator catalog by keyword across node type names, categories, and summaries."
    )]
    pub fn node_catalog_search(
        &self,
        Parameters(params): Parameters<CatalogSearchParams>,
    ) -> Result<Json<NodeCatalogSearchResponse>, Json<ToolErrorPayload>> {
        Ok(Json(NodeCatalogSearchResponse {
            node_types: self.engine.search_node_catalog(&params.query),
        }))
    }

    #[tool(
        description = "Inspect input/output ports and parameter schema for a specific node type in the operator catalog."
    )]
    pub fn node_type_inspect(
        &self,
        Parameters(params): Parameters<NodeTypeInspectParams>,
    ) -> Result<Json<NodeTypeSpec>, Json<ToolErrorPayload>> {
        self.engine
            .inspect_node_type(&params.type_name)
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Start a cook job on the current immutable scene snapshot with strict inherited resource limits."
    )]
    pub fn cook_job_start(
        &self,
        Parameters(params): Parameters<StartCookJobParams>,
    ) -> Result<Json<CookJobSnapshot>, Json<ToolErrorPayload>> {
        self.engine
            .start_cook_job(
                params.frame_numerator,
                params.frame_denominator,
                params.max_memory_bytes,
                params.max_elements,
            )
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Retrieve status, step progress, and result or error for a cook job by immutable job_id."
    )]
    pub fn cook_job_status(
        &self,
        Parameters(params): Parameters<JobIdParams>,
    ) -> Result<Json<CookJobSnapshot>, Json<ToolErrorPayload>> {
        self.engine
            .cook_job_status(JobId(params.job_id))
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(description = "Cooperatively cancel a cook job by immutable job_id.")]
    pub fn cook_job_cancel(
        &self,
        Parameters(params): Parameters<JobIdParams>,
    ) -> Result<Json<CancelJobResponse>, Json<ToolErrorPayload>> {
        self.engine
            .cancel_cook_job(JobId(params.job_id))
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Read a mesh file from the server's workspace into the content-addressed asset store and return its content hash. This does not change the scene: add an `asset` node with that hash using `graph_batch_edit` or `replace_scene`. The scene then no longer depends on the file. Requires the server to be started with --workspace."
    )]
    pub fn geo_import(
        &self,
        Parameters(params): Parameters<GeoImportParams>,
    ) -> Result<Json<GeoImportResponse>, Json<ToolErrorPayload>> {
        self.engine
            .geo_import(&params.path)
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Cook a node, or the scene's final node, and write it to a file in the server's workspace. The extension picks the format: `.obj` (keeps vertex `uv`), `.stl` (triangulated), or `.ofxg` (lossless, all attributes). Fails rather than replacing a file unless `overwrite` is set. Requires the server to be started with --workspace."
    )]
    pub fn geo_export(
        &self,
        Parameters(params): Parameters<GeoExportParams>,
    ) -> Result<Json<GeoExportResponse>, Json<ToolErrorPayload>> {
        self.engine
            .geo_export(
                &params.path,
                params.node_id.map(NodeId),
                params.overwrite,
                params.frame_numerator,
                params.frame_denominator,
            )
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Cook a specific intermediate or terminal node in the current immutable scene snapshot and return its geometry probe."
    )]
    pub fn node_cook(
        &self,
        Parameters(params): Parameters<NodeCookParams>,
    ) -> Result<Json<GeoProbe>, Json<ToolErrorPayload>> {
        self.engine
            .probe_node(
                NodeId(params.node_id),
                params.frame_numerator,
                params.frame_denominator,
            )
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Begin an isolated Copy-on-Write sandbox transaction forked from the current immutable scene revision."
    )]
    pub fn tx_begin(&self) -> Result<Json<TxSessionResponse>, Json<ToolErrorPayload>> {
        self.engine
            .tx_begin()
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Stage a batch of graph edits inside an isolated sandbox transaction without mutating the live scene."
    )]
    pub fn tx_stage_edits(
        &self,
        Parameters(params): Parameters<TxStageEditsParams>,
    ) -> Result<Json<TxSessionResponse>, Json<ToolErrorPayload>> {
        self.engine
            .tx_stage_edits(TransactionId(params.tx_id), &params.edits)
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Cook and probe the staged graph inside an isolated sandbox transaction without mutating the live scene."
    )]
    pub fn tx_probe(
        &self,
        Parameters(params): Parameters<TxProbeParams>,
    ) -> Result<Json<GeoProbe>, Json<ToolErrorPayload>> {
        self.engine
            .tx_probe(
                TransactionId(params.tx_id),
                params.frame_numerator,
                params.frame_denominator,
            )
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Atomically commit a sandbox transaction to the live scene store if its base_revision is still current."
    )]
    pub fn tx_commit(
        &self,
        Parameters(params): Parameters<TxIdParams>,
    ) -> Result<Json<ReplaceSceneResponse>, Json<ToolErrorPayload>> {
        self.engine
            .tx_commit(TransactionId(params.tx_id))
            .map(|committed_revision| Json(ReplaceSceneResponse { committed_revision }))
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Discard an uncommitted sandbox transaction without touching the live scene store."
    )]
    pub fn tx_rollback(
        &self,
        Parameters(params): Parameters<TxIdParams>,
    ) -> Result<Json<TxRollbackResponse>, Json<ToolErrorPayload>> {
        self.engine
            .tx_rollback(TransactionId(params.tx_id))
            .map(Json)
            .map_err(|error| Json(error.to_tool_error(self.engine.max_cook_memory_bytes())))
    }

    #[tool(
        description = "Compile and type-check a VEX-lite point wrangle snippet, returning read/written attributes and structured diagnostics."
    )]
    pub fn vex_check(
        &self,
        Parameters(params): Parameters<VexCheckParams>,
    ) -> Result<Json<VexCheckReport>, Json<ToolErrorPayload>> {
        Ok(Json(self.engine.vex_check(&params.snippet)))
    }
}
