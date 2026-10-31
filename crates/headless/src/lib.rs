//! Tessel without a window (§6.7): `tessel render` turns a design file into
//! an image and a layer map, and `tessel mcp --headless` serves the agent
//! tools over the design files in one folder, so an agent can make and edit
//! designs on a computer where nobody has the app open.

pub mod cli;
pub mod gpu;
pub mod layers;
pub mod render;
pub mod workspace;

pub use gpu::{Gpu, GpuError};
pub use layers::{layer_map, Layer, LayerMap};
pub use render::{default_node, load, render, Format, RenderError, RenderOptions, Rendered};
pub use workspace::Workspace;
