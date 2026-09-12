# nxm-mcp-servers

Public home for **MCP (Model Context Protocol) servers** in the `nxm` family.

> **Status: transition in progress.** The servers are being reorganized. The
> source of truth is a private umbrella (`nxm-mcp-servers`), from which the
> public servers are derived. This public repo will be repopulated with the
> open-source servers shortly.

## Planned public content

| Server | Type | License | Notes |
|--------|------|---------|-------|
| `nxm-docs-mcp` | MCP server | MIT / Apache-2.0 | PDF ↔ Markdown converter (fully open) |

### Not published here

- **`nxm-memory`** ships as **interface + docs + a pre-built binary only**. Its
  engine **source code is never published** — see the private `RULES.md`
  (*Public / Private Split*, MANDATORY). It lives in its own distribution
  (`dangranaz/nxm-memory`), not in this repo.
- **`nxm-tui-tools`** is a TUI tool library, not an MCP server — it now lives
  under [`nxm-tui`](https://github.com/dangranaz/nxm-tui).

## License

Public servers here are licensed permissively (MIT / Apache-2.0). See each
server's own `LICENSE-*` files.
