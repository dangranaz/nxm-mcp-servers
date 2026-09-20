//! nxm-gateway-mcp binary — JSON-RPC over stdio (one JSON object per line).
//!
//! Transport used by CLI agents (Kiro CLI, Claude Code, Cursor, ...). The
//! gateway resolves its DB nodes from `NXM_GATEWAY_NODES` or
//! `~/.nxm/memory/config.toml` at startup, then serves `tools/call` by fanning
//! out to those nodes and fusing with RRF.

use std::sync::Arc;

use nxm_gateway_mcp::gateway::GatewayState;
use nxm_gateway_mcp::handler;
use nxm_gateway_mcp::protocol::{codes, JsonRpcRequest, JsonRpcResponse};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let state = Arc::new(GatewayState::from_config());
    tracing::info!(
        target: "nxm::gateway",
        "nxm-gateway-mcp v{} starting with {} node(s)",
        env!("CARGO_PKG_VERSION"),
        state.nodes.len()
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => {
                eprintln!("stdin read error: {e}");
                break;
            }
        };
        if n == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                write_response(
                    &mut stdout,
                    &JsonRpcResponse::error(None, codes::PARSE_ERROR, format!("parse error: {e}")),
                )
                .await;
                continue;
            }
        };

        if let Some(response) = handler::dispatch(&state, &request).await {
            write_response(&mut stdout, &response).await;
        }
    }
}

async fn write_response(stdout: &mut tokio::io::Stdout, response: &JsonRpcResponse) {
    match serde_json::to_string(response) {
        Ok(s) => {
            let _ = stdout.write_all(s.as_bytes()).await;
            let _ = stdout.write_all(b"\n").await;
            let _ = stdout.flush().await;
        }
        Err(e) => eprintln!("failed to serialize response: {e}"),
    }
}
