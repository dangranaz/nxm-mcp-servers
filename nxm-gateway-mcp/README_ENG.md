# nxm-gateway-mcp

A **thin federated MCP gateway**. It exposes one MCP endpoint to your agent and
fans each query out to the configured **nxm-memory DB nodes**, fusing their
results with **Reciprocal Rank Fusion (RRF)**.

It holds **no index and loads no model** — it is a stateless proxy. The heavy
lifting (indexing, embedding, search) happens in the DB nodes it talks to.

> Bilingual README — Italian: [`README_ITA.md`](./README_ITA.md).

## What it does

- **Federated search** (`index_search`, `search_code`, `search_docs`,
  `search_exact`, `search_regex`, `memory_recall`): query every node in parallel,
  fuse with RRF, return the top hits with their `source_node`.
- **Fan-out-first** (`get_chunk`): probe all nodes, return the first that has the
  item.
- **Passthrough** (`context_compress`, `context_budget`, `find_symbol`,
  `outline`, `find_references`, `stats`, `memory_remember`, `index_workspace`,
  `workspace_list`, `workspace_create`): forward to one node (routed by an
  optional `workspace` argument, else the first node).

## Configuration

Nodes are resolved at startup, in this order of precedence:

1. `NXM_GATEWAY_NODES` env var — `name:port,name:port,...`
2. `~/.nxm/memory/config.toml`, section `[gateway].nodes`

```toml
[gateway]
[[gateway.nodes]]
name = "engine"
port = 7169
enabled = true
```

## Run

```bash
nxm-gateway-mcp   # JSON-RPC over stdio
```

Configure it in an MCP-compatible agent:

```json
{
  "mcpServers": {
    "nxm-gateway": {
      "command": "nxm-gateway-mcp",
      "env": { "NXM_GATEWAY_NODES": "engine:7169,vendors:7173" }
    }
  }
}
```

## License

Dual-licensed under MIT OR Apache-2.0.
