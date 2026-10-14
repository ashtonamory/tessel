//! A minimal MCP client (newline-delimited JSON-RPC) for driving the app's
//! agent server from tests, and helpers to run it while frames step.

use std::future::Future;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use super::Harness;

pub struct Client {
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
    next_id: i64,
    pub notifications: Vec<Value>,
}

/// The result of a `tools/call`.
#[derive(Debug)]
pub struct ToolResult {
    pub is_error: bool,
    /// The first text content, parsed as JSON.
    pub json: Value,
    pub raw: Value,
}

impl Client {
    pub fn new<T: AsyncRead + AsyncWrite + Send + 'static>(io: T) -> Client {
        let (r, w) = tokio::io::split(io);
        Client {
            reader: BufReader::new(Box::new(r)),
            writer: Box::new(w),
            next_id: 1,
            notifications: Vec::new(),
        }
    }

    pub async fn send(&mut self, msg: Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn read_message(&mut self) -> Option<Value> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await.ok()?;
        (n > 0).then(|| serde_json::from_str(&line).expect("valid JSON line"))
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(20), self.read_message())
                .await
                .expect("response within 20 s")
                .expect("connection open");
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
            self.notifications.push(msg);
        }
    }

    pub async fn result(&mut self, method: &str, params: Value) -> Value {
        let resp = self.request(method, params).await;
        resp.get("result")
            .unwrap_or_else(|| panic!("{method} failed: {resp}"))
            .clone()
    }

    pub async fn initialize(&mut self, client: &str) -> Value {
        let result = self
            .result(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": client, "version": "1.0" }
                }),
            )
            .await;
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        result
    }

    pub async fn call(&mut self, tool: &str, args: Value) -> ToolResult {
        let raw = self
            .result("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        parse_tool_result(raw)
    }

    /// Wait up to `within` for the next notification.
    pub async fn next_notification(&mut self, within: Duration) -> Option<Value> {
        if !self.notifications.is_empty() {
            return Some(self.notifications.remove(0));
        }
        tokio::time::timeout(within, self.read_message())
            .await
            .ok()
            .flatten()
    }
}

pub fn parse_tool_result(raw: Value) -> ToolResult {
    let is_error = raw["isError"].as_bool().unwrap_or(false);
    let text = raw["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["type"] == "text")
        .and_then(|c| c["text"].as_str())
        .unwrap_or("null");
    let json = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
    ToolResult {
        is_error,
        json,
        raw,
    }
}

/// Start the app's agent server (no socket) and connect one initialized
/// client to it over an in-memory pipe.
pub fn connect(h: &mut Harness, client: &str) -> Client {
    if h.app.agent_host().is_none() {
        let ctx = h.ctx.clone();
        h.app.start_agent(&ctx, None);
    }
    let host = h.app.agent_host().unwrap();
    let agent = host.agent().clone();
    let (server, io) = tokio::io::duplex(1 << 20);
    host.runtime().spawn(async move {
        let _ = agent.serve(server).await;
    });
    let name = client.to_string();
    let (c, _) = run(h, async move {
        let mut c = Client::new(io);
        c.initialize(&name).await;
        (c, ())
    });
    c
}

/// A task on the agent runtime, run while the test steps frames.
pub struct Task<T>(tokio::task::JoinHandle<T>, tokio::runtime::Handle);

impl<T> Task<T> {
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    /// Step frames until the task is done, and return its output.
    pub fn finish(self, h: &mut Harness) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.0.is_finished() {
            assert!(Instant::now() < deadline, "the agent task did not finish");
            h.step(vec![]);
            std::thread::sleep(Duration::from_millis(1));
        }
        self.1.block_on(self.0).expect("task ran")
    }
}

pub fn spawn<T: Send + 'static>(
    h: &Harness,
    fut: impl Future<Output = T> + Send + 'static,
) -> Task<T> {
    let rt = h.app.agent_host().expect("agent started").runtime().clone();
    Task(rt.spawn(fut), rt)
}

/// Run `fut` to completion while stepping frames.
pub fn run<T: Send + 'static>(h: &mut Harness, fut: impl Future<Output = T> + Send + 'static) -> T {
    spawn(h, fut).finish(h)
}

/// One tool call, with the client handed back.
pub fn call(h: &mut Harness, c: Client, tool: &str, args: Value) -> (Client, ToolResult) {
    let tool = tool.to_string();
    run(h, async move {
        let mut c = c;
        let r = c.call(&tool, args).await;
        (c, r)
    })
}

/// A connected client that tests use synchronously: each call runs while
/// frames step.
pub struct Session {
    client: Option<Client>,
}

/// A call in flight (see [`Session::start`]).
pub struct InFlight(Task<(Client, ToolResult)>);

impl InFlight {
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl Session {
    pub fn connect(h: &mut Harness, client: &str) -> Session {
        Session {
            client: Some(connect(h, client)),
        }
    }

    /// Send a call without waiting for its answer.
    pub fn start(&mut self, h: &Harness, tool: &str, args: Value) -> InFlight {
        let mut c = self.client.take().expect("no call in flight");
        let tool = tool.to_string();
        InFlight(spawn(h, async move {
            let r = c.call(&tool, args).await;
            (c, r)
        }))
    }

    /// Step frames until a started call is answered.
    pub fn finish(&mut self, h: &mut Harness, call: InFlight) -> ToolResult {
        let (c, r) = call.0.finish(h);
        self.client = Some(c);
        r
    }

    pub fn call(&mut self, h: &mut Harness, tool: &str, args: Value) -> ToolResult {
        let call = self.start(h, tool, args);
        self.finish(h, call)
    }

    /// A call that must succeed.
    pub fn ok(&mut self, h: &mut Harness, tool: &str, args: Value) -> Value {
        let r = self.call(h, tool, args);
        assert!(!r.is_error, "{tool} failed: {}", r.json);
        r.json
    }

    /// A call that must fail with `code`.
    pub fn err(&mut self, h: &mut Harness, tool: &str, args: Value, code: &str) -> Value {
        let r = self.call(h, tool, args);
        assert!(r.is_error, "{tool} should fail with {code}: {}", r.json);
        assert_eq!(r.json["code"], code, "{tool}: {}", r.json);
        r.json
    }

    /// Wait for a notification while frames step.
    pub fn notification(&mut self, h: &mut Harness, within: Duration) -> Option<Value> {
        let mut c = self.client.take().expect("no call in flight");
        let (c, n) = run(h, async move {
            let n = c.next_notification(within).await;
            (c, n)
        });
        self.client = Some(c);
        n
    }

    pub fn subscribe(&mut self, h: &mut Harness, uri: &str) {
        let mut c = self.client.take().expect("no call in flight");
        let uri = uri.to_string();
        let (c, _) = run(h, async move {
            c.result("resources/subscribe", json!({ "uri": uri })).await;
            (c, ())
        });
        self.client = Some(c);
    }
}
