//! `tessel mcp`, the real binary, between a client on its stdio
//! and a headless window's socket (§6.1).

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::Harness;
use serde_json::{json, Value};

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<Value>,
}

impl Bridge {
    fn start(args: &[&str]) -> Bridge {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tessel"))
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let v = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("stdout is JSON-RPC only: {line:?}: {e}"));
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        Bridge {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The response to request `id`, stepping `h`'s frames while waiting.
    fn response(&mut self, id: i64, mut h: Option<&mut Harness>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "no response to request {id}");
            if let Some(h) = h.as_deref_mut() {
                h.step(vec![]);
            }
            match self.lines.recv_timeout(Duration::from_millis(5)) {
                Ok(v) if v["id"] == id => return v,
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(e) => panic!("bridge exited: {e}"),
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialize(id: i64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": { "name": "claude-code", "version": "1.0" } } })
}

fn call(id: i64, tool: &str, args: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": args } })
}

fn tool_body(resp: &Value) -> Value {
    serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn with_no_app_it_answers_initialize_and_fails_calls() {
    let mut b = Bridge::start(&["--socket", "/nonexistent/tessel.sock"]);
    b.send(initialize(1));
    let init = b.response(1, None);
    assert_eq!(init["result"]["serverInfo"]["name"], "tessel");
    b.send(call(2, "get_document", json!({})));
    let r = b.response(2, None);
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(tool_body(&r)["code"], "not_running");
}

#[test]
fn it_bridges_to_a_window_that_starts_later() {
    let root = std::env::temp_dir().join(format!("god-bridge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("tessel");
    let sock = dir.join(format!("{}.sock", std::process::id()));
    let mut b = Bridge::start(&["--socket", sock.to_str().unwrap()]);
    b.send(initialize(1));
    b.response(1, None);
    b.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    // Now the window opens.
    let mut h = Harness::new();
    let ctx = h.ctx.clone();
    h.app.start_agent(&ctx, Some(&dir));
    b.send(call(
        2,
        "create_node",
        json!({ "parent_id": h.ed().page().id.to_string(),
            "node": { "name": "Hello", "kind": "rect", "width": 10, "height": 10 } }),
    ));
    let r = b.response(2, Some(&mut h));
    assert_ne!(r["result"]["isError"], true, "{r}");
    assert_eq!(h.ed().page().children.len(), 1);
    assert_eq!(
        h.app.agent_indicator().as_deref(),
        Some("1 agent: claude-code"),
        "the replayed initialize names the client"
    );
    assert_eq!(
        h.ed().history.transactions()[0].origin,
        doc::Origin::Agent {
            client: "claude-code".into()
        }
    );

    // The window closes: calls fail with not_running again.
    h.app.stop_agent();
    b.send(call(3, "get_document", json!({})));
    let r = b.response(3, None);
    assert_eq!(tool_body(&r)["code"], "not_running", "{r}");
    let _ = std::fs::remove_dir_all(root);
}
