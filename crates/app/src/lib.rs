//! The tessel desktop app (§9): an eframe shell around the
//! document editor, the vello canvas and the layers panel.

pub mod activity;
pub mod agent_host;
pub mod agent_socket;
pub mod app;
pub mod auto_layout;
pub mod autosave;
pub mod canvas;
pub mod clipboard;
pub mod components;
pub mod components_panel;
pub mod editor;
pub mod export_window;
pub mod guides;
pub mod inspector;
pub mod instance_inspector;
pub mod layers;
pub mod layout_handles;
pub mod master_edit;
pub mod mcp_bridge;
pub mod pages;
pub mod panels;
pub mod pickers;
pub mod place;
pub mod snap;
pub mod structure;
pub mod styles_panel;
pub mod svg_import;
pub mod tokens_panel;
pub mod watch;

pub use app::DesignApp;
pub use editor::Editor;
