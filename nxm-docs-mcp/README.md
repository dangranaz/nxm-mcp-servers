# nxm-docs-mcp

A small, dependency-light **MCP server** that converts documents between **PDF and Markdown**.
It speaks the Model Context Protocol over **stdio**, so any MCP-capable agent
(Kiro CLI, Claude Code, Cursor, …) can call it as a tool.

> Part of the **nxm-tools** family of MCP utilities. This one is fully open
> source (MIT / Apache-2.0).

## Tools

| Tool | Input | Output |
|------|-------|--------|
| `pdf_to_md` | `input_path` (required), `output_path` (optional) | Markdown — inline, or written to `output_path` |
| `md_to_pdf` | `input_path` (required), `output_path` (optional) | Standalone HTML ready for "Print to PDF" — inline, or written to `output_path` |

### Notes on scope

- `pdf_to_md` performs **best-effort text extraction**: it reads the PDF's text
  layer. It does **not** OCR scanned/image-only PDFs and does not reconstruct
  complex layout (multi-column, tables).
- `md_to_pdf` renders Markdown to a clean **HTML** document. True PDF rendering
  (fonts, pagination) needs a heavy native engine, deliberately kept out of this
  tiny tool — open the HTML in a browser and "Print → Save as PDF", or pipe it
  through `wkhtmltopdf`.

## Build

```bash
cargo build --release
# binary at target/release/nxm-docs-mcp
```

## Run (manual smoke test)

The server reads one JSON-RPC object per line on stdin and replies on stdout:

```bash
printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | ./target/release/nxm-docs-mcp
```

## Register as an MCP server

### Kiro CLI / Claude Code (stdio)

Add to your MCP config (e.g. `~/.aws/amazonq/mcp.json` for Kiro, or the client's
`mcpServers` block):

```json
{
  "mcpServers": {
    "nxm-docs": {
      "command": "/absolute/path/to/target/release/nxm-docs-mcp",
      "args": [],
      "env": {}
    }
  }
}
```

Then, in a prompt: *"use nxm-docs to convert `~/report.pdf` to Markdown at `~/report.md`"*.

## Example calls

Convert a PDF to a Markdown file:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
  "name":"pdf_to_md",
  "arguments":{"input_path":"/abs/report.pdf","output_path":"/abs/report.md"}}}
```

Render Markdown to HTML inline:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
  "name":"md_to_pdf",
  "arguments":{"input_path":"/abs/notes.md"}}}
```

## Test

```bash
cargo test
```

Unit tests cover the conversion core; an integration test drives the built
binary over stdio like a real MCP client.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
