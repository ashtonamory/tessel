//! The tessel document: data model, canonical file format,
//! validation, commands and undo history. Pure data; no GPU, UI or async.
//!
//! See `docs/SPEC.md` §3 (model), §4 (file format) and §5 (commands).

pub mod assets;
pub mod canonical;
pub mod command;
pub mod components;
pub mod expand;
pub mod history;
pub mod id;
pub mod index;
pub mod io;
pub mod model;
mod node_serde;
pub mod resolve;
mod tokens_styles;
pub mod validate;

pub use assets::Assets;
pub use canonical::to_canonical_string;
pub use command::{Command, CommandError, NodePatch};
pub use expand::{expand_deep, expand_instance, resolve_derived, shown_node, Derived};
pub use history::{History, Origin, Selection, Transaction};
pub use id::{ComponentId, Id, IdError, IdKind, NodeId, PageId, StyleId};
pub use index::{NodePath, ParentRef};
pub use io::{load_file, load_str, migrate, save_file, save_string, LoadError, Loaded};
pub use model::*;
pub use tokens_styles::TokensPatch;
pub use validate::{validate, Problem};
