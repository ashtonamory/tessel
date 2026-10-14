//! Test support: `FakeApp`, an in-memory `AppHandle` over a `Document` and
//! `History`, and `Client`, a raw newline-delimited JSON-RPC MCP client.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent::{
    check_locked, write_export_file, Agent, AppHandle, ExportFormat, ExportRequest, Notifier,
    Screenshot, ScreenshotRequest, SelectionState, Snapshot, ToolError, TxOk, TxRequest,
};
use doc::{Command, Document, History, Id, NodeId, Origin, Selection};
use layout::LayoutEngine;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub const PAGE: &str = "p_page000001";
pub const PAGE2: &str = "p_page000002";
pub const SCREEN: &str = "n_screen0001";
pub const BG: &str = "n_bg00000001";
pub const TITLE: &str = "n_text000001";
pub const LOCKED: &str = "n_kkk0000001";
pub const LOCKED_CHILD: &str = "n_kkc0000001";
pub const GROUP: &str = "n_grp0000001";
pub const GROUP_CHILD: &str = "n_gc00000001";
pub const FILL_STYLE: &str = "s_f000000001";

pub fn fixture() -> Document {
    let json = json!({
        "format": { "name": "tessel", "version": 1 },
        "meta": { "title": "Fixture" },
        "tokens": { "colors": { "color.primary": "#2563eb" } },
        "styles": [
            { "id": FILL_STYLE, "name": "Surface/Card", "kind": "fill",
              "fills": [{ "type": "solid", "color": "#ffffff" }] }
        ],
        "pages": [
            { "id": PAGE, "name": "Home", "children": [
                { "id": SCREEN, "name": "Screen", "kind": "frame",
                  "x": 0, "y": 0, "width": 400, "height": 300,
                  "fills": [{ "type": "solid", "color": "#f0f0f0" }],
                  "children": [
                    { "id": BG, "name": "Bg", "kind": "rect",
                      "x": 10, "y": 20, "width": 100, "height": 50 },
                    { "id": TITLE, "name": "Title", "kind": "text",
                      "x": 10, "y": 100, "width": 200, "height": 24,
                      "content": "Hello world", "sizing": "fixed" },
                    { "id": GROUP, "name": "Group", "kind": "group",
                      "x": 0, "y": 0, "width": 10, "height": 10, "children": [
                        { "id": GROUP_CHILD, "name": "Dot", "kind": "ellipse",
                          "x": 0, "y": 0, "width": 10, "height": 10 }
                    ]}
                  ]},
                { "id": LOCKED, "name": "Locked", "kind": "frame", "locked": true,
                  "x": 500, "y": 0, "width": 100, "height": 100,
                  "children": [
                    { "id": LOCKED_CHILD, "name": "Inner", "kind": "rect",
                      "x": 0, "y": 0, "width": 10, "height": 10 }
                  ]}
            ]},
            { "id": PAGE2, "name": "Second" }
        ]
    });
    let loaded = doc::load_str(&json.to_string()).expect("fixture loads");
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    loaded.doc
}

pub fn id(s: &str) -> NodeId {
    Id::from_raw(s)
}

struct State {
    doc: Document,
    history: History,
    selection: Vec<NodeId>,
    revision: u64,
    dirty: bool,
}

thread_local! {
    // `LayoutEngine` is not `Send`; the app keeps it on the UI thread.
    static LAYOUT: std::cell::RefCell<LayoutEngine> = std::cell::RefCell::new(LayoutEngine::new());
}

fn layout(doc: &Document) -> layout::Geometry {
    LAYOUT.with(|l| l.borrow_mut().compute(doc))
}

/// The app, minus the window: edits go straight into a `History`.
pub struct FakeApp {
    state: Mutex<State>,
    published: Mutex<Arc<Snapshot>>,
    path: Option<PathBuf>,
    notifier: OnceLock<Notifier>,
}

impl FakeApp {
    pub fn new(doc: Document, path: Option<PathBuf>) -> FakeApp {
        let geometry = layout(&doc);
        let snap = Snapshot {
            document: doc.clone(),
            geometry,
            selection: SelectionState {
                page_id: Some(id(PAGE)),
                ids: Vec::new(),
            },
            revision: 0,
            dirty: false,
            path: path.clone(),
            assets: Default::default(),
        };
        FakeApp {
            state: Mutex::new(State {
                doc,
                history: History::default(),
                selection: Vec::new(),
                revision: 0,
                dirty: false,
            }),
            published: Mutex::new(Arc::new(snap)),
            path,
            notifier: OnceLock::new(),
        }
    }

    /// Wire up change notification, as the app does.
    pub fn set_notifier(&self, n: Notifier) {
        let _ = self.notifier.set(n);
    }

    fn publish(&self, s: &mut State) {
        let geometry = layout(&s.doc);
        *self.published.lock().unwrap() = Arc::new(Snapshot {
            document: s.doc.clone(),
            geometry,
            selection: SelectionState {
                page_id: Some(id(PAGE)),
                ids: s.selection.clone(),
            },
            revision: s.revision,
            dirty: s.dirty,
            path: self.path.clone(),
            assets: Default::default(),
        });
    }

    pub fn doc(&self) -> Document {
        self.state.lock().unwrap().doc.clone()
    }

    pub fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }

    pub fn selection(&self) -> Vec<NodeId> {
        self.state.lock().unwrap().selection.clone()
    }

    /// (label, origin) of every undoable transaction, oldest first.
    pub fn history(&self) -> Vec<(String, Origin)> {
        let s = self.state.lock().unwrap();
        s.history
            .transactions()
            .iter()
            .map(|t| (t.label.clone(), t.origin.clone()))
            .collect()
    }

    /// Ctrl+Z.
    pub fn undo(&self) {
        let mut s = self.state.lock().unwrap();
        let State { doc, history, .. } = &mut *s;
        history.undo(doc).unwrap().expect("something to undo");
        s.revision += 1;
        self.publish(&mut s);
    }

    /// An edit by the user (moves the revision on).
    pub fn user_edit(&self, commands: Vec<Command>) {
        let mut s = self.state.lock().unwrap();
        let State { doc, history, .. } = &mut *s;
        history
            .commit(
                doc,
                "User edit",
                Origin::User,
                commands,
                Selection::default(),
                Selection::default(),
            )
            .unwrap();
        s.revision += 1;
        self.publish(&mut s);
        if let Some(n) = self.notifier.get() {
            n.document_changed();
        }
    }

    /// The user clicks on something.
    pub fn user_select(&self, ids: &[&str]) {
        let mut s = self.state.lock().unwrap();
        s.selection = ids.iter().map(|i| id(i)).collect();
        self.publish(&mut s);
        if let Some(n) = self.notifier.get() {
            n.selection_changed();
        }
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbaImage::new(width.max(1), height.max(1));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

impl AppHandle for FakeApp {
    fn snapshot(&self) -> Arc<Snapshot> {
        self.published.lock().unwrap().clone()
    }

    async fn submit(&self, tx: TxRequest) -> Result<TxOk, ToolError> {
        let mut s = self.state.lock().unwrap();
        if let Some(expected) = tx.if_revision {
            if expected != s.revision {
                return Err(ToolError::stale_revision(expected, s.revision));
            }
        }
        check_locked(&s.doc, &tx.commands)?;
        let touched: BTreeSet<NodeId> = tx.commands.iter().flat_map(|c| c.touched_ids()).collect();
        let before = Selection {
            ids: s.selection.clone(),
        };
        let after = Selection {
            ids: tx.select.clone().unwrap_or_else(|| s.selection.clone()),
        };
        let State { doc, history, .. } = &mut *s;
        history.commit(
            doc,
            tx.label,
            Origin::Agent { client: tx.client },
            tx.commands,
            before,
            after.clone(),
        )?;
        s.selection = after.ids;
        s.revision += 1;
        s.dirty = true;
        self.publish(&mut s);
        if let Some(n) = self.notifier.get() {
            n.document_changed();
            if tx.select.is_some() {
                n.selection_changed();
            }
        }
        Ok(TxOk {
            revision: s.revision,
            touched,
        })
    }

    async fn set_selection(
        &self,
        ids: Vec<NodeId>,
        _zoom_to: bool,
    ) -> Result<Vec<NodeId>, ToolError> {
        let mut s = self.state.lock().unwrap();
        s.selection = ids.clone();
        self.publish(&mut s);
        if let Some(n) = self.notifier.get() {
            n.selection_changed();
        }
        Ok(ids)
    }

    async fn screenshot(&self, req: ScreenshotRequest) -> Result<Screenshot, ToolError> {
        let snap = self.snapshot();
        let g = snap
            .geometry
            .get(&req.node)
            .ok_or_else(|| ToolError::not_found(&req.node))?;
        let longest = g.size.width.max(g.size.height) as f32;
        let scale = req.scale.min(req.max_size as f32 / longest.max(1.0));
        let width = (g.size.width as f32 * scale).round() as u32;
        let height = (g.size.height as f32 * scale).round() as u32;
        Ok(Screenshot {
            png: png(width, height),
            width,
            height,
            scale,
        })
    }

    async fn export_file(&self, req: ExportRequest) -> Result<u64, ToolError> {
        let bytes = match req.format {
            ExportFormat::Png => png(4, 4),
            ExportFormat::Svg => br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec(),
        };
        write_export_file(&req.path, &bytes).map_err(|e| ToolError::internal(e.to_string()))?;
        Ok(bytes.len() as u64)
    }
}

/// A fresh, empty temp directory for one test.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("god-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A minimal MCP client speaking newline-delimited JSON-RPC.
pub struct Client {
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
    next_id: i64,
    /// Notifications received while waiting for a response.
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
        let n = self.reader.read_line(&mut line).await.unwrap();
        (n > 0).then(|| serde_json::from_str(&line).expect("valid JSON line"))
    }

    /// Send a request and return its whole response (`result` or `error`).
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(10), self.read_message())
                .await
                .expect("response within 10 s")
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
        let is_error = raw["isError"].as_bool().unwrap_or(false);
        let text = raw["content"]
            .as_array()
            .unwrap()
            .iter()
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

    /// Call a tool that must succeed.
    pub async fn ok(&mut self, tool: &str, args: Value) -> Value {
        let r = self.call(tool, args).await;
        assert!(!r.is_error, "{tool} failed: {}", r.json);
        r.json
    }

    /// Call a tool that must fail with `code`; returns the error body.
    pub async fn err(&mut self, tool: &str, args: Value, code: &str) -> Value {
        let r = self.call(tool, args).await;
        assert!(r.is_error, "{tool} should fail with {code}: {}", r.json);
        assert_eq!(r.json["code"], code, "{tool}: {}", r.json);
        r.json
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

/// An agent over a fake app, with one initialized client on an in-memory
/// pipe.
pub async fn start(app: FakeApp, client: &str) -> (Agent<FakeApp>, Client) {
    let agent = Agent::new(app);
    agent.app().set_notifier(agent.notifier());
    let mut c = connect(&agent);
    c.initialize(client).await;
    (agent, c)
}

/// A new, uninitialized session on an in-memory pipe.
pub fn connect(agent: &Agent<FakeApp>) -> Client {
    let (server, client) = tokio::io::duplex(1 << 20);
    let a = agent.clone();
    tokio::spawn(async move {
        let _ = a.serve(server).await;
    });
    Client::new(client)
}
