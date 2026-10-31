//! Render one node of a design file to PNG, JPEG or SVG, with its layer map.

use std::path::Path;

use doc::assets::assets_dir_for;
use doc::{Assets, Document, NodeId};
use layout::{Geometry, LayoutEngine};
use render::peniko::Color;
use render::{parse_hex, ImageCache};

use crate::gpu::{Gpu, GpuError};
use crate::layers::{layer_map, LayerMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Svg,
}

impl Format {
    pub fn from_extension(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "svg" => Some(Format::Svg),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Svg => "svg",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    /// The node to render; default [`default_node`].
    pub node: Option<String>,
    pub format: Format,
    /// Image pixels per document px.
    pub scale: f64,
    /// `#rrggbb[aa]` behind the design; `None` is transparent, or white for
    /// JPEG, which has no transparency.
    pub background: Option<String>,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            node: None,
            format: Format::Png,
            scale: 1.0,
            background: None,
            quality: 90,
        }
    }
}

/// An encoded image and the layer map that goes with it.
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub node: NodeId,
    pub layers: LayerMap,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("{0}")]
    Load(#[from] doc::LoadError),
    #[error("no node {0} in the document")]
    NoNode(String),
    #[error("the document has nothing to render")]
    Empty,
    #[error("scale must be above 0 and at most 16")]
    BadScale,
    #[error("background {0:?} is not a #rrggbb[aa] color")]
    BadBackground(String),
    #[error(transparent)]
    Gpu(#[from] GpuError),
    #[error("render failed: {0}")]
    Render(String),
}

/// A design file and its images.
pub fn load(path: &Path) -> Result<(Document, Assets), RenderError> {
    let loaded = doc::load_file(path)?;
    for p in &loaded.problems {
        log::warn!("{}: {p:?}", path.display());
    }
    let assets = Assets::new();
    let dir = assets_dir_for(path);
    assets.set_dir(Some(dir.clone()));
    assets.load_dir(&dir, &loaded.doc);
    Ok((loaded.doc, assets))
}

/// What a file renders by default: the first visible layer of the first page
/// that has one, which in a design file is its design frame.
pub fn default_node(doc: &Document) -> Option<NodeId> {
    doc.pages
        .iter()
        .flat_map(|p| p.children.iter())
        .find(|n| n.visible)
        .map(|n| n.id.clone())
}

/// Render `opts.node` of `doc`. PNG and JPEG need `gpu`; SVG does not.
pub fn render(
    doc: &Document,
    assets: &Assets,
    opts: &RenderOptions,
    gpu: Option<&mut Gpu>,
) -> Result<Rendered, RenderError> {
    if !(opts.scale > 0.0 && opts.scale <= 16.0) {
        return Err(RenderError::BadScale);
    }
    let node = match &opts.node {
        Some(id) => doc::Id::from_raw(id),
        None => default_node(doc).ok_or(RenderError::Empty)?,
    };
    let geom = LayoutEngine::new().compute(doc);
    let Some(bounds) = geom.get(&node).map(|g| g.bounds) else {
        return Err(RenderError::NoNode(node.to_string()));
    };
    // SVG is sized in document px times the scale; a render reports its own size.
    let size = |v: f64| (v * opts.scale).ceil().max(1.0) as u32;
    let (mut width, mut height) = (size(bounds.width()), size(bounds.height()));
    let bytes = match opts.format {
        Format::Svg => {
            let svg = export::SvgOptions {
                file: if doc.meta.title.is_empty() {
                    "design"
                } else {
                    &doc.meta.title
                },
                revision: 0,
                assets: Some(assets),
            };
            export::export_svg(doc, &geom, &node, &svg)
                .map_err(|e| RenderError::Render(e.to_string()))?
                .into_bytes()
        }
        Format::Png | Format::Jpeg => {
            let gpu = match gpu {
                Some(g) => g,
                None => return Err(RenderError::Render("PNG and JPEG need a GPU".into())),
            };
            let background = match (&opts.background, opts.format) {
                (Some(s), _) => color(s)?,
                (None, Format::Jpeg) => Color::WHITE,
                (None, _) => Color::TRANSPARENT,
            };
            let img = rgba(doc, &geom, &node, assets, opts.scale, background, gpu)?;
            (width, height) = (img.width, img.height);
            match opts.format {
                Format::Png => encode_png(&img)?,
                _ => encode_jpeg(&img, opts.quality)?,
            }
        }
    };
    let layers = layer_map(doc, &geom, &node, opts.scale, width, height);
    Ok(Rendered {
        bytes,
        node,
        layers,
    })
}

fn color(s: &str) -> Result<Color, RenderError> {
    parse_hex(s).ok_or_else(|| RenderError::BadBackground(s.to_string()))
}

fn rgba(
    doc: &Document,
    geom: &Geometry,
    node: &NodeId,
    assets: &Assets,
    scale: f64,
    background: Color,
    gpu: &mut Gpu,
) -> Result<render::NodeRender, RenderError> {
    let images = ImageCache::new(assets.clone());
    render::render_node(
        &mut gpu.renderer,
        &gpu.device,
        &gpu.queue,
        doc,
        geom,
        node,
        &images,
        scale,
        background,
    )
    .map_err(|e| RenderError::Render(e.to_string()))?
    .ok_or_else(|| RenderError::NoNode(node.to_string()))
}

fn encode_png(img: &render::NodeRender) -> Result<Vec<u8>, RenderError> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
        .ok_or_else(|| RenderError::Render("the renderer returned a short image".into()))?
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| RenderError::Render(e.to_string()))?;
    Ok(out.into_inner())
}

/// JPEG has no alpha: what is left transparent comes out over white.
fn encode_jpeg(img: &render::NodeRender, quality: u8) -> Result<Vec<u8>, RenderError> {
    let rgb: Vec<u8> = img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            let over = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            [over(p[0]), over(p[1]), over(p[2])]
        })
        .collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
        .encode(&rgb, img.width, img.height, image::ExtendedColorType::Rgb8)
        .map_err(|e| RenderError::Render(e.to_string()))?;
    Ok(out)
}
