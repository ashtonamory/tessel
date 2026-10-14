//! Tool errors (§6.2): returned to the agent as `isError` results whose text
//! is `{ "code": "...", "message": "...", "node_id"?: "..." }`.

use std::fmt;

use doc::CommandError;
use serde::Serialize;

/// The error codes of §6.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    InvalidArgs,
    InvalidPatch,
    Locked,
    StaleRevision,
    Invariant,
    ReadOnly,
    NotRunning,
    Internal,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = serde_json::to_value(self).expect("unit variant");
        f.write_str(v.as_str().expect("serializes as a string"))
    }
}

/// A failed tool call, as the agent sees it.
#[derive(Clone, Debug, PartialEq, Serialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ToolError {
        ToolError {
            code,
            message: message.into(),
            node_id: None,
        }
    }

    pub fn with_node(mut self, id: impl ToString) -> ToolError {
        self.node_id = Some(id.to_string());
        self
    }

    pub fn not_found(id: impl ToString) -> ToolError {
        let id = id.to_string();
        ToolError::new(ErrorCode::NotFound, format!("no node, page or style {id}")).with_node(id)
    }

    pub fn invalid_args(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::InvalidArgs, message)
    }

    pub fn locked(id: impl ToString) -> ToolError {
        let id = id.to_string();
        ToolError::new(
            ErrorCode::Locked,
            format!("{id} is locked by the user; agents may read it but not edit it"),
        )
        .with_node(id)
    }

    pub fn stale_revision(expected: u64, current: u64) -> ToolError {
        ToolError::new(
            ErrorCode::StaleRevision,
            format!("the document is at revision {current}, not {expected}; read it again"),
        )
    }

    pub fn read_only() -> ToolError {
        ToolError::new(
            ErrorCode::ReadOnly,
            "the user has turned agent edits off for this window",
        )
    }

    pub fn internal(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::Internal, message)
    }

    /// The JSON body sent to the agent.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("plain data")
    }
}

impl From<CommandError> for ToolError {
    fn from(e: CommandError) -> ToolError {
        let message = e.to_string();
        match e {
            CommandError::NotFound(id) => {
                ToolError::new(ErrorCode::NotFound, message).with_node(id)
            }
            CommandError::NotAContainer(id)
            | CommandError::DuplicateId(id)
            | CommandError::Cycle(id) => ToolError::invalid_args(message).with_node(id),
            CommandError::IndexOutOfRange { .. } => ToolError::invalid_args(message),
            CommandError::InvalidPatch(_) => ToolError::new(ErrorCode::InvalidPatch, message),
            CommandError::Invariant(problems) => {
                let e = ToolError::new(ErrorCode::Invariant, message);
                match problems.into_iter().find_map(|p| p.id) {
                    Some(id) => e.with_node(id),
                    None => e,
                }
            }
        }
    }
}
