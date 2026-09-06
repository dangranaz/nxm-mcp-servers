//! `nxm-tools` — pure-Rust actuator set for the Nexum agentic TUI.
//!
//! A single crate holding the tools the OpenAI agent calls (`web_search`,
//! `read_file`, `list_resources`). `nxm-tui`'s `Agent` (src/agent.rs) depends on
//! this crate's [`Tool`] trait + [`Registry`] (P2-proper wiring). v0.1 scaffold:
//! blocking I/O is intentional — P2-proper makes `Tool::run` async.
//!
//! Run: `cargo test --workspace` (lib + integration tests under `tests/`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tracing::info;

/// Tracing target per `RULES.md` (`nexum::<module>::<feature>`).
pub const TARGET: &str = "nexum::tools::runtime";

/// Outcome of a tool run, returned to the model as a `role: "tool"` result.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Whether the tool succeeded.
    pub ok: bool,
    /// Text content handed back to the model.
    pub content: String,
}

impl ToolResult {
    pub fn ok(content: String) -> Self {
        Self { ok: true, content }
    }
    pub fn err(msg: String) -> Self {
        Self { ok: false, content: msg }
    }
}

/// A tool callable by the OpenAI agent. Implementations are stateless and
/// `Send + Sync` so the [`Registry`] can hold `Arc<dyn Tool>`.
pub trait Tool: Send + Sync {
    /// Canonical, kebab-case name the model calls (e.g. `read_file`).
    fn name(&self) -> &str;
    /// Human-readable description given to the model.
    fn description(&self) -> &str;
    /// JSON schema (`function.parameters`) for the tool's arguments.
    fn schema(&self) -> Value;
    /// Execute the tool. Synchronous in v0.1 (P2-proper: make `async`).
    fn run(&self, args: &Value) -> Result<ToolResult>;
}

/// Registry of available tools + OpenAI manifest builder.
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        info!(target: TARGET, "register tool `{name}`", name = tool.name());
        self.tools.push(Arc::new(tool));
    }

    /// Look up a tool by name (clones the `Arc`, no cloning of the tool itself).
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Execute a named tool. Returns an `Err` only if the tool is unknown or
    /// the binary throws; tool-level failures come back as `ToolResult::err`.
    pub fn call(&self, name: &str, args: &Value) -> Result<ToolResult> {
        let tool = self
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool `{name}`"))?;
        tool.run(args)
    }

    /// Build the OpenAI `tools` manifest sent in the chat request.
    pub fn manifest(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.schema(),
                    }
                })
            })
            .collect()
    }
}

/// Read the full contents of a file (`args.path`).
pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read the full contents of a file on disk."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or relative file path." }
            },
            "required": ["path"]
        })
    }
    fn run(&self, args: &Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("`path` argument is required"))?;
        let content = std::fs::read_to_string(PathBuf::from(path))?;
        Ok(ToolResult::ok(content))
    }
}

/// List entries in a directory (`args.path`, default cwd).
pub struct ListResources;

impl Tool for ListResources {
    fn name(&self) -> &str {
        "list_resources"
    }
    fn description(&self) -> &str {
        "List files and directories in a path."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to list; defaults to cwd." }
            },
            "required": []
        })
    }
    fn run(&self, args: &Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let mut names: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(PathBuf::from(path))? {
            let entry = entry?;
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        names.sort();
        Ok(ToolResult::ok(json!({ "entries": names }).to_string()))
    }
}

/// Web search via a configurable HTTP endpoint (Nexum engine / provider).
/// Endpoint from `NEXUM_SEARCH_ENDPOINT` (default `http://localhost:8080`).
pub struct WebSearch {
    endpoint: String,
}

impl WebSearch {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

impl Default for WebSearch {
    fn default() -> Self {
        let ep = std::env::var("NEXUM_SEARCH_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());
        Self::new(ep)
    }
}

impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web via a configured HTTP search endpoint."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query." }
            },
            "required": ["query"]
        })
    }
    fn run(&self, args: &Value) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("`query` argument is required"))?;
        let mut url = reqwest::Url::parse(&format!(
            "{}/search",
            self.endpoint.trim_end_matches('/')
        ))?;
        url.query_pairs_mut().append_pair("q", query);
        let resp = reqwest::blocking::get(url.as_str())?;
        if !resp.status().is_success() {
            return Ok(ToolResult::err(format!(
                "search endpoint returned {}",
                resp.status()
            )));
        }
        let text = resp.text()?;
        Ok(ToolResult::ok(text))
    }
}

/// Build the default MVP registry (read_file, list_resources, web_search).
pub fn default_registry() -> Registry {
    let mut reg = Registry::new();
    reg.register(ReadFile);
    reg.register(ListResources);
    reg.register(WebSearch::default());
    reg
}
