//! nxm-gateway-mcp — a thin federated MCP gateway.
//!
//! Fans a query out to the nxm-memory DB nodes and fuses their results with
//! Reciprocal Rank Fusion (RRF). Holds no index and loads no model — it is a
//! stateless proxy. This is the ONLY part of the nxm-memory world that is
//! published (see RULES.md → Public / Private Split).
//!
//! SOURCE OF TRUTH for the federated logic: nxm-memory/nxm_mcp/src/gateway.rs.
//! This crate is an alignment destination produced by
//! nxm-memory/scripts/sync-gateway-to-mcp.sh.

pub mod config;
pub mod gateway;
pub mod handler;
pub mod protocol;

pub use gateway::GatewayState;
