//! Offscreen rendering of one node (§7.6): screenshots and PNG export.

use doc::{Document, NodeId};
use kurbo::{Affine, Rect};
use layout::Geometry;
use peniko::Color;
use vello::Scene;

use crate::gpu::{RenderError, SceneRenderer};
use crate::image::ImageCache;
use crate::scene::{encode_node, SceneOptions};

/// Largest side rendered in one pass; bigger renders are tiled.
const TILE: u32 = 4096;

/// A rendered node: tightly packed RGBA8 rows.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeRender {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The scene of `id` and its subtree alone, in page coordinates, plus the
/// node's page bounds. `id` may be a node inside an instance (a derived ID).
/// `None` if the node or its geometry is missing.
pub fn node_scene(
    doc: &Document,
    geom: &Geometry,
    id: &NodeId,
    images: &ImageCache,
) -> Option<(Scene, Rect)> {
    let bounds = geom.get(id)?.bounds;
    let node = match doc.node(id) {
        Some(n) => std::borrow::Cow::Borrowed(n),
        None => std::borrow::Cow::Owned(doc::shown_node(doc, id)?),
    };
    let mut scene = Scene::new();
    encode_node(
        &mut scene,
        doc,
        geom,
        &node,
        &SceneOptions::default(),
        images,
    );
    Some((scene, bounds))
}

/// Render one node at `scale` (any value above 0) on the caller's device:
/// the node's bounds times the scale, on `background` (use a transparent
/// color for none), without any selection chrome. Renders larger than 4096 px
/// are tiled.
#[allow(clippy::too_many_arguments)]
pub fn render_node(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    doc: &Document,
    geom: &Geometry,
    id: &NodeId,
    images: &ImageCache,
    scale: f64,
    background: Color,
) -> Result<Option<NodeRender>, RenderError> {
    let Some((scene, bounds)) = node_scene(doc, geom, id, images) else {
        return Ok(None);
    };
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let width = ((bounds.width() * scale).ceil() as u32).max(1);
    let height = ((bounds.height() * scale).ceil() as u32).max(1);
    let view = Affine::scale(scale) * Affine::translate((-bounds.x0, -bounds.y0));
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for ty in (0..height).step_by(TILE as usize) {
        for tx in (0..width).step_by(TILE as usize) {
            let tw = TILE.min(width - tx);
            let th = TILE.min(height - ty);
            let mut tile = Scene::new();
            let tf = Affine::translate((-(tx as f64), -(ty as f64))) * view;
            tile.append(&scene, Some(tf));
            let px = renderer.render_to_rgba(device, queue, &tile, tw, th, background)?;
            for row in 0..th as usize {
                let src = row * tw as usize * 4;
                let dst = ((ty as usize + row) * width as usize + tx as usize) * 4;
                rgba[dst..dst + tw as usize * 4].copy_from_slice(&px[src..src + tw as usize * 4]);
            }
        }
    }
    Ok(Some(NodeRender {
        width,
        height,
        rgba,
    }))
}
