//! Rendering for tessel (§7): document + `Geometry` to a vello
//! `Scene`, cached per page-level node, and rendering scenes on a shared
//! wgpu device, either into a canvas texture or offscreen to RGBA.

pub mod gpu;
pub mod image;
pub mod node;
pub mod paint;
pub mod scene;

pub use gpu::{CanvasTarget, RenderError, SceneRenderer, TARGET_FORMAT};
pub use image::{natural_size, ImageCache};
pub use node::{node_scene, render_node, NodeRender};
pub use paint::{parse_hex, resolve_color};
pub use scene::{page_scene, page_scene_with_images, top_level_of, SceneCache, SceneOptions};

pub use peniko;
pub use vello;
pub use wgpu;
