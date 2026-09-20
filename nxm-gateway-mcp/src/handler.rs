//! MCP request dispatch: `initialize`, `tools/list`, `tools/call`.
//!
//! The gateway advertises the federated + passthrough tool names and routes
//! `tools/call` through `gateway::call_gateway_tool`.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::gateway::{
    self, GatewayState, FANOUT_FIRST_TOOLS, FEDERATED_TOOLS, PASSTHROUGH_TOOLS,
};
use crate::protocol::{codes, ContentBlock, JsonRpcRequest, JsonRpcResponse};

/// Protocol version this server implements.
const PROTOCOL_VERSION: &str = "2026-07-28";

/// Build the advertised tool list from the gateway's tool sets.
fn tools_list() -> Value {
    let describe = |name: &str, desc: &str| {
        json!({
            "name": name,
            "description": desc,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "workspace": { "type": "string", "description": "Route to a single node by name (optional)." },
                    "limit": { "type": "integer" }
                }
            }
        })
    };

    let mut tools = Vec::new();
    for t in FEDERATED_TOOLS {
        tools.push(describe(t, "Federated search across DB nodes, fused with RRF."));
    }
    for t in FANOUT_FIRST_TOOLS {
        tools.push(describe(t, "Resolve from the first node that has the item."));
    }
    for t in PASSTHROUGH_TOOLS {
        tools.push(describe(t, "Forwarded verbatim to one DB node."));
    }
    json!({ "tools": tools })
}

/// Dispatch one JSON-RPC request. Returns `None` for notifications.
pub async fn dispatch(
    state: &Arc<GatewayState>,
    request: &JsonRpcRequest,
) -> Option<JsonRpcResponse> {
    let id = request.id.clone();

    match request.method.as_str() {
        "initialize" => Some(JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "nxm-gateway-mcp", "version": env!("CARGO_PKG_VERSION") }
            }),
        )),

        "notifications/initialized" => None,

        "tools/list" => Some(JsonRpcResponse::ok(id, tools_list())),

        "tools/call" => {
            let name = request
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if name.is_empty() {
                return Some(JsonRpcResponse::error(
                    id,
                    codes::INVALID_PARAMS,
                    "tools/call requires a 'name'",
                ));
            }
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            let result = gateway::call_gateway_tool(state, name, arguments).await;

            // Serialize the ToolResult into the MCP result envelope.
            let content: Vec<Value> = result
                .content
                .iter()
                .map(|c| match c {
                    ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
                })
                .collect();
            let mut envelope = json!({ "content": content });
            if let Some(is_err) = result.is_error {
                envelope["isError"] = json!(is_err);
            }
            Some(JsonRpcResponse::ok(id, envelope))
        }

        other => Some(JsonRpcResponse::error(
            id,
            codes::METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        )),
    }
}
