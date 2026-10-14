//! The MCP server (§6.1, §6.5, §6.6): one rmcp session per connection over
//! any byte stream, resources with subscriptions, and change notification.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, Implementation, InitializeRequestParams, InitializeResult,
    ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt,
    PromptMessage, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ResourceUpdatedNotificationParam, Role,
    ServerCapabilities, ServerConfig, SubscribeRequestParams, UnsubscribeRequestParams,
};
use rmcp::service::{Peer, RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::handle::AppHandle;
use crate::tools::{self, ToolOutput};
use crate::{prompt, ToolError};

pub const DOCUMENT_URI: &str = "design://document";
pub const SELECTION_URI: &str = "design://selection";

/// Resource-updated notifications wait until changes have been quiet this
/// long (§6.5)...
pub const DEBOUNCE: Duration = Duration::from_millis(250);
/// ...but no longer than this after the first change.
const DEBOUNCE_MAX: Duration = Duration::from_secs(1);

/// The name an agent gets until it sends `initialize`.
const UNKNOWN_CLIENT: &str = "agent";

const INSTRUCTIONS: &str = "Edits the design document open in the Tessel app. \
Load the design_conventions prompt first. Call get_selection when the user refers to \
\"this\" or \"the selected\" thing, and screenshot after edits to check your work.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Document,
    Selection,
}

impl Change {
    fn uri(self) -> &'static str {
        match self {
            Change::Document => DOCUMENT_URI,
            Change::Selection => SELECTION_URI,
        }
    }
}

/// A connected agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u64,
    /// The client name from `initialize` (`Origin::Agent { client }`).
    pub client: String,
}

struct SessionState {
    client: String,
    peer: Option<Peer<RoleServer>>,
    subscriptions: BTreeSet<String>,
    cancel: CancellationToken,
}

type Sessions = Arc<Mutex<BTreeMap<u64, SessionState>>>;

struct Shared<H> {
    app: H,
    read_only: AtomicBool,
    sessions: Sessions,
    next_id: AtomicU64,
    notifier: Notifier,
}

/// Tells subscribed agents about changes. Cheap to clone; callable from any
/// thread (the app calls it from the UI thread after each commit).
#[derive(Clone)]
pub struct Notifier(mpsc::UnboundedSender<Change>);

impl Notifier {
    /// A transaction committed.
    pub fn document_changed(&self) {
        let _ = self.0.send(Change::Document);
    }

    /// The selection or the shown page changed.
    pub fn selection_changed(&self) {
        let _ = self.0.send(Change::Selection);
    }
}

/// The MCP server for one app window. Clones share the same sessions.
pub struct Agent<H> {
    shared: Arc<Shared<H>>,
}

impl<H> Clone for Agent<H> {
    fn clone(&self) -> Self {
        Agent {
            shared: self.shared.clone(),
        }
    }
}

impl<H: AppHandle> Agent<H> {
    /// Must be called inside a tokio runtime: it spawns the task that sends
    /// debounced resource notifications.
    pub fn new(app: H) -> Agent<H> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sessions = Sessions::default();
        tokio::spawn(notify_loop(rx, sessions.clone()));
        Agent {
            shared: Arc::new(Shared {
                app,
                read_only: AtomicBool::new(false),
                sessions,
                next_id: AtomicU64::new(1),
                notifier: Notifier(tx),
            }),
        }
    }

    pub fn app(&self) -> &H {
        &self.shared.app
    }

    pub fn notifier(&self) -> Notifier {
        self.shared.notifier.clone()
    }

    /// Refuse every document edit with `read_only` (§6.1 "agents may edit"
    /// toggle). Reads, selection and screenshots still work.
    pub fn set_read_only(&self, read_only: bool) {
        self.shared.read_only.store(read_only, Ordering::Relaxed);
    }

    pub fn read_only(&self) -> bool {
        self.shared.read_only.load(Ordering::Relaxed)
    }

    /// Connected agents, oldest first.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.shared.sessions.lock().expect("sessions lock");
        sessions
            .iter()
            .map(|(id, s)| SessionInfo {
                id: *id,
                client: s.client.clone(),
            })
            .collect()
    }

    /// Close one agent's connection.
    pub fn disconnect(&self, id: u64) {
        if let Some(s) = self.shared.sessions.lock().expect("sessions lock").get(&id) {
            s.cancel.cancel();
        }
    }

    /// Serve one MCP session over `io` (newline-delimited JSON-RPC) until the
    /// client disconnects or [`Agent::disconnect`] closes it.
    pub async fn serve<T>(&self, io: T) -> Result<(), ServeError>
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        self.shared.sessions.lock().expect("sessions lock").insert(
            id,
            SessionState {
                client: UNKNOWN_CLIENT.into(),
                peer: None,
                subscriptions: BTreeSet::new(),
                cancel: cancel.clone(),
            },
        );
        let _remove = RemoveOnDrop(self.shared.sessions.clone(), id);
        let session = Session {
            shared: self.shared.clone(),
            id,
        };
        let running = session
            .serve_with_ct(io, cancel)
            .await
            .map_err(|e| ServeError(e.to_string()))?;
        running
            .waiting()
            .await
            .map_err(|e| ServeError(e.to_string()))?;
        Ok(())
    }

    /// Accept connections on a unix socket forever, one session each.
    pub async fn serve_unix(&self, listener: tokio::net::UnixListener) -> std::io::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            let agent = self.clone();
            tokio::spawn(async move {
                if let Err(e) = agent.serve(stream).await {
                    tracing::warn!("agent session ended with an error: {e}");
                }
            });
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("MCP session failed: {0}")]
pub struct ServeError(String);

struct RemoveOnDrop(Sessions, u64);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.lock() {
            s.remove(&self.1);
        }
    }
}

async fn notify_loop(mut rx: mpsc::UnboundedReceiver<Change>, sessions: Sessions) {
    while let Some(first) = rx.recv().await {
        let started = Instant::now();
        let mut pending = BTreeSet::from([first.uri()]);
        let mut open = true;
        loop {
            let deadline = (Instant::now() + DEBOUNCE).min(started + DEBOUNCE_MAX);
            tokio::select! {
                c = rx.recv() => match c {
                    Some(c) => { pending.insert(c.uri()); }
                    None => { open = false; break; }
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        let targets: Vec<(Peer<RoleServer>, &'static str)> = {
            let sessions = sessions.lock().expect("sessions lock");
            sessions
                .values()
                .filter_map(|s| Some((s.peer.clone()?, &s.subscriptions)))
                .flat_map(|(peer, subs)| {
                    pending
                        .iter()
                        .filter(|uri| subs.contains(**uri))
                        .map(move |uri| (peer.clone(), *uri))
                })
                .collect()
        };
        for (peer, uri) in targets {
            let _ = peer
                .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                .await;
        }
        if !open {
            break;
        }
    }
}

/// One connection's handler.
struct Session<H> {
    shared: Arc<Shared<H>>,
    id: u64,
}

impl<H: AppHandle> Session<H> {
    fn with_state<R>(&self, f: impl FnOnce(&mut SessionState) -> R) -> Option<R> {
        let mut sessions = self.shared.sessions.lock().expect("sessions lock");
        sessions.get_mut(&self.id).map(f)
    }

    fn client(&self) -> String {
        self.with_state(|s| s.client.clone())
            .unwrap_or_else(|| UNKNOWN_CLIENT.into())
    }

    fn resource_json(&self, uri: &str) -> Option<Value> {
        let snap = self.shared.app.snapshot();
        match uri {
            DOCUMENT_URI => Some(tools::document_json(&snap, None, 1).expect("no page filter")),
            SELECTION_URI => Some(tools::selection_json(&snap)),
            _ => None,
        }
    }
}

fn tool_result(out: Result<ToolOutput, ToolError>) -> CallToolResult {
    match out {
        Ok(ToolOutput::Json(v)) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
        Ok(ToolOutput::Image { png, info }) => CallToolResult::success(vec![
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(png),
                "image/png",
            ),
            ContentBlock::text(info.to_string()),
        ]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_json())]),
    }
}

fn not_a_resource(uri: &str) -> McpError {
    McpError::resource_not_found(format!("no resource {uri}"), None)
}

impl<H: AppHandle> ServerHandler for Session<H> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
        .with_server_info(Implementation::new("tessel", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    /// Sessions start with `initialize` and use `resources/subscribe`, so
    /// only the protocol revisions that have them.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(
            &ProtocolVersion::LATEST_WITH_INITIALIZE,
        ))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        let name = request.client_info.name.trim().to_string();
        let peer = context.peer.clone();
        self.with_state(|s| {
            if !name.is_empty() {
                s.client = name;
            }
            s.peer = Some(peer);
        });
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools::list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let client = self.client();
        let read_only = self.shared.read_only.load(Ordering::Relaxed);
        match tools::call(&self.shared.app, &client, read_only, &request.name, args).await {
            Some(out) => Ok(tool_result(out).into()),
            None => Err(McpError::invalid_params(
                format!("no tool called {:?}", request.name),
                None,
            )),
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            prompt::NAME,
            Some(prompt::DESCRIPTION),
            None,
        )]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        if request.name != prompt::NAME {
            return Err(McpError::invalid_params(
                format!("no prompt called {:?}", request.name),
                None,
            ));
        }
        let mut result =
            GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt::TEXT)]);
        result.description = Some(prompt::DESCRIPTION.into());
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(DOCUMENT_URI, "document")
                .with_description("The open document: pages, top-level layers, tokens, styles.")
                .with_mime_type("application/json"),
            Resource::new(SELECTION_URI, "selection")
                .with_description("What the user has selected right now.")
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let v = self
            .resource_json(&request.uri)
            .ok_or_else(|| not_a_resource(&request.uri))?;
        let contents = ResourceContents::text(v.to_string(), request.uri.clone())
            .with_mime_type("application/json");
        Ok(ReadResourceResult::new(vec![contents]).into())
    }

    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if request.uri != DOCUMENT_URI && request.uri != SELECTION_URI {
            return Err(not_a_resource(&request.uri));
        }
        self.with_state(|s| s.subscriptions.insert(request.uri));
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.with_state(|s| s.subscriptions.remove(&request.uri));
        Ok(())
    }
}
