//! End-to-end test: drive the built binary over stdio like a real MCP client.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_session(input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nxm-docs-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn server");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

#[test]
fn initialize_and_list_tools() {
    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}
";
    let out = run_session(input);
    assert!(out.contains("\"nxm-docs-mcp\""), "serverInfo missing: {out}");
    assert!(out.contains("pdf_to_md"), "pdf_to_md missing: {out}");
    assert!(out.contains("md_to_pdf"), "md_to_pdf missing: {out}");
}

#[test]
fn md_to_pdf_inline_returns_base64_pdf() {
    // Write a temp markdown file and ask the tool to convert it inline.
    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("doc.md");
    std::fs::write(&md_path, "# Hello\n\nWorld.").unwrap();

    let call = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"md_to_pdf\",\"arguments\":{{\"input_path\":\"{}\"}}}}}}\n",
        md_path.display()
    );
    let out = run_session(&call);
    // The result should be base64-encoded PDF (starts with "JVBER" which is base64 for "%PDF")
    assert!(out.contains("JVBER"), "PDF base64 header missing: {out}");
    assert!(out.contains("\"isError\""), "result envelope missing: {out}");
}

#[test]
fn md_to_pdf_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("doc.md");
    let pdf_path = dir.path().join("doc.pdf");
    std::fs::write(&md_path, "# Hello\n\nWorld.").unwrap();

    let call = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"md_to_pdf\",\"arguments\":{{\"input_path\":\"{}\",\"output_path\":\"{}\"}}}}}}\n",
        md_path.display(),
        pdf_path.display()
    );
    let out = run_session(&call);
    assert!(out.contains("Wrote"), "write confirmation missing: {out}");

    // Verify the PDF file was created and starts with %PDF
    let pdf_bytes = std::fs::read(&pdf_path).unwrap();
    assert!(pdf_bytes.starts_with(b"%PDF"), "not a valid PDF");
}
