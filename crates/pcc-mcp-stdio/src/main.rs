//! Stdio transport entrypoint; all engine state remains in `pcc_mcp::RootService`.
use pcc_graph::GraphSnapshot;
use pcc_math::Vec3;
use pcc_mcp::RootService;
use pcc_mcp_stdio::PccMcp;
use rmcp::{ServiceExt, transport::stdio};
use std::{path::Path, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let initial = GraphSnapshot::demo(0, Vec3::ZERO);
    let mut engine = RootService::new(initial, 1 << 30);
    // File tools stay disabled unless the host explicitly names the one directory they may use.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => {}
        ["--workspace", directory] => {
            engine = engine
                .with_workspace(Path::new(directory))
                .map_err(|error| error.to_tool_error(0).message)?;
        }
        _ => return Err("usage: pcc-mcp-stdio [--workspace DIRECTORY]".into()),
    }
    let server = PccMcp::new(Arc::new(engine)).serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}
