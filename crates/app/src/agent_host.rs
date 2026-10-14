//! The app's side of the agent link (§5.4, §6.1): an [`AppHandle`] whose
//! calls become messages to the UI thread, and the host that answers them.
//!
//! The MCP server runs on a tokio runtime on its own threads. A tool call
//! that edits, selects or renders sends a [`Request`] with a oneshot reply
//! and wakes the UI; the UI thread drains requests at the start of each
//! frame and applies each edit as one transaction. Reads are served from an
//! `Arc<Snapshot>` the UI thread publishes after each commit and selection
//! change, so they never wait for a frame.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use agent::{
    check_export_path, check_locked, write_export_file, Agent, AppHandle, ErrorCode, ExportFormat,
    ExportRequest, Notifier, Screenshot, ScreenshotRequest, SelectionState, SessionInfo, Snapshot,
    ToolError, TxOk, TxRequest,
};
use doc::{NodeId, ParentRef};
use tokio::sync::oneshot;

use crate::agent_socket::SocketFiles;
use crate::canvas::Canvas;
use crate::editor::{Editor, SceneDirty};

/// How long an agent edit waits for a user gesture on the same nodes to
/// end before it fails (§5.4).
pub const GESTURE_WAIT: Duration = Duration::from_secs(2);

type Reply<T> = oneshot::Sender<Result<T, ToolError>>;

/// A tool call the UI thread must answer.
pub enum Request {
    Submit(TxRequest, Reply<TxOk>),
    Select {
        ids: Vec<NodeId>,
        zoom_to: bool,
        reply: Reply<Vec<NodeId>>,
    },
    Screenshot(ScreenshotRequest, Reply<Screenshot>),
    Export(ExportRequest, Reply<u64>),
}

impl Request {
    /// Answer with an error without running the request.
    fn fail(self, e: ToolError) {
        match self {
            Request::Submit(_, r) => drop(r.send(Err(e))),
            Request::Select { reply, .. } => drop(reply.send(Err(e))),
            Request::Screenshot(_, r) => drop(r.send(Err(e))),
            Request::Export(_, r) => drop(r.send(Err(e))),
        }
    }
}

/// The [`AppHandle`] the MCP server holds: a channel to the UI thread.
#[derive(Clone)]
pub struct AppLink {
    requests: mpsc::Sender<Request>,
    snapshot: Arc<RwLock<Arc<Snapshot>>>,
    ctx: egui::Context,
}

fn app_closed() -> ToolError {
    ToolError::new(ErrorCode::NotRunning, "the Tessel window closed")
}

impl AppLink {
    async fn ask<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T, ToolError> {
        let (tx, rx) = oneshot::channel();
        self.requests.send(request(tx)).map_err(|_| app_closed())?;
        self.ctx.request_repaint();
        rx.await.map_err(|_| app_closed())?
    }
}

impl AppHandle for AppLink {
    fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.read().expect("snapshot lock").clone()
    }

    async fn submit(&self, tx: TxRequest) -> Result<TxOk, ToolError> {
        self.ask(|r| Request::Submit(tx, r)).await
    }

    async fn set_selection(
        &self,
        ids: Vec<NodeId>,
        zoom_to: bool,
    ) -> Result<Vec<NodeId>, ToolError> {
        self.ask(|reply| Request::Select {
            ids,
            zoom_to,
            reply,
        })
        .await
    }

    async fn screenshot(&self, req: ScreenshotRequest) -> Result<Screenshot, ToolError> {
        self.ask(|r| Request::Screenshot(req, r)).await
    }

    async fn export_file(&self, req: ExportRequest) -> Result<u64, ToolError> {
        self.ask(|r| Request::Export(req, r)).await
    }
}

/// A request waiting for a user gesture to end.
struct Pending {
    request: Request,
    deadline: Instant,
}

/// What the published snapshot was made from.
#[derive(PartialEq)]
struct Seen {
    revision: u64,
    dirty: bool,
    path: Option<PathBuf>,
    selection: SelectionState,
}

/// The UI thread's end of the agent link, owned by `DesignApp`.
pub struct AgentHost {
    agent: Agent<AppLink>,
    notifier: Notifier,
    runtime: Option<tokio::runtime::Runtime>,
    requests: mpsc::Receiver<Request>,
    snapshot: Arc<RwLock<Arc<Snapshot>>>,
    pending: VecDeque<Pending>,
    seen: Seen,
    /// The agent-visible revision: bumped once per published document
    /// change (§6.2).
    revision: u64,
    /// Publish on the next chance even if nothing looks changed (the
    /// document was replaced).
    force: bool,
    socket: Option<SocketFiles>,
    /// Why there is no socket, for the status bar.
    socket_error: Option<String>,
    focused: Option<bool>,
}

fn selection_state(ed: &Editor) -> SelectionState {
    SelectionState {
        page_id: Some(ed.document_page().id.clone()),
        ids: ed.selection.clone(),
    }
}

fn snapshot_of(ed: &Editor, revision: u64) -> Snapshot {
    Snapshot {
        document: ed.document().clone(),
        geometry: ed.document_geometry().clone(),
        selection: selection_state(ed),
        revision,
        dirty: ed.is_dirty(),
        path: ed.path.clone(),
        assets: ed.assets.clone(),
    }
}

impl AgentHost {
    /// Start the MCP server for `ed`'s window. With `socket_dir`, listen on
    /// `<socket_dir>/<pid>.sock`; a socket that cannot be bound leaves the
    /// host running without one (tests serve sessions directly).
    pub fn start(
        ctx: &egui::Context,
        ed: &Editor,
        socket_dir: Option<&Path>,
    ) -> std::io::Result<AgentHost> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("agent")
            .enable_all()
            .build()?;
        let (tx, requests) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(Arc::new(snapshot_of(ed, 0))));
        let link = AppLink {
            requests: tx,
            snapshot: snapshot.clone(),
            ctx: ctx.clone(),
        };
        let agent = {
            let _rt = runtime.enter();
            Agent::new(link)
        };
        let mut host = AgentHost {
            notifier: agent.notifier(),
            agent,
            runtime: None,
            requests,
            snapshot,
            pending: VecDeque::new(),
            seen: Seen {
                revision: ed.revision,
                dirty: ed.is_dirty(),
                path: ed.path.clone(),
                selection: selection_state(ed),
            },
            revision: 0,
            force: false,
            socket: None,
            socket_error: None,
            focused: None,
        };
        if let Some(dir) = socket_dir {
            match host.listen(&runtime, dir) {
                Ok(files) => {
                    tracing::info!("agents can connect at {}", files.path().display());
                    files.set_file(ed.path.as_deref());
                    if let Err(e) = files.make_current() {
                        tracing::warn!("current.sock: {e}");
                    }
                    host.socket = Some(files);
                }
                Err(e) => {
                    tracing::error!("cannot listen for agents in {}: {e}", dir.display());
                    host.socket_error = Some(format!("{}: {e}", dir.display()));
                }
            }
        }
        host.runtime = Some(runtime);
        Ok(host)
    }

    fn listen(
        &self,
        runtime: &tokio::runtime::Runtime,
        dir: &Path,
    ) -> std::io::Result<SocketFiles> {
        let (files, listener) = SocketFiles::bind(dir, std::process::id())?;
        listener.set_nonblocking(true)?;
        let listener = {
            let _rt = runtime.enter();
            tokio::net::UnixListener::from_std(listener)?
        };
        let agent = self.agent.clone();
        runtime.spawn(async move {
            if let Err(e) = agent.serve_unix(listener).await {
                tracing::error!("agent socket: {e}");
            }
        });
        Ok(files)
    }

    pub fn agent(&self) -> &Agent<AppLink> {
        &self.agent
    }

    /// The runtime the server runs on, to serve sessions from tests.
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        self.runtime.as_ref().expect("running").handle()
    }

    pub fn socket_path(&self) -> Option<PathBuf> {
        self.socket.as_ref().map(SocketFiles::path)
    }

    pub fn socket_error(&self) -> Option<&str> {
        self.socket_error.as_deref()
    }

    /// Connected agents, oldest first.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        self.agent.sessions()
    }

    pub fn disconnect(&self, id: u64) {
        self.agent.disconnect(id);
    }

    /// Whether agents may edit the document ("Agents may edit", §6.1).
    pub fn agents_may_edit(&self) -> bool {
        !self.agent.read_only()
    }

    pub fn set_agents_may_edit(&self, on: bool) {
        self.agent.set_read_only(!on);
    }

    /// The agent-visible revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Requests waiting for a gesture to end.
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// The window shows a different document now.
    pub fn document_replaced(&mut self) {
        self.force = true;
    }

    /// Answer agent requests; called at the start of each frame.
    pub fn process(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        ed: &mut Editor,
        canvas: &mut Canvas,
    ) {
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(true) && self.focused != Some(true) {
            if let Some(Err(e)) = self.socket.as_ref().map(SocketFiles::make_current) {
                tracing::warn!("current.sock: {e}");
            }
        }
        self.focused = focused;

        let now = ed.clock.now();
        let fresh = self.requests.try_iter().map(|request| Pending {
            request,
            deadline: now + GESTURE_WAIT,
        });
        let queue: Vec<Pending> = self.pending.drain(..).chain(fresh).collect();
        for p in queue {
            let busy = busy_nodes(ed, canvas);
            match blocker(&busy, &p.request) {
                Some(_) if now < p.deadline => self.pending.push_back(p),
                Some(id) => p.request.fail(
                    ToolError::new(
                        ErrorCode::Locked,
                        format!("the user is still editing {id}; try again in a moment"),
                    )
                    .with_node(id),
                ),
                None => self.answer(p.request, frame, ed, canvas),
            }
        }
        if !self.pending.is_empty() {
            // Check again soon even if the user holds still.
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn answer(
        &mut self,
        request: Request,
        frame: &eframe::Frame,
        ed: &mut Editor,
        canvas: &mut Canvas,
    ) {
        // Agents work on the document, so show it rather than a component.
        ed.leave_component();
        match request {
            Request::Submit(tx, reply) => {
                let _ = reply.send(self.submit(ed, tx));
            }
            Request::Select {
                ids,
                zoom_to,
                reply,
            } => {
                let ids: Vec<NodeId> = ids.into_iter().filter(|id| ed.node(id).is_some()).collect();
                ed.selection = ids.clone();
                show_selection_page(ed);
                if zoom_to && !ids.is_empty() {
                    canvas.zoom_to_fit(ed, true);
                }
                self.publish(ed);
                let _ = reply.send(Ok(ids));
            }
            Request::Screenshot(req, reply) => {
                let _ = reply.send(screenshot(frame, ed, canvas, req));
            }
            Request::Export(req, reply) => {
                let _ = reply.send(self.export(frame, ed, canvas, req));
            }
        }
    }

    fn submit(&mut self, ed: &mut Editor, tx: TxRequest) -> Result<TxOk, ToolError> {
        if ed.read_only {
            return Err(ToolError::new(
                ErrorCode::ReadOnly,
                "the document has problems, so it is open read-only",
            ));
        }
        if self.agent.read_only() {
            return Err(ToolError::read_only());
        }
        if let Some(expected) = tx.if_revision {
            if expected != self.revision {
                return Err(ToolError::stale_revision(expected, self.revision));
            }
        }
        check_locked(&ed.doc, &tx.commands)?;
        let touched: BTreeSet<NodeId> = tx.commands.iter().flat_map(|c| c.touched_ids()).collect();
        // A failed agent edit is the agent's to report, not the status bar's.
        let status = ed.status.clone();
        let select = tx.select.is_some();
        if let Err(e) = ed.commit_agent(&tx.label, &tx.client, tx.commands, tx.select) {
            ed.status = status;
            return Err(e.into());
        }
        if select {
            show_selection_page(ed);
        }
        self.publish(ed);
        Ok(TxOk {
            revision: self.revision,
            touched,
        })
    }

    fn export(
        &self,
        frame: &eframe::Frame,
        ed: &Editor,
        canvas: &mut Canvas,
        req: ExportRequest,
    ) -> Result<u64, ToolError> {
        // Checked again here: the document may have moved since the agent
        // checked, and the app is the one writing.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = check_export_path(&req.path, req.format, ed.path.as_deref(), home.as_deref())?;
        if ed.node(&req.node).is_none() {
            return Err(ToolError::not_found(&req.node));
        }
        let bytes = match req.format {
            ExportFormat::Png => {
                let opts = export::PngOptions {
                    scale: req.scale as f64,
                    background: None,
                };
                canvas
                    .render_png(frame, ed, &req.node, &opts)
                    .map_err(ToolError::internal)?
            }
            ExportFormat::Svg => {
                let name = ed.display_name();
                let opts = export::SvgOptions {
                    file: &name,
                    revision: self.revision,
                    assets: Some(&ed.assets),
                };
                export::export_svg(&ed.doc, &ed.geometry, &req.node, &opts)
                    .map_err(|e| ToolError::internal(e.to_string()))?
                    .into_bytes()
            }
        };
        write_export_file(&path, &bytes)
            .map_err(|e| ToolError::internal(format!("{}: {e}", path.display())))?;
        Ok(bytes.len() as u64)
    }

    /// Publish a new snapshot if the document, its saved state or the
    /// selection changed, and tell subscribed agents. Called after each
    /// agent request and at the end of each frame; a user gesture in
    /// progress is published when it ends.
    pub fn end_frame(&mut self, ed: &Editor) {
        if !ed.in_gesture() || self.force {
            self.publish(ed);
        }
    }

    fn publish(&mut self, ed: &Editor) {
        let seen = Seen {
            revision: ed.revision,
            dirty: ed.is_dirty(),
            path: ed.path.clone(),
            selection: selection_state(ed),
        };
        let document = self.force || seen.revision != self.seen.revision;
        if !document && seen == self.seen {
            return;
        }
        let selection = seen.selection != self.seen.selection;
        if seen.path != self.seen.path {
            if let Some(files) = &self.socket {
                files.set_file(seen.path.as_deref());
            }
        }
        if document {
            self.revision += 1;
        }
        self.force = false;
        self.seen = seen;
        *self.snapshot.write().expect("snapshot lock") = Arc::new(snapshot_of(ed, self.revision));
        if document {
            self.notifier.document_changed();
        }
        if selection {
            self.notifier.selection_changed();
        }
    }
}

impl Drop for AgentHost {
    fn drop(&mut self) {
        // Waiting agents hear that the window closed.
        for p in self.pending.drain(..) {
            p.request.fail(app_closed());
        }
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// The nodes a user gesture or inline text edit is working on, and their
/// ancestors (deleting or moving a parent would pull the node out from
/// under the user).
fn busy_nodes(ed: &Editor, canvas: &Canvas) -> BTreeSet<NodeId> {
    let mut busy = ed.gesture_nodes();
    busy.extend(canvas.editing_node().cloned());
    let mut out = busy.clone();
    for id in busy {
        let mut cur = id;
        while let Some((ParentRef::Node(p), _)) = ed.doc.parent_of(&cur) {
            out.insert(p.clone());
            cur = p;
        }
    }
    out
}

/// The busy node a request would touch, if any. Selection changes wait for
/// any gesture or text edit: changing the selection mid-drag would change
/// what is dragged.
fn blocker(busy: &BTreeSet<NodeId>, request: &Request) -> Option<NodeId> {
    match request {
        Request::Submit(tx, _) => tx
            .commands
            .iter()
            .flat_map(|c| c.touched_ids())
            .find(|id| busy.contains(id)),
        Request::Select { .. } => busy.first().cloned(),
        Request::Screenshot(..) | Request::Export(..) => None,
    }
}

/// Show the page of the first selected node, if it is not the current one.
fn show_selection_page(ed: &mut Editor) {
    let Some(first) = ed.selection.first() else {
        return;
    };
    let Some(page) = page_of(&ed.doc, first) else {
        return;
    };
    if ed.page().id != page {
        // The selection is one page's; drop nodes on other pages.
        let doc = &ed.doc;
        ed.selection
            .retain(|id| page_of(doc, id).is_some_and(|p| p == page));
        ed.current_page = Some(page);
        ed.scene_dirty = SceneDirty::All;
    }
}

fn page_of(doc: &doc::Document, id: &NodeId) -> Option<doc::PageId> {
    let mut cur = id.clone();
    loop {
        match doc.parent_of(&cur)? {
            (ParentRef::Node(p), _) => cur = p,
            (ParentRef::Page(p), _) => return Some(p),
        }
    }
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let w = png.get(16..20)?;
    let h = png.get(20..24)?;
    Some((
        u32::from_be_bytes(w.try_into().ok()?),
        u32::from_be_bytes(h.try_into().ok()?),
    ))
}

fn screenshot(
    frame: &eframe::Frame,
    ed: &Editor,
    canvas: &mut Canvas,
    req: ScreenshotRequest,
) -> Result<Screenshot, ToolError> {
    let g = ed
        .geometry
        .get(&req.node)
        .filter(|_| ed.node(&req.node).is_some())
        .ok_or_else(|| ToolError::not_found(&req.node))?;
    let longest = g.bounds.width().max(g.bounds.height()) as f32;
    let scale = req.scale.min(req.max_size as f32 / longest.max(1.0));
    let opts = export::PngOptions {
        scale: scale as f64,
        background: req.background.map(|c| c.0),
    };
    let png = canvas
        .render_png(frame, ed, &req.node, &opts)
        .map_err(ToolError::internal)?;
    let (width, height) =
        png_size(&png).ok_or_else(|| ToolError::internal("the renderer made a bad PNG"))?;
    Ok(Screenshot {
        png,
        width,
        height,
        scale,
    })
}
