//! The agent link (§6): an MCP server that exposes the open document to
//! coding agents as tools, a prompt and resources.
//!
//! The app implements [`AppHandle`]; [`Agent`] serves one MCP session per
//! connection over any byte stream (the app's unix socket, or a test pipe).
//! Each mutating tool call becomes one transaction, labelled "Agent: …", so
//! one agent action is one undo step (§5.3).

pub mod args;
mod error;
mod handle;
mod json;
pub mod prompt;
mod server;
pub mod tools;

pub use error::{ErrorCode, ToolError};
pub use handle::{
    check_export_path, check_locked, locked_at, write_export_file, AppHandle, ExportFormat,
    ExportRequest, Screenshot, ScreenshotRequest, SelectionState, Snapshot, TxOk, TxRequest,
};
pub use server::{Agent, Notifier, ServeError, SessionInfo, DEBOUNCE, DOCUMENT_URI, SELECTION_URI};
