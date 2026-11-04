//! Integration tests launching `pcc-mcp-stdio` over actual child-process stdio pipes.
use pcc_graph::GraphSnapshot;
use pcc_id::{JobId, RevisionId};
use pcc_math::Vec3;
use pcc_mcp::{
    CancelJobResponse, CookJobSnapshot, CookJobState, GeoInvariantReport, GeoProbe,
    NodeCatalogSearchResponse, ReplaceSceneResponse, ToolErrorCode, ToolErrorPayload,
};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[derive(Debug)]
struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioClient {
    fn spawn() -> Result<Self, String> {
        let bin = env!("CARGO_BIN_EXE_pcc-mcp-stdio");
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn server: {e}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "missing child stdin".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "missing child stdout".to_owned())?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    async fn send(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        self.stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn recv(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("unexpected EOF from server stdout".to_owned());
        }
        serde_json::from_str(&line).map_err(|e| format!("invalid JSON `{line}`: {e}"))
    }

    async fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

#[tokio::test]
async fn stdio_transport_initialize_discover_probe_replace_and_conflict() {
    let client_res = StdioClient::spawn();
    assert!(client_res.is_ok(), "{client_res:?}");
    let Ok(mut client) = client_res else {
        return;
    };

    // 1. Initialize handshake
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "integration-test", "version": "0.1.0" }
                }
            }))
            .await
            .is_ok()
    );
    let init_resp = client.recv().await;
    assert!(init_resp.is_ok(), "{init_resp:?}");
    if let Ok(resp) = init_resp {
        assert_eq!(resp["id"], 1);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .await
            .is_ok()
    );

    // 2. Tool discovery (`tools/list`)
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }))
            .await
            .is_ok()
    );
    let list_resp = client.recv().await;
    assert!(list_resp.is_ok(), "{list_resp:?}");
    if let Ok(resp) = list_resp {
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array();
        assert!(tools.is_some());
        if let Some(tools) = tools {
            let names = tools
                .iter()
                .filter_map(|t| t["name"].as_str())
                .collect::<Vec<_>>();
            assert!(names.contains(&"geo_probe"));
            assert!(names.contains(&"geo_assert_invariants"));
            assert!(names.contains(&"replace_scene"));
            assert!(names.contains(&"graph_batch_edit"));
            assert!(names.contains(&"scene_undo"));
            assert!(names.contains(&"scene_redo"));
            assert!(names.contains(&"node_catalog_search"));
            assert!(names.contains(&"node_type_inspect"));
            assert!(names.contains(&"cook_job_start"));
            assert!(names.contains(&"cook_job_status"));
            assert!(names.contains(&"cook_job_cancel"));
            assert!(names.contains(&"node_cook"));
            assert!(names.contains(&"tx_begin"));
            assert!(names.contains(&"tx_stage_edits"));
            assert!(names.contains(&"tx_probe"));
            assert!(names.contains(&"tx_commit"));
            assert!(names.contains(&"tx_rollback"));
            assert!(names.contains(&"vex_check"));
            for tool in tools {
                assert!(tool["inputSchema"].is_object());
                assert!(tool["outputSchema"].is_object());
            }
        }
    }

    // 3. Initial `geo_probe` on default empty scene (revision 1, 0 points)
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "geo_probe",
                    "arguments": {
                        "frame_numerator": 0,
                        "frame_denominator": 24
                    }
                }
            }))
            .await
            .is_ok()
    );
    let probe1_resp = client.recv().await;
    assert!(probe1_resp.is_ok(), "{probe1_resp:?}");
    if let Ok(resp) = probe1_resp {
        assert_eq!(resp["id"], 3);
        assert_eq!(resp["result"]["isError"], false);
        let structured =
            serde_json::from_value::<GeoProbe>(resp["result"]["structuredContent"].clone());
        assert_eq!(
            structured.ok(),
            Some(GeoProbe {
                revision: RevisionId(1),
                point_count: 0,
                prim_count: 0,
                vertex_count: 0,
                surface_area: 0.0,
                enclosed_volume: 0.0,
                is_manifold: true,
                is_closed_manifold: false,
                point_attributes: Vec::new(),
                hash: pcc_geo::GeoDetail::points(Vec::new())
                    .hash()
                    .to_hex()
                    .to_string(),
                bounds: None,
            })
        );
    }

    // 4. Successful `replace_scene` from base_revision 1 -> revision 2
    let next_scene = GraphSnapshot::demo(4, Vec3::checked(1.0, 2.0, -3.0).unwrap_or(Vec3::ZERO));
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "replace_scene",
                    "arguments": {
                        "base_revision": 1,
                        "ofxs": next_scene.to_ofxs()
                    }
                }
            }))
            .await
            .is_ok()
    );
    let replace_resp = client.recv().await;
    assert!(replace_resp.is_ok(), "{replace_resp:?}");
    if let Ok(resp) = replace_resp {
        assert_eq!(resp["id"], 4);
        assert_eq!(resp["result"]["isError"], false);
        let structured = serde_json::from_value::<ReplaceSceneResponse>(
            resp["result"]["structuredContent"].clone(),
        );
        assert_eq!(
            structured.ok(),
            Some(ReplaceSceneResponse {
                committed_revision: RevisionId(2)
            })
        );
    }

    // 5. Verify `geo_probe` now observes revision 2 with 4 translated points
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "geo_probe",
                    "arguments": {
                        "frame_numerator": 0,
                        "frame_denominator": 24
                    }
                }
            }))
            .await
            .is_ok()
    );
    let probe2_resp = client.recv().await;
    assert!(probe2_resp.is_ok(), "{probe2_resp:?}");
    if let Ok(resp) = probe2_resp {
        assert_eq!(resp["id"], 5);
        assert_eq!(resp["result"]["isError"], false);
        let structured =
            serde_json::from_value::<GeoProbe>(resp["result"]["structuredContent"].clone());
        assert!(matches!(
            structured.ok(),
            Some(GeoProbe {
                revision: RevisionId(2),
                point_count: 4,
                bounds: Some((min, max)),
                ..
            }) if min == Vec3::checked(1.0, 2.0, -3.0).unwrap_or(Vec3::ZERO)
               && max == Vec3::checked(4.0, 2.0, -3.0).unwrap_or(Vec3::ZERO)
        ));
    }

    // 6. Stale-revision conflict on `replace_scene` with base_revision = 1 (actual is 2)
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "replace_scene",
                    "arguments": {
                        "base_revision": 1,
                        "ofxs": next_scene.to_ofxs()
                    }
                }
            }))
            .await
            .is_ok()
    );
    let conflict_resp = client.recv().await;
    assert!(conflict_resp.is_ok(), "{conflict_resp:?}");
    if let Ok(resp) = conflict_resp {
        assert_eq!(resp["id"], 6);
        assert_eq!(resp["result"]["isError"], true);
        let err_payload =
            serde_json::from_value::<ToolErrorPayload>(resp["result"]["structuredContent"].clone());
        assert!(matches!(
            err_payload.ok(),
            Some(ToolErrorPayload {
                code: ToolErrorCode::RevisionConflict,
                expected_revision: Some(RevisionId(1)),
                actual_revision: Some(RevisionId(2)),
                ..
            })
        ));
    }

    // 7. Start a cook job via `cook_job_start` and query `cook_job_status`
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "cook_job_start",
                    "arguments": {
                        "frame_numerator": 10,
                        "frame_denominator": 24,
                        "max_elements": 64
                    }
                }
            }))
            .await
            .is_ok()
    );
    let job_start_resp = client.recv().await;
    assert!(job_start_resp.is_ok(), "{job_start_resp:?}");
    if let Ok(resp) = job_start_resp {
        assert_eq!(resp["id"], 7);
        assert_eq!(resp["result"]["isError"], false);
        let job_snap =
            serde_json::from_value::<CookJobSnapshot>(resp["result"]["structuredContent"].clone());
        assert!(matches!(
            job_snap.ok(),
            Some(CookJobSnapshot {
                job_id: JobId(1),
                revision: RevisionId(2),
                state: CookJobState::Completed,
                completed_steps: 2,
                total_steps: 2,
                probe: Some(GeoProbe { point_count: 4, .. }),
                ..
            })
        ));
    }

    // 8. Query `cook_job_status` for job 1 and verify `cook_job_cancel` on completed job
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "cook_job_status",
                    "arguments": {
                        "job_id": 1
                    }
                }
            }))
            .await
            .is_ok()
    );
    let job_status_resp = client.recv().await;
    assert!(job_status_resp.is_ok(), "{job_status_resp:?}");
    if let Ok(resp) = job_status_resp {
        assert_eq!(resp["id"], 8);
        assert_eq!(resp["result"]["isError"], false);
        let job_snap =
            serde_json::from_value::<CookJobSnapshot>(resp["result"]["structuredContent"].clone());
        assert!(matches!(
            job_snap.ok(),
            Some(CookJobSnapshot {
                job_id: JobId(1),
                state: CookJobState::Completed,
                ..
            })
        ));
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {
                    "name": "cook_job_cancel",
                    "arguments": {
                        "job_id": 1
                    }
                }
            }))
            .await
            .is_ok()
    );
    let job_cancel_resp = client.recv().await;
    assert!(job_cancel_resp.is_ok(), "{job_cancel_resp:?}");
    if let Ok(resp) = job_cancel_resp {
        assert_eq!(resp["id"], 9);
        assert_eq!(resp["result"]["isError"], false);
        let cancel_out = serde_json::from_value::<CancelJobResponse>(
            resp["result"]["structuredContent"].clone(),
        );
        assert_eq!(
            cancel_out.ok(),
            Some(CancelJobResponse {
                job_id: JobId(1),
                cancelled: false,
                state: CookJobState::Completed,
            })
        );
    }

    // 9. `node_catalog_search` and `node_type_inspect`
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": {
                    "name": "node_catalog_search",
                    "arguments": {
                        "query": "merge"
                    }
                }
            }))
            .await
            .is_ok()
    );
    let cat_resp = client.recv().await;
    assert!(cat_resp.is_ok(), "{cat_resp:?}");
    if let Ok(resp) = cat_resp {
        assert_eq!(resp["id"], 10);
        assert_eq!(resp["result"]["isError"], false);
        let search_out = serde_json::from_value::<NodeCatalogSearchResponse>(
            resp["result"]["structuredContent"].clone(),
        );
        assert_eq!(search_out.map(|s| s.node_types.len()).ok(), Some(1));
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": "node_type_inspect",
                    "arguments": {
                        "type_name": "transform"
                    }
                }
            }))
            .await
            .is_ok()
    );
    let inspect_resp = client.recv().await;
    assert!(inspect_resp.is_ok(), "{inspect_resp:?}");
    if let Ok(resp) = inspect_resp {
        assert_eq!(resp["id"], 11);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(
            resp["result"]["structuredContent"]["type_name"],
            "transform"
        );
    }

    // 10. `graph_batch_edit` from base_revision 2 -> revision 3 (set count to 8)
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "tools/call",
                "params": {
                    "name": "graph_batch_edit",
                    "arguments": {
                        "base_revision": 2,
                        "edits": [
                            { "edit": "set_create_count", "id": 1, "count": 8 }
                        ]
                    }
                }
            }))
            .await
            .is_ok()
    );
    let batch_resp = client.recv().await;
    assert!(batch_resp.is_ok(), "{batch_resp:?}");
    if let Ok(resp) = batch_resp {
        assert_eq!(resp["id"], 12);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 3);
    }

    // 11. `geo_assert_invariants` verifying 8 points at revision 3
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 13,
                "method": "tools/call",
                "params": {
                    "name": "geo_assert_invariants",
                    "arguments": {
                        "frame_numerator": 0,
                        "frame_denominator": 24,
                        "spec": {
                            "min_points": 8,
                            "max_points": 8
                        }
                    }
                }
            }))
            .await
            .is_ok()
    );
    let inv_resp = client.recv().await;
    assert!(inv_resp.is_ok(), "{inv_resp:?}");
    if let Ok(resp) = inv_resp {
        assert_eq!(resp["id"], 13);
        assert_eq!(resp["result"]["isError"], false);
        let report = serde_json::from_value::<GeoInvariantReport>(
            resp["result"]["structuredContent"].clone(),
        );
        assert!(matches!(
            report.ok(),
            Some(GeoInvariantReport {
                passed: true,
                probe: GeoProbe {
                    revision: RevisionId(3),
                    point_count: 8,
                    ..
                },
                ..
            })
        ));
    }

    // 12. `scene_undo` (revision 3 -> 4, reverting to 4 points) and `scene_redo` (revision 4 -> 5, 8 points)
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 14,
                "method": "tools/call",
                "params": {
                    "name": "scene_undo",
                    "arguments": {
                        "expected_revision": 3
                    }
                }
            }))
            .await
            .is_ok()
    );
    let undo_resp = client.recv().await;
    assert!(undo_resp.is_ok(), "{undo_resp:?}");
    if let Ok(resp) = undo_resp {
        assert_eq!(resp["id"], 14);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 4);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 15,
                "method": "tools/call",
                "params": {
                    "name": "scene_redo",
                    "arguments": {
                        "expected_revision": 4
                    }
                }
            }))
            .await
            .is_ok()
    );
    let redo_resp = client.recv().await;
    assert!(redo_resp.is_ok(), "{redo_resp:?}");
    if let Ok(resp) = redo_resp {
        assert_eq!(resp["id"], 15);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 5);
    }

    // 13. `tx_begin`, `tx_stage_edits`, `tx_probe`, and `tx_commit`
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 16,
                "method": "tools/call",
                "params": {
                    "name": "tx_begin",
                    "arguments": {}
                }
            }))
            .await
            .is_ok()
    );
    let tx_begin_resp = client.recv().await;
    assert!(tx_begin_resp.is_ok(), "{tx_begin_resp:?}");
    if let Ok(resp) = tx_begin_resp {
        assert_eq!(resp["id"], 16);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["tx_id"], 1);
        assert_eq!(resp["result"]["structuredContent"]["base_revision"], 5);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 17,
                "method": "tools/call",
                "params": {
                    "name": "tx_stage_edits",
                    "arguments": {
                        "tx_id": 1,
                        "edits": [
                            { "edit": "set_create_count", "id": 1, "count": 12 }
                        ]
                    }
                }
            }))
            .await
            .is_ok()
    );
    let tx_stage_resp = client.recv().await;
    assert!(tx_stage_resp.is_ok(), "{tx_stage_resp:?}");
    if let Ok(resp) = tx_stage_resp {
        assert_eq!(resp["id"], 17);
        assert_eq!(resp["result"]["isError"], false);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 18,
                "method": "tools/call",
                "params": {
                    "name": "tx_probe",
                    "arguments": {
                        "tx_id": 1,
                        "frame_numerator": 0,
                        "frame_denominator": 24
                    }
                }
            }))
            .await
            .is_ok()
    );
    let tx_probe_resp = client.recv().await;
    assert!(tx_probe_resp.is_ok(), "{tx_probe_resp:?}");
    if let Ok(resp) = tx_probe_resp {
        assert_eq!(resp["id"], 18);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["point_count"], 12);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 19,
                "method": "tools/call",
                "params": {
                    "name": "tx_commit",
                    "arguments": {
                        "tx_id": 1
                    }
                }
            }))
            .await
            .is_ok()
    );
    let tx_commit_resp = client.recv().await;
    assert!(tx_commit_resp.is_ok(), "{tx_commit_resp:?}");
    if let Ok(resp) = tx_commit_resp {
        assert_eq!(resp["id"], 19);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 6);
    }

    // 14. `node_cook` on intermediate node 1 (CreatePoints, 12 points)
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 20,
                "method": "tools/call",
                "params": {
                    "name": "node_cook",
                    "arguments": {
                        "node_id": 1,
                        "frame_numerator": 0,
                        "frame_denominator": 24
                    }
                }
            }))
            .await
            .is_ok()
    );
    let node_cook_resp = client.recv().await;
    assert!(node_cook_resp.is_ok(), "{node_cook_resp:?}");
    if let Ok(resp) = node_cook_resp {
        assert_eq!(resp["id"], 20);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["point_count"], 12);
        assert_eq!(resp["result"]["structuredContent"]["revision"], 6);
    }

    // 15. `vex_check` validating read/written attributes for a VEX-lite snippet
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 21,
                "method": "tools/call",
                "params": {
                    "name": "vex_check",
                    "arguments": {
                        "snippet": "f@pscale = 0.5; @P.y = sin(@P.x);"
                    }
                }
            }))
            .await
            .is_ok()
    );
    let vex_resp = client.recv().await;
    assert!(vex_resp.is_ok(), "{vex_resp:?}");
    if let Ok(resp) = vex_resp {
        assert_eq!(resp["id"], 21);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["valid"], true);
        assert_eq!(
            resp["result"]["structuredContent"]["read_attributes"],
            json!(["P"])
        );
        assert_eq!(
            resp["result"]["structuredContent"]["written_attributes"],
            json!(["P", "pscale"])
        );
    }

    // 16. Replace scene with topological modeling pipeline: CreateBox -> Subdivide(1) -> PolyExtrude(0.25, 0.05) -> Normal
    let modeling_scene = GraphSnapshot {
        revision: RevisionId(6),
        nodes: vec![
            pcc_graph::Node::CreateBox {
                id: pcc_id::NodeId(1),
                size: Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
            },
            pcc_graph::Node::Subdivide {
                id: pcc_id::NodeId(2),
                input: pcc_id::NodeId(1),
                iterations: 1,
            },
            pcc_graph::Node::PolyExtrude {
                id: pcc_id::NodeId(3),
                input: pcc_id::NodeId(2),
                distance: 0.25,
                inset: 0.05,
            },
            pcc_graph::Node::Normal {
                id: pcc_id::NodeId(4),
                input: pcc_id::NodeId(3),
            },
        ],
    };
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 22,
                "method": "tools/call",
                "params": {
                    "name": "replace_scene",
                    "arguments": {
                        "base_revision": 6,
                        "ofxs": modeling_scene.to_ofxs()
                    }
                }
            }))
            .await
            .is_ok()
    );
    let rep_mod_resp = client.recv().await;
    assert!(rep_mod_resp.is_ok(), "{rep_mod_resp:?}");
    if let Ok(resp) = rep_mod_resp {
        assert_eq!(resp["id"], 22);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 7);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 23,
                "method": "tools/call",
                "params": {
                    "name": "geo_probe",
                    "arguments": {
                        "frame_numerator": 0,
                        "frame_denominator": 24
                    }
                }
            }))
            .await
            .is_ok()
    );
    let mod_probe_resp = client.recv().await;
    assert!(mod_probe_resp.is_ok(), "{mod_probe_resp:?}");
    if let Ok(resp) = mod_probe_resp {
        assert_eq!(resp["id"], 23);
        assert_eq!(resp["result"]["isError"], false);
        let structured =
            serde_json::from_value::<GeoProbe>(resp["result"]["structuredContent"].clone());
        assert!(structured.is_ok(), "{structured:?}");
        if let Ok(probe) = structured {
            assert_eq!(probe.revision, RevisionId(7));
            // Subdivide(1) produces 24 quads; PolyExtrude extrudes all 24 quads -> 24 caps + 96 walls = 120 quads
            assert_eq!(probe.prim_count, 120);
            assert_eq!(probe.vertex_count, 480);
            assert!(probe.is_manifold);
            assert!(probe.is_closed_manifold);
            assert!(probe.surface_area > 10.0);
            assert!(probe.enclosed_volume > 2.0);
            assert_eq!(probe.point_attributes, vec!["N".to_owned()]);
        }
    }

    // 17. Replace scene with M1.4 HeightField terrain & UV pipeline:
    // HeightField(10x10) -> HeightFieldNoise -> HeightFieldErode -> UvProject(PlanarY) -> Normal
    let terrain_scene = GraphSnapshot {
        revision: RevisionId(7),
        nodes: vec![
            pcc_graph::Node::HeightField {
                id: pcc_id::NodeId(1),
                rows: 10,
                cols: 10,
                size_x: 20.0,
                size_z: 20.0,
            },
            pcc_graph::Node::HeightFieldNoise {
                id: pcc_id::NodeId(2),
                input: pcc_id::NodeId(1),
                amplitude: 5.0,
                frequency: 0.12,
                octaves: 3,
                seed: 42,
            },
            pcc_graph::Node::HeightFieldErode {
                id: pcc_id::NodeId(3),
                input: pcc_id::NodeId(2),
                iterations: 8,
                rain_rate: 0.05,
                talus_angle: 35.0,
                seed: 7,
            },
            pcc_graph::Node::UvProject {
                id: pcc_id::NodeId(4),
                input: pcc_id::NodeId(3),
                mode: pcc_graph::UvProjectMode::PlanarY,
            },
            pcc_graph::Node::Normal {
                id: pcc_id::NodeId(5),
                input: pcc_id::NodeId(4),
            },
        ],
    };
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 24,
                "method": "tools/call",
                "params": {
                    "name": "replace_scene",
                    "arguments": {
                        "base_revision": 7,
                        "ofxs": terrain_scene.to_ofxs()
                    }
                }
            }))
            .await
            .is_ok()
    );
    let rep_ter_resp = client.recv().await;
    assert!(rep_ter_resp.is_ok(), "{rep_ter_resp:?}");
    if let Ok(resp) = rep_ter_resp {
        assert_eq!(resp["id"], 24);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 8);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 25,
                "method": "tools/call",
                "params": {
                    "name": "geo_probe",
                    "arguments": {
                        "frame_numerator": 0,
                        "frame_denominator": 24
                    }
                }
            }))
            .await
            .is_ok()
    );
    let ter_probe_resp = client.recv().await;
    assert!(ter_probe_resp.is_ok(), "{ter_probe_resp:?}");
    if let Ok(resp) = ter_probe_resp {
        assert_eq!(resp["id"], 25);
        assert_eq!(resp["result"]["isError"], false);
        let structured =
            serde_json::from_value::<GeoProbe>(resp["result"]["structuredContent"].clone());
        assert!(structured.is_ok(), "{structured:?}");
        if let Ok(probe) = structured {
            assert_eq!(probe.revision, RevisionId(8));
            assert_eq!(probe.point_count, 100);
            assert_eq!(probe.prim_count, 81);
            assert_eq!(probe.vertex_count, 324);
            assert!(probe.is_manifold);
            assert!(!probe.is_closed_manifold);
            assert!(probe.surface_area > 400.0);
            assert_eq!(
                probe.point_attributes,
                vec![
                    "N".to_owned(),
                    "height".to_owned(),
                    "mask".to_owned(),
                    "sediment".to_owned(),
                    "uv".to_owned(),
                    "water".to_owned(),
                ]
            );
        }
    }

    // 18. Replace scene with M1.3 VDB & Volumetric Boolean pipeline:
    // CreateBox(1) + Transform(2) -> Boolean(Union, 3) -> VdbFromPolygons(4) -> VdbReshape(5) -> ConvertVdb(6) -> Normal(7)
    let vdb_boolean_scene = GraphSnapshot {
        revision: RevisionId(8),
        nodes: vec![
            pcc_graph::Node::CreateBox {
                id: pcc_id::NodeId(1),
                size: Vec3::checked(2.0, 2.0, 2.0).unwrap_or(Vec3::ZERO),
            },
            pcc_graph::Node::Transform {
                id: pcc_id::NodeId(2),
                input: pcc_id::NodeId(1),
                offset: Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO),
            },
            pcc_graph::Node::Boolean {
                id: pcc_id::NodeId(3),
                left: pcc_id::NodeId(1),
                right: pcc_id::NodeId(2),
                mode: pcc_graph::BooleanMode::Union,
                voxel_size: 0.5,
            },
            pcc_graph::Node::VdbFromPolygons {
                id: pcc_id::NodeId(4),
                input: pcc_id::NodeId(3),
                voxel_size: 0.5,
            },
            pcc_graph::Node::VdbReshape {
                id: pcc_id::NodeId(5),
                input: pcc_id::NodeId(4),
                offset: 0.1,
                smooth_iterations: 1,
            },
            pcc_graph::Node::ConvertVdb {
                id: pcc_id::NodeId(6),
                input: pcc_id::NodeId(5),
            },
            pcc_graph::Node::Normal {
                id: pcc_id::NodeId(7),
                input: pcc_id::NodeId(6),
            },
        ],
    };
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 26,
                "method": "tools/call",
                "params": {
                    "name": "replace_scene",
                    "arguments": {
                        "base_revision": 8,
                        "ofxs": vdb_boolean_scene.to_ofxs()
                    }
                }
            }))
            .await
            .is_ok()
    );
    let rep_vdb_resp = client.recv().await;
    assert!(rep_vdb_resp.is_ok(), "{rep_vdb_resp:?}");
    if let Ok(resp) = rep_vdb_resp {
        assert_eq!(resp["id"], 26);
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["committed_revision"], 9);
    }

    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 27,
                "method": "tools/call",
                "params": {
                    "name": "geo_assert_invariants",
                    "arguments": {
                        "frame_numerator": 0,
                        "frame_denominator": 24,
                        "spec": {
                            "min_points": 20,
                            "min_prims": 18,
                            "require_manifold": true,
                            "require_closed_manifold": true
                        }
                    }
                }
            }))
            .await
            .is_ok()
    );
    let vdb_inv_resp = client.recv().await;
    assert!(vdb_inv_resp.is_ok(), "{vdb_inv_resp:?}");
    if let Ok(resp) = vdb_inv_resp {
        assert_eq!(resp["id"], 27);
        assert_eq!(resp["result"]["isError"], false);
        let report = serde_json::from_value::<GeoInvariantReport>(
            resp["result"]["structuredContent"].clone(),
        );
        assert!(report.is_ok(), "{report:?}");
        if let Ok(report) = report {
            assert!(report.passed, "violations: {:?}", report.violations);
            assert_eq!(report.probe.revision, RevisionId(9));
            assert!(report.probe.is_closed_manifold);
            assert!(report.probe.enclosed_volume > 8.0);
        }
    }

    client.shutdown().await;
}

/// Sends one `tools/call` and returns its `result` object after checking the response id.
async fn call_tool(client: &mut StdioClient, id: u64, name: &str, arguments: Value) -> Value {
    let sent = client
        .send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }))
        .await;
    assert!(sent.is_ok(), "{sent:?}");
    let received = client.recv().await;
    assert!(received.is_ok(), "{received:?}");
    let response = received.unwrap_or(Value::Null);
    assert_eq!(response["id"], id, "{response}");
    response["result"].clone()
}

#[tokio::test]
async fn agent_builds_fractures_and_verifies_a_solid_through_the_sop_operators() {
    let client_res = StdioClient::spawn();
    assert!(client_res.is_ok(), "{client_res:?}");
    let Ok(mut client) = client_res else {
        return;
    };
    assert!(
        client
            .send(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "integration-test", "version": "0.1.0" }
                }
            }))
            .await
            .is_ok()
    );
    assert!(client.recv().await.is_ok());
    assert!(
        client
            .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
            .is_ok()
    );

    // An agent discovers the new operators and their typed parameters through the catalog.
    let catalog = call_tool(
        &mut client,
        2,
        "node_catalog_search",
        json!({ "query": "fracture" }),
    )
    .await;
    let found =
        serde_json::from_value::<NodeCatalogSearchResponse>(catalog["structuredContent"].clone());
    assert!(found.is_ok(), "{found:?}");
    if let Ok(found) = found {
        assert!(
            found
                .node_types
                .iter()
                .any(|node| node.type_name == "voronoifracture")
        );
    }
    let remesh = call_tool(
        &mut client,
        3,
        "node_type_inspect",
        json!({ "type_name": "remesh" }),
    )
    .await;
    let params = &remesh["structuredContent"]["params"];
    assert_eq!(params.as_array().map(Vec::len), Some(3), "{remesh}");
    assert_eq!(params[0]["name"], "target_length");

    // Sphere -> remesh -> decimate, then fracture from surface-scattered seeds pulled inward.
    let node_id = pcc_id::NodeId;
    let unit = |value: f32| Vec3::checked(value, value, value).unwrap_or(Vec3::ZERO);
    let scene = GraphSnapshot {
        revision: RevisionId(1),
        nodes: vec![
            pcc_graph::Node::CreateSphere {
                id: node_id(1),
                radius: 1.0,
                rows: 16,
                cols: 24,
            },
            pcc_graph::Node::Remesh {
                id: node_id(2),
                input: node_id(1),
                target_length: 0.25,
                iterations: 3,
                feature_angle_degrees: 90.0,
            },
            pcc_graph::Node::PolyReduce {
                id: node_id(3),
                input: node_id(2),
                target_ratio: 0.5,
            },
            pcc_graph::Node::ScatterSurface {
                id: node_id(4),
                input: node_id(1),
                count: 8,
                seed: 21,
                min_distance: 0.4,
            },
            pcc_graph::Node::Xform {
                id: node_id(5),
                input: node_id(4),
                translate: Vec3::ZERO,
                rotate_degrees: Vec3::ZERO,
                scale: unit(0.6),
            },
            pcc_graph::Node::VoronoiFracture {
                id: node_id(6),
                input: node_id(3),
                seeds: node_id(5),
            },
        ],
    };
    let replaced = call_tool(
        &mut client,
        4,
        "replace_scene",
        json!({ "base_revision": 1, "ofxs": scene.to_ofxs() }),
    )
    .await;
    assert_eq!(replaced["isError"], false, "{replaced}");
    assert_eq!(replaced["structuredContent"]["committed_revision"], 2);

    // Every shard is closed, and together they keep the volume of a unit sphere within tolerance.
    let report = call_tool(
        &mut client,
        5,
        "geo_assert_invariants",
        json!({
            "frame_numerator": 0,
            "frame_denominator": 24,
            "spec": {
                "min_prims": 100,
                "require_closed_manifold": true,
                "expected_volume": 4.18879,
                "volume_tolerance": 0.2
            }
        }),
    )
    .await;
    assert_eq!(report["isError"], false, "{report}");
    let parsed = serde_json::from_value::<GeoInvariantReport>(report["structuredContent"].clone());
    assert!(parsed.is_ok(), "{parsed:?}");
    if let Ok(parsed) = parsed {
        assert!(parsed.passed, "violations: {:?}", parsed.violations);
        assert_eq!(parsed.probe.revision, RevisionId(2));
        assert!(
            parsed
                .probe
                .point_attributes
                .iter()
                .any(|name| name == "piece")
        );
    }

    client.shutdown().await;
}
