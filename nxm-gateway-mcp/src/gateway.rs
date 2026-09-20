//! Federated query gateway — a thin MCP client that fans a search out to the
//! DB nodes and fuses their results with cross-node RRF.
//!
//! SOURCE OF TRUTH: nxm-memory/nxm_mcp/src/gateway.rs. This file is an alignment
//! copy (see the crate's Cargo.toml alignment note). The federated logic is
//! identical; only the config source differs (standalone `config::load_nodes`
//! instead of the engine's `NxmConfig`).
//!
//! - The gateway exposes ONE MCP endpoint to the agent (`index_search`,
//!   `search_code`, `search_docs`, `memory_recall`, ...).
//! - It knows the DB nodes from config (`[gateway].nodes`, `name → port`).
//! - Default: fan-out to every enabled node in parallel, then fuse with
//!   Reciprocal Rank Fusion (RRF). Optional `workspace` argument routes the
//!   query directly to a single node.
//! - The nodes stay separate processes; the gateway is stateless and thin
//!   (it holds no index and loads no model).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{self, GatewayNodeRef};
use crate::protocol::{ContentBlock, ToolResult};

/// RRF constant. 60 is the canonical value from the original RRF paper and the
/// same k nxm-memory uses for intra-node BM25+dense fusion — reused here for
/// cross-node fusion so behavior is consistent end to end.
const RRF_K: f64 = 60.0;

/// Gateway state: the set of nodes to federate over + a blocking HTTP client.
pub struct GatewayState {
    /// Enabled DB nodes, in config order.
    pub nodes: Vec<GatewayNodeRef>,
    /// Reusable blocking HTTP agent (ureq). Calls are wrapped in
    /// `spawn_blocking` at the call site so they don't stall the tokio runtime.
    agent: ureq::Agent,
}

impl GatewayState {
    /// Build gateway state from the resolved node list.
    pub fn new(nodes: Vec<GatewayNodeRef>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build();
        Self { nodes, agent }
    }

    /// Build gateway state from the standalone config resolver
    /// (`NXM_GATEWAY_NODES` env or `~/.nxm/memory/config.toml`).
    pub fn from_config() -> Self {
        Self::new(config::load_nodes())
    }

    /// Resolve the nodes to query for this request.
    fn target_nodes(&self, workspace: Option<&str>) -> Vec<GatewayNodeRef> {
        match workspace {
            Some(ws) => self.nodes.iter().filter(|n| n.name == ws).cloned().collect(),
            None => self.nodes.clone(),
        }
    }

    /// Call one node's `tools/call <tool>` over MCP HTTP (blocking).
    fn call_node(
        agent: &ureq::Agent,
        port: u16,
        tool: &str,
        arguments: &Value,
    ) -> Result<Vec<Value>, String> {
        let url = format!("http://127.0.0.1:{port}/mcp");
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        });

        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&serde_json::to_string(&request).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{tool}@{port}: {e}"))?;

        let json_resp: Value = response.into_json().map_err(|e| e.to_string())?;

        if let Some(err) = json_resp.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown JSON-RPC error");
            return Err(format!("{tool}@{port}: {msg}"));
        }

        if json_resp
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let msg = json_resp
                .get("result")
                .and_then(|r| r.get("content"))
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("tool reported an error");
            return Err(format!("{tool}@{port}: {msg}"));
        }

        let text = json_resp
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| format!("{tool}@{port}: unexpected response shape"))?;

        let hits: Value = serde_json::from_str(text)
            .map_err(|e| format!("{tool}@{port}: result not JSON: {e}"))?;

        match hits {
            Value::Array(a) => Ok(a),
            Value::Object(ref map) => {
                if let Some(Value::Array(results)) = map.get("results") {
                    Ok(results.clone())
                } else {
                    Ok(vec![hits])
                }
            }
            other => Ok(vec![other]),
        }
    }

    /// Federate a search `tool` across the target nodes and fuse with RRF.
    pub async fn federated_search(self: &Arc<Self>, tool: &str, arguments: Value) -> ToolResult {
        let workspace = arguments
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let targets = self.target_nodes(workspace.as_deref());
        if targets.is_empty() {
            return error_result(match workspace {
                Some(ws) => format!("no federated node named '{ws}'"),
                None => "no federated nodes configured (see [gateway].nodes)".to_string(),
            });
        }

        let mut node_args = arguments.clone();
        if let Some(obj) = node_args.as_object_mut() {
            obj.remove("workspace");
        }

        let mut handles = Vec::with_capacity(targets.len());
        for node in targets {
            let this = Arc::clone(self);
            let tool = tool.to_string();
            let args = node_args.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                let res = Self::call_node(&this.agent, node.port, &tool, &args);
                (node.name, res)
            }));
        }

        let mut per_node: Vec<(String, Vec<Value>)> = Vec::new();
        let mut node_errors: Vec<String> = Vec::new();
        for h in handles {
            if let Ok((name, res)) = h.await {
                match res {
                    Ok(hits) => per_node.push((name, hits)),
                    Err(e) => {
                        tracing::warn!(target: "nxm::gateway", "node '{name}' failed: {e}");
                        node_errors.push(e);
                    }
                }
            }
        }

        if per_node.is_empty() {
            let detail = if node_errors.is_empty() {
                "all federated nodes returned nothing".to_string()
            } else {
                format!("all federated nodes failed: {}", node_errors.join("; "))
            };
            return error_result(detail);
        }

        let fused = rrf_fuse(per_node, limit);
        ToolResult {
            content: vec![ContentBlock::Text {
                text: serde_json::to_string_pretty(&fused).unwrap_or_default(),
            }],
            is_error: None,
        }
    }

    /// Forward a stateless tool to ONE node and return its ToolResult verbatim.
    pub async fn passthrough(self: &Arc<Self>, tool: &str, arguments: Value) -> ToolResult {
        let workspace = arguments
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let node = match &workspace {
            Some(ws) => self.nodes.iter().find(|n| &n.name == ws).cloned(),
            None => self.nodes.first().cloned(),
        };
        let node = match node {
            Some(n) => n,
            None => {
                return error_result(match workspace {
                    Some(ws) => format!("no node named '{ws}' to run '{tool}'"),
                    None => format!("no nodes configured to run '{tool}'"),
                })
            }
        };

        let mut node_args = arguments.clone();
        if let Some(obj) = node_args.as_object_mut() {
            obj.remove("workspace");
        }

        let this = Arc::clone(self);
        let tool_s = tool.to_string();
        let joined = tokio::task::spawn_blocking(move || {
            Self::call_node_raw(&this.agent, node.port, &tool_s, &node_args)
        })
        .await;

        match joined {
            Ok(Ok(text)) => ToolResult {
                content: vec![ContentBlock::Text { text }],
                is_error: None,
            },
            Ok(Err(e)) => error_result(format!("{tool} passthrough failed: {e}")),
            Err(e) => error_result(format!("{tool} passthrough task error: {e}")),
        }
    }

    /// Call `tools/call <tool>` on one node and return the tool's text payload.
    fn call_node_raw(
        agent: &ureq::Agent,
        port: u16,
        tool: &str,
        arguments: &Value,
    ) -> Result<String, String> {
        let url = format!("http://127.0.0.1:{port}/mcp");
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        });
        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&serde_json::to_string(&request).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{tool}@{port}: {e}"))?;
        let json_resp: Value = response.into_json().map_err(|e| e.to_string())?;
        json_resp
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("{tool}@{port}: unexpected response shape"))
    }

    /// Resolve a tool by asking every node and returning the FIRST successful,
    /// non-error, non-empty answer (used for `get_chunk`).
    pub async fn fanout_first(self: &Arc<Self>, tool: &str, arguments: Value) -> ToolResult {
        let workspace = arguments
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let targets = self.target_nodes(workspace.as_deref());
        if targets.is_empty() {
            return error_result(format!("no nodes to resolve '{tool}'"));
        }

        let mut node_args = arguments.clone();
        if let Some(obj) = node_args.as_object_mut() {
            obj.remove("workspace");
        }

        let mut handles = Vec::with_capacity(targets.len());
        for node in targets {
            let this = Arc::clone(self);
            let tool = tool.to_string();
            let args = node_args.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                Self::call_node_raw(&this.agent, node.port, &tool, &args)
            }));
        }

        for h in handles {
            if let Ok(Ok(text)) = h.await {
                let t = text.trim();
                let looks_empty = t.is_empty()
                    || t.contains("not found")
                    || t.contains("No chunk")
                    || t == "{}"
                    || t == "[]";
                if !looks_empty {
                    return ToolResult {
                        content: vec![ContentBlock::Text { text }],
                        is_error: None,
                    };
                }
            }
        }
        error_result(format!("'{tool}': no node returned a result"))
    }
}

/// Stable key so the same chunk from two nodes is fused, not double-counted.
fn hit_key(hit: &Value) -> String {
    if let Some(id) = hit.get("chunk_id").and_then(|v| v.as_str()) {
        return format!("id:{id}");
    }
    let fp = hit.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let sl = hit
        .get("start_line")
        .or_else(|| hit.get("line_number"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!("loc:{fp}:{sl}")
}

/// Reciprocal Rank Fusion across nodes.
fn rrf_fuse(per_node: Vec<(String, Vec<Value>)>, limit: usize) -> Vec<Value> {
    struct Acc {
        score: f64,
        best_rank: usize,
        source: String,
        hit: Value,
    }
    let mut acc: HashMap<String, Acc> = HashMap::new();

    for (node_name, hits) in per_node {
        for (rank, hit) in hits.into_iter().enumerate() {
            let key = hit_key(&hit);
            let contrib = 1.0 / (RRF_K + rank as f64);
            acc.entry(key)
                .and_modify(|a| {
                    a.score += contrib;
                    if rank < a.best_rank {
                        a.best_rank = rank;
                        a.source = node_name.clone();
                        a.hit = hit.clone();
                    }
                })
                .or_insert(Acc {
                    score: contrib,
                    best_rank: rank,
                    source: node_name.clone(),
                    hit,
                });
        }
    }

    let mut fused: Vec<Acc> = acc.into_values().collect();
    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.best_rank.cmp(&b.best_rank))
    });
    fused.truncate(limit);

    fused
        .into_iter()
        .map(|a| {
            let mut obj = a.hit;
            if let Some(map) = obj.as_object_mut() {
                map.insert("source_node".to_string(), json!(a.source));
                map.insert("rrf_score".to_string(), json!(format!("{:.6}", a.score)));
            }
            obj
        })
        .collect()
}

/// Helper: build an error `ToolResult`.
fn error_result(msg: String) -> ToolResult {
    ToolResult {
        content: vec![ContentBlock::Text { text: msg }],
        is_error: Some(true),
    }
}

/// Search tools the gateway federates across nodes (fan-out + RRF).
pub const FEDERATED_TOOLS: &[&str] = &[
    "index_search",
    "search_code",
    "search_docs",
    "search_exact",
    "search_regex",
    "memory_recall",
];

/// Stateless / node-local tools the gateway forwards to ONE node verbatim.
pub const PASSTHROUGH_TOOLS: &[&str] = &[
    "context_compress",
    "context_budget",
    "find_symbol",
    "outline",
    "find_references",
    "stats",
    "watcher_status",
    "memory_remember",
    "index_workspace",
    "workspace_list",
    "workspace_create",
];

/// Tools resolved by asking every node and taking the FIRST non-empty answer.
pub const FANOUT_FIRST_TOOLS: &[&str] = &["get_chunk"];

/// Dispatch a gateway `tools/call`.
pub async fn call_gateway_tool(
    state: &Arc<GatewayState>,
    tool: &str,
    arguments: Value,
) -> ToolResult {
    if FEDERATED_TOOLS.contains(&tool) {
        state.federated_search(tool, arguments).await
    } else if FANOUT_FIRST_TOOLS.contains(&tool) {
        state.fanout_first(tool, arguments).await
    } else if PASSTHROUGH_TOOLS.contains(&tool) {
        state.passthrough(tool, arguments).await
    } else {
        error_result(format!(
            "tool '{tool}' is not exposed by the gateway. Query a DB node \
             directly for anything not in the federated/passthrough sets."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(fp: &str, line: u64, id: &str) -> Value {
        json!({ "file_path": fp, "start_line": line, "chunk_id": id, "score": "0.5" })
    }

    #[test]
    fn rrf_ranks_agreement_higher() {
        let node_a = vec![hit("a.rs", 1, "A"), hit("b.rs", 2, "B")];
        let node_b = vec![hit("a.rs", 1, "A"), hit("c.rs", 3, "C")];
        let fused = rrf_fuse(vec![("na".into(), node_a), ("nb".into(), node_b)], 10);
        assert_eq!(fused[0]["chunk_id"], "A", "shared top hit wins");
        assert_eq!(fused.len(), 3);
    }

    #[test]
    fn rrf_dedups_by_chunk_id() {
        let node_a = vec![hit("a.rs", 1, "X")];
        let node_b = vec![hit("a.rs", 1, "X")];
        let fused = rrf_fuse(vec![("na".into(), node_a), ("nb".into(), node_b)], 10);
        assert_eq!(fused.len(), 1, "same chunk_id fused into one");
        assert_eq!(fused[0]["chunk_id"], "X");
    }

    #[test]
    fn rrf_respects_limit() {
        let many: Vec<Value> = (0..20).map(|i| hit("f.rs", i, &format!("id{i}"))).collect();
        let fused = rrf_fuse(vec![("na".into(), many)], 5);
        assert_eq!(fused.len(), 5);
    }

    #[test]
    fn target_nodes_routes_by_workspace() {
        let gs = GatewayState::new(vec![
            GatewayNodeRef { name: "engine".into(), port: 7169 },
            GatewayNodeRef { name: "vendors".into(), port: 7173 },
        ]);
        assert_eq!(gs.target_nodes(Some("vendors")).len(), 1);
        assert_eq!(gs.target_nodes(Some("vendors"))[0].port, 7173);
        assert_eq!(gs.target_nodes(None).len(), 2);
        assert!(gs.target_nodes(Some("nope")).is_empty());
    }
}
