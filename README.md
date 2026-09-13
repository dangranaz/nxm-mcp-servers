# nxm-mcp-servers

Public home for **MCP (Model Context Protocol) servers** in the `nxm` family.

## Servers

| Server | Type | License | Notes |
|--------|------|---------|-------|
| [`nxm-docs-mcp`](nxm-docs-mcp/) | MCP server | MIT / Apache-2.0 | PDF ↔ Markdown converter (fully open) |

### nxm-docs-mcp

A small, dependency-light MCP server that converts documents between **PDF and
Markdown**. Speaks JSON-RPC 2.0 over **stdio**, compatible with Kiro CLI,
Claude Code, Cursor, and other MCP-capable agents.

| Tool | Input | Output |
|------|-------|--------|
| `pdf_to_md` | `input_path` (required), `output_path` (optional) | Markdown |
| `md_to_pdf` | `input_path` (required), `output_path` (optional) | PDF |

## Not published here

- **`nxm-memory`** ships as **interface + docs + a pre-built binary only**; its
  engine **source code is never published** (see private `RULES.md` →
  *Public / Private Split*, MANDATORY). It lives in `dangranaz/nxm-memory`.
- **`nxm-tui-tools`** is a TUI tool library, not an MCP server — it lives under
  [`nxm-tui`](https://github.com/dangranaz/nxm-tui).

## Build

```bash
cargo build --release
# binary at target/release/nxm-docs-mcp
```

## License

Public servers here are licensed permissively. See each server's `LICENSE-*`.
