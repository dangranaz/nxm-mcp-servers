# nxm-docs-mcp

A small, dependency-light **MCP server** that converts documents between **PDF and Markdown**.
It speaks the Model Context Protocol over **stdio**, so any MCP-capable agent
(Kiro CLI, Claude Code, Cursor, …) can call it as a tool.

> Part of the **nxm-tools** family of MCP utilities. This one is fully open
> source (MIT / Apache-2.0).

![Markdown → native PDF, live](https://raw.githubusercontent.com/dangranaz/nxm-docs-mcp/main/docs/assets/demo.gif)

## Tools

| Tool | Input | Output |
|------|-------|--------|
| `pdf_to_md` | `input_path` (required), `output_path` (optional) | Markdown — inline, or written to `output_path` |
| `md_to_pdf` | `input_path` (required), `output_path` (optional) | A **native PDF** — base64 inline, or written to `output_path` |

### Notes on scope

- `md_to_pdf` produces a **real PDF** using an embedded [Typst](https://typst.app)
  typesetting engine — no external tools, no headless browser, no system fonts
  required. The pipeline is Markdown → Typst markup → Typst compilation → PDF.
  When `output_path` is omitted the PDF is returned **base64-encoded** in the
  tool result.
- `pdf_to_md` performs **best-effort text extraction** from the PDF's text
  layer. It reconstructs reading order from glyph positions (full affine text +
  CTM tracking) and promotes larger-font lines to Markdown headings, so it
  survives round-tripping through Typst-generated PDFs (Type0/Identity-H fonts
  with `ToUnicode` CMaps) and real-world CVs. It does **not** OCR scanned or
  image-only PDFs, and does not reconstruct complex layout (multi-column, tables).

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

Render Markdown to a native PDF, returned base64-encoded inline:

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
