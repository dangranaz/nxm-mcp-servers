//! Standalone gateway configuration.
//!
//! The gateway needs exactly one thing to work: the list of DB nodes to
//! federate over (`name` → `port`). It reads them from the same TOML file
//! nxm-memory uses (`~/.nxm/memory/config.toml`, `[gateway].nodes`), but WITHOUT
//! depending on the engine crate — only the two fields it cares about are
//! deserialized. Nodes can also be provided via the `NXM_GATEWAY_NODES`
//! environment variable (`name:port,name:port,...`), which wins over the file.

use std::path::PathBuf;

use serde::Deserialize;

/// A single federated DB node.
#[derive(Debug, Clone)]
pub struct GatewayNodeRef {
    pub name: String,
    pub port: u16,
}

/// Minimal view of the TOML: we only read `[gateway].nodes`.
#[derive(Debug, Default, Deserialize)]
struct MinimalConfig {
    #[serde(default)]
    gateway: MinimalGateway,
}

#[derive(Debug, Default, Deserialize)]
struct MinimalGateway {
    #[serde(default)]
    nodes: Vec<MinimalNode>,
}

#[derive(Debug, Deserialize)]
struct MinimalNode {
    name: String,
    port: u16,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Default config path: `~/.nxm/memory/config.toml`.
fn default_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".nxm/memory/config.toml"))
}

/// Parse `NXM_GATEWAY_NODES` (`name:port,name:port`) if present.
fn nodes_from_env() -> Option<Vec<GatewayNodeRef>> {
    let raw = std::env::var("NXM_GATEWAY_NODES").ok()?;
    let mut out = Vec::new();
    for pair in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, port) = pair.rsplit_once(':')?;
        let port: u16 = port.trim().parse().ok()?;
        out.push(GatewayNodeRef { name: name.trim().to_string(), port });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Resolve the enabled DB nodes for the federation.
///
/// Order of precedence: `NXM_GATEWAY_NODES` env → the TOML config file → empty.
/// An empty node set is not an error here; the server starts and each tool call
/// returns a clear "no nodes configured" message.
pub fn load_nodes() -> Vec<GatewayNodeRef> {
    if let Some(nodes) = nodes_from_env() {
        tracing::info!(target: "nxm::gateway::config", "loaded {} node(s) from NXM_GATEWAY_NODES", nodes.len());
        return nodes;
    }

    let path = match default_config_path() {
        Some(p) => p,
        None => {
            tracing::warn!(target: "nxm::gateway::config", "HOME not set; no config; no nodes");
            return Vec::new();
        }
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target: "nxm::gateway::config", "config {} unreadable: {e}; no nodes", path.display());
            return Vec::new();
        }
    };

    match toml::from_str::<MinimalConfig>(&text) {
        Ok(cfg) => cfg
            .gateway
            .nodes
            .into_iter()
            .filter(|n| n.enabled)
            .map(|n| GatewayNodeRef { name: n.name, port: n.port })
            .collect(),
        Err(e) => {
            tracing::warn!(target: "nxm::gateway::config", "config parse failed: {e}; no nodes");
            Vec::new()
        }
    }
}
