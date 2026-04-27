//! JSON-RPC 2.0 envelope compliance for `plenum mcp`.
//!
//! Drives the compiled `plenum mcp` binary over stdio and asserts that every
//! emitted line is a valid JSON-RPC 2.0 Response — never a Notification, never
//! `id: null`, never with extra top-level keys. Regression coverage for the
//! Claude Code Zod-strict validation incident where replying to
//! `notifications/initialized` dropped the stdio connection before
//! `tools/list` ever ran.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run the canonical MCP handshake against `plenum mcp` and collect every
/// emitted stdout line until two responses are seen or a deadline elapses.
fn run_handshake() -> Vec<Value> {
    let bin = env!("CARGO_BIN_EXE_plenum");

    let mut child = Command::new(bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plenum mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "plenum-test", "version": "0" }
        }
    });
    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let list_tools = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    writeln!(stdin, "{initialize}").unwrap();
    writeln!(stdin, "{initialized}").unwrap();
    writeln!(stdin, "{list_tools}").unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let mut reader = BufReader::new(stdout);
    let mut lines: Vec<Value> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline && lines.len() < 2 {
        let mut buf = String::new();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(trimmed)
                    .unwrap_or_else(|e| panic!("non-JSON output on stdout: {e}: {trimmed:?}"));
                lines.push(value);
            }
            Err(e) => panic!("read stdout: {e}"),
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    lines
}

fn assert_valid_response(value: &Value) {
    let obj = value.as_object().expect("response is a JSON object");

    assert_eq!(
        obj.get("jsonrpc").and_then(Value::as_str),
        Some("2.0"),
        "jsonrpc field must be \"2.0\": {value}"
    );

    let id = obj.get("id").expect("response must have an id (notifications must not get replies)");
    assert!(
        id.is_number() || id.is_string(),
        "id must be a number or string per JSON-RPC 2.0 strict schemas, got {id} in {value}"
    );

    let has_result = obj.contains_key("result");
    let has_error = obj.contains_key("error");
    assert!(
        has_result ^ has_error,
        "exactly one of result/error must be present: {value}"
    );

    let allowed: HashSet<&str> = ["jsonrpc", "id", "result", "error"].into_iter().collect();
    for key in obj.keys() {
        assert!(
            allowed.contains(key.as_str()),
            "unexpected top-level key {key:?} would be rejected by strict Zod schema: {value}"
        );
    }
}

#[test]
fn mcp_handshake_emits_only_valid_responses() {
    let lines = run_handshake();

    assert_eq!(
        lines.len(),
        2,
        "expected exactly 2 responses (one per request, none for the notification), got {}: {:#?}",
        lines.len(),
        lines
    );

    for line in &lines {
        assert_valid_response(line);
    }

    let init = &lines[0];
    assert_eq!(init["id"], json!(1));
    assert_eq!(init["result"]["protocolVersion"], json!("2024-11-05"));

    let tools = &lines[1];
    assert_eq!(tools["id"], json!(2));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"introspect"), "tools list missing introspect: {names:?}");
    assert!(names.contains(&"query"), "tools list missing query: {names:?}");
}

#[test]
fn mcp_does_not_respond_to_unparseable_input() {
    let bin = env!("CARGO_BIN_EXE_plenum");

    let mut child = Command::new(bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plenum mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    writeln!(stdin, "this is not json").unwrap();
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "initialize",
        "params": {}
    });
    writeln!(stdin, "{initialize}").unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    reader.read_line(&mut buf).expect("read stdout");
    let value: Value = serde_json::from_str(buf.trim())
        .unwrap_or_else(|e| panic!("first stdout line not JSON: {e}: {buf:?}"));

    assert_valid_response(&value);
    assert_eq!(value["id"], json!(42), "first response should be for the initialize request, not the garbage line: {value}");

    let _ = child.kill();
    let _ = child.wait();
}
