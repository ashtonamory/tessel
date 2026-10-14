//! PNG export: render the node offscreen (`render::render_node`) and encode.

use doc::{Document, NodeId};
use layout::Geometry;
use render::peniko::Color;
use render::{parse_hex, wgpu, ImageCache, SceneRenderer};

use crate::ExportError;

#[derive(Clone, Debug, PartialEq)]
pub struct PngOptions {
    /// Pixels per document px: 1, 2, 3 or any value above 0.
    pub scale: f64,
    /// `#rrggbb[aa]`; `None` is transparent.
    pub background: Option<String>,
}

impl Default for PngOptions {
    fn default() -> Self {
        PngOptions {
            scale: 1.0,
            background: None,
        }
    }
}

/// The app's GPU objects the export renders with.
pub struct GpuContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub renderer: &'a mut SceneRenderer,
    pub images: &'a ImageCache,
}

/// The node as PNG bytes. No selection chrome is drawn: only the document's
/// own scene.
pub fn export_png(
    doc: &Document,
    geom: &Geometry,
    id: &NodeId,
    opts: &PngOptions,
    gpu: &mut GpuContext<'_>,
) -> Result<Vec<u8>, ExportError> {
    let background = match &opts.background {
        Some(s) => parse_hex(s).ok_or_else(|| ExportError::BadBackground(s.clone()))?,
        None => Color::TRANSPARENT,
    };
    let img = render::render_node(
        gpu.renderer,
        gpu.device,
        gpu.queue,
        doc,
        geom,
        id,
        gpu.images,
        opts.scale,
        background,
    )?
    .ok_or_else(|| ExportError::NodeNotFound(id.to_string()))?;
    let mut out = Vec::new();
    let mut enc = ::png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(::png::ColorType::Rgba);
    enc.set_depth(::png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img.rgba)?;
    Ok(out)
}
