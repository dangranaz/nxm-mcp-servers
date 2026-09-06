//! Integration tests for `nxm_tools` (RULES: tests under `tests/`, not inline).
//! Exercises the real MVP tools (no network): `read_file`, `list_resources`,
//! plus the registry `call`/`manifest` plumbing.

use std::fs;

use nxm_tools::default_registry;

#[test]
fn read_file_returns_file_contents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("note.txt");
    fs::write(&path, "hello-tool").expect("write");
    let reg = default_registry();
    let args = serde_json::json!({ "path": path.to_string_lossy() });
    let out = reg.call("read_file", &args).expect("read_file fails");
    assert!(out.ok);
    assert_eq!(out.content, "hello-tool");
}

#[test]
fn list_resources_lists_directory_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("a.txt"), "a").expect("write");
    let reg = default_registry();
    let args = serde_json::json!({ "path": dir.path().to_string_lossy() });
    let out = reg.call("list_resources", &args).expect("list fails");
    assert!(out.ok);
    let v: serde_json::Value = serde_json::from_str(&out.content).expect("parse");
    let entries = v["entries"].as_array().expect("entries array");
    let names: Vec<String> = entries
        .iter()
        .map(|x| x.as_str().expect("name").to_string())
        .collect();
    assert!(names.contains(&"a.txt".to_string()));
}

#[test]
fn manifest_exposes_the_mvp_tool_set() {
    let names: Vec<String> = default_registry()
        .manifest()
        .iter()
        .map(|m| m["function"]["name"].as_str().expect("name").to_string())
        .collect();
    assert_eq!(names, ["read_file", "list_resources", "web_search"]);
}

#[test]
fn unknown_tool_call_is_an_error() {
    let reg = default_registry();
    let err = reg.call("no_such_tool", &serde_json::json!({})).unwrap_err();
    assert!(err.to_string().contains("unknown tool"));
}
