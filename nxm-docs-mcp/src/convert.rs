//! Document conversion core.
//!
//! Two directions, both dependency-light and offline:
//! - [`pdf_to_md`]: extract text from a PDF into Markdown (best-effort, text layer only).
//! - [`md_to_html`]: render Markdown to a standalone HTML document.
//!
//! This module has no knowledge of MCP or JSON-RPC — it is a plain library so it
//! can be unit-tested and reused independently.

use anyhow::{Context, Result};
use pulldown_cmark::{html, Options, Parser};
use std::path::Path;

use crate::md_to_typst;
use crate::world::NxmWorld;

/// Extract the text layer of a PDF and return it as Markdown.
///
/// This is a *best-effort text extraction*: it reads the embedded text of each
/// page and separates pages with a Markdown horizontal rule. It does **not**
/// perform OCR (scanned/image-only PDFs will yield little or no text) and does
/// not attempt to reconstruct complex layout (tables, multi-column).
pub fn pdf_to_md(pdf_bytes: &[u8]) -> Result<String> {
    let doc = lopdf::Document::load_mem(pdf_bytes)
        .context("failed to parse PDF (is the file a valid PDF?)")?;

    let mut out = String::new();
    // `get_pages` returns a map of page-number -> object id, ordered by page number.
    let pages = doc.get_pages();
    let total = pages.len();

    for (idx, (&page_number, _)) in pages.iter().enumerate() {
        let text = doc
            .extract_text(&[page_number])
            .unwrap_or_default();
        let text = normalize_extracted_text(&text);
        if !text.is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
        // Separate pages with a horizontal rule, except after the last page.
        if idx + 1 < total {
            out.push_str("\n---\n\n");
        }
    }

    Ok(out.trim_end().to_string() + "\n")
}

/// Convenience wrapper: read a PDF file from disk and convert it to Markdown.
pub fn pdf_file_to_md(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read PDF file: {}", path.display()))?;
    pdf_to_md(&bytes)
}

/// Render a Markdown string to a standalone HTML document.
///
/// A true Markdown→PDF conversion requires a PDF rendering engine (layout,
/// fonts, pagination) which is intentionally out of scope for this small,
/// dependency-light tool. Producing clean HTML lets any browser or `wkhtmltopdf`
/// / "Print to PDF" step complete the last mile without pulling a heavy native
/// rendering stack into this crate.
#[allow(dead_code)]
pub fn md_to_html(markdown: &str, title: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);
    let mut body = String::new();
    html::push_html(&mut body, parser);

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
<style>body{{max-width:48rem;margin:2rem auto;padding:0 1rem;\
font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;\
line-height:1.6}}pre{{background:#f5f5f5;padding:1rem;overflow:auto}}\
code{{font-family:ui-monospace,monospace}}table{{border-collapse:collapse}}\
td,th{{border:1px solid #ddd;padding:.4rem .6rem}}</style>\n\
</head>\n<body>\n{body}</body>\n</html>\n",
        title = escape_html(title),
        body = body,
    )
}

/// Convenience wrapper: read a Markdown file from disk and render it to HTML.
#[allow(dead_code)]
pub fn md_file_to_html(path: &Path) -> Result<String> {
    let markdown = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Markdown file: {}", path.display()))?;
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    Ok(md_to_html(&markdown, title))
}

/// Collapse the noisy whitespace that PDF text extraction often produces.
fn normalize_extracted_text(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // Collapse runs of internal whitespace to single spaces.
        let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(collapsed);
    }
    // Drop leading/trailing empty lines but keep single blank lines between blocks.
    let joined = lines.join("\n");
    joined.trim().to_string()
}

/// Minimal HTML escaping for text placed into element content / title.
#[allow(dead_code)]
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert a Markdown string to a native PDF document.
///
/// Uses Typst as the typesetting engine. The pipeline is:
/// Markdown → Typst markup → Typst compilation → PDF export.
pub fn md_to_pdf(markdown: &str) -> Result<Vec<u8>> {
    let typst_src = md_to_typst::convert(markdown);
    let world = NxmWorld::new(&typst_src);
    let warned = typst::compile(&world);
    let document = warned.output.map_err(|errors| {
        let msgs: Vec<String> = errors.iter().map(|e| format!("{:?}", e)).collect();
        anyhow::anyhow!("Typst compilation failed:\n{}", msgs.join("\n"))
    })?;
    let pdf_bytes = typst_pdf::pdf(&document, &Default::default())
        .map_err(|errors| {
            let msgs: Vec<String> = errors.iter().map(|e| format!("{:?}", e)).collect();
            anyhow::anyhow!("PDF export failed:\n{}", msgs.join("\n"))
        })?;
    Ok(pdf_bytes)
}

/// Convenience wrapper: read a Markdown file from disk and convert it to PDF.
pub fn md_file_to_pdf(path: &Path) -> Result<Vec<u8>> {
    let markdown = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Markdown file: {}", path.display()))?;
    md_to_pdf(&markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md_to_html_renders_heading_and_paragraph() {
        let html = md_to_html("# Title\n\nHello **world**.", "doc");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>world</strong>"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn md_to_html_supports_tables() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |";
        let html = md_to_html(md, "t");
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn md_to_html_escapes_title() {
        let html = md_to_html("x", "a<b>&c");
        assert!(html.contains("<title>a&lt;b&gt;&amp;c</title>"));
    }

    #[test]
    fn normalize_collapses_whitespace() {
        let got = normalize_extracted_text("  hello    world  \n\n  foo \t bar ");
        assert_eq!(got, "hello world\n\nfoo bar");
    }

    #[test]
    fn pdf_to_md_rejects_garbage() {
        let err = pdf_to_md(b"not a pdf at all").unwrap_err();
        assert!(err.to_string().contains("PDF"));
    }
}
