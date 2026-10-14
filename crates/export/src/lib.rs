//! Exports for tessel (§8): pure functions over a document, its
//! computed geometry and a node. PNG renders the node offscreen on the
//! caller's GPU device (§7.6); SVG is a deterministic hand-written writer;
//! code (HTML + CSS, Tailwind, React) and tokens are written from one
//! element tree.

pub mod code;
pub mod png;
pub mod svg;

pub use code::{
    export_code, export_tokens, instance_children, CodeFormat, CodeOptions, FileContent, ImageMode,
    OutFile, TailwindVersion, TokenFormat, TokenMode,
};
pub use png::{export_png, GpuContext, PngOptions};
pub use svg::{export_svg, SvgOptions};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("node {0} is not in the document or has no geometry")]
    NodeNotFound(String),
    #[error("invalid background color {0:?}")]
    BadBackground(String),
    #[error("render: {0}")]
    Render(#[from] render::RenderError),
    #[error("encoding the PNG failed: {0}")]
    Png(#[from] ::png::EncodingError),
}
