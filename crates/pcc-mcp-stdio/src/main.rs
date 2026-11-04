//! Stdio transport entrypoint; all engine state remains in `pcc_mcp::RootService`.
use pcc_graph::GraphSnapshot;
use pcc_math::Vec3;
use pcc_mcp::RootService;
use pcc_mcp_stdio::PccMcp;
use rmcp::{ServiceExt, transport::stdio};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let initial = GraphSnapshot::demo(0, Vec3::ZERO);
    let server = PccMcp::new(Arc::new(RootService::new(initial, 1 << 30)))
        .serve(stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}
