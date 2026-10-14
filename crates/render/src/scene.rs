//! Document + geometry to vello scenes (§7.4). Scenes are cached per
//! page-level node in page coordinates; a canvas redraw appends the visible
//! cached scenes under the view transform, so pan and zoom never re-encode.

use std::collections::{BTreeSet, HashMap};

use doc::expand::{components_used, owning_instance};
use doc::resolve::{
    effective_corner_radius, effective_fills, effective_shadows, effective_stroke_width,
    effective_strokes,
};
use doc::{
    Component, ComponentId, CornerRadius, Document, Node, NodeId, NodeKind, Page, Paint, RadiusRef,
    StrokeAlign,
};
use kurbo::{Affine, BezPath, Ellipse, Point, Rect, RoundedRect, RoundedRectRadii, Stroke};
use layout::{Geometry, NodeGeometry, TextGeometry};
use parley::PositionedLayoutItem;
use peniko::{Brush, Color, Fill, Mix};
use vello::Scene;

use crate::image::{draw_image, ImageCache};
use crate::paint::{brush, resolve_color};

/// Options that change what a scene contains.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneOptions {
    /// A node left out of the scene, e.g. a text node being edited inline
    /// (the editor draws it instead).
    pub hidden: Option<NodeId>,
}

struct Cached {
    scene: Scene,
    /// Page-coordinate bounds of everything the scene may paint.
    bounds: Rect,
    /// Components whose instances the scene shows, directly or nested.
    uses: BTreeSet<ComponentId>,
}

/// Cached scenes of one page's top-level nodes.
#[derive(Default)]
pub struct SceneCache {
    entries: HashMap<NodeId, Cached>,
    options: SceneOptions,
    images: ImageCache,
    /// Components as of the last build: a scene showing an instance of a
    /// component that changed since is re-encoded.
    components: Vec<Component>,
}

impl SceneCache {
    pub fn new() -> SceneCache {
        SceneCache::default()
    }

    /// A cache that draws images from `images`.
    pub fn with_images(images: ImageCache) -> SceneCache {
        SceneCache {
            images,
            ..SceneCache::default()
        }
    }

    pub fn images(&self) -> &ImageCache {
        &self.images
    }

    /// Forget the scenes containing any of `ids` (§5.5). Call it with the
    /// touched IDs of a change, looked up in the document after the change;
    /// IDs no longer in the document are ignored (their page-level node is
    /// dropped by the next `build`, or its parent is in `ids`). A derived ID
    /// counts as its instance. Changes to component masters need no IDs:
    /// `build` re-encodes every scene showing an instance of a changed
    /// component.
    pub fn invalidate<'a>(&mut self, doc: &Document, ids: impl IntoIterator<Item = &'a NodeId>) {
        for id in ids {
            if let Some(top) = top_level_of(doc, id) {
                self.entries.remove(&top);
            }
        }
    }

    /// Forget the scenes that show an instance (directly or nested) of a
    /// component added, removed or changed since the last call. `build`
    /// calls it.
    pub fn sync_components(&mut self, doc: &Document) {
        if self.components != doc.components {
            let changed = changed_components(&self.components, &doc.components);
            self.entries.retain(|_, c| c.uses.is_disjoint(&changed));
            self.components = doc.components.clone();
        }
    }

    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The page's scene under `view` (page → target pixels), skipping
    /// page-level nodes outside `viewport` (page coordinates).
    pub fn build(
        &mut self,
        doc: &Document,
        geom: &Geometry,
        page: &Page,
        view: Affine,
        viewport: Rect,
        options: &SceneOptions,
    ) -> Scene {
        let _span = tracing::trace_span!("scene").entered();
        if &self.options != options {
            self.entries.clear();
            self.options = options.clone();
        }
        self.entries
            .retain(|id, _| page.children.iter().any(|n| &n.id == id));
        self.sync_components(doc);
        let mut root = Scene::new();
        for node in &page.children {
            let cached = self.entries.entry(node.id.clone()).or_insert_with(|| {
                let mut scene = Scene::new();
                encode_node(&mut scene, doc, geom, node, options, &self.images);
                Cached {
                    scene,
                    bounds: paint_bounds(doc, geom, node),
                    uses: components_used(doc, node),
                }
            });
            if overlaps(cached.bounds, viewport) {
                root.append(&cached.scene, Some(view));
            }
        }
        root
    }
}

/// IDs of components added, removed or changed between `old` and `new`.
fn changed_components(old: &[Component], new: &[Component]) -> BTreeSet<ComponentId> {
    let mut out = BTreeSet::new();
    for c in old {
        if new.iter().find(|n| n.id == c.id) != Some(c) {
            out.insert(c.id.clone());
        }
    }
    for c in new {
        if !old.iter().any(|o| o.id == c.id) {
            out.insert(c.id.clone());
        }
    }
    out
}

/// The page-level ancestor of a node (or the node itself). A derived ID
/// (a node inside an instance) counts as its instance.
pub fn top_level_of(doc: &Document, id: &NodeId) -> Option<NodeId> {
    let owner = owning_instance(id);
    let path = doc.path_of(owner.as_ref().unwrap_or(id))?;
    let first = *path.indices.first()?;
    Some(doc.pages.get(path.page)?.children.get(first)?.id.clone())
}

/// One page's scene in page coordinates, uncached (tests, offscreen renders).
pub fn page_scene(doc: &Document, geom: &Geometry, page: &Page) -> Scene {
    page_scene_with_images(doc, geom, page, &ImageCache::default())
}

/// Like [`page_scene`], drawing images from `images`.
pub fn page_scene_with_images(
    doc: &Document,
    geom: &Geometry,
    page: &Page,
    images: &ImageCache,
) -> Scene {
    let mut scene = Scene::new();
    for node in &page.children {
        encode_node(
            &mut scene,
            doc,
            geom,
            node,
            &SceneOptions::default(),
            images,
        );
    }
    scene
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// Bounds of a subtree including strokes and shadows.
fn paint_bounds(doc: &Document, geom: &Geometry, node: &Node) -> Rect {
    let mut out: Option<Rect> = None;
    geom.walk_shown(node, &mut |n| {
        let Some(g) = geom.get(&n.id) else { return };
        let margin = n.shape().map_or(0.0, |_| {
            let stroke = if effective_strokes(doc, n).is_empty() {
                0.0
            } else {
                effective_stroke_width(doc, n) as f64
            };
            let shadow = effective_shadows(doc, n)
                .iter()
                .map(|sh| {
                    sh.blur as f64 * 1.5
                        + sh.spread.max(0.0) as f64
                        + sh.x.abs().max(sh.y.abs()) as f64
                })
                .fold(0.0, f64::max);
            stroke.max(shadow)
        });
        let b = g.bounds.inflate(margin + 1.0, margin + 1.0);
        out = Some(out.map_or(b, |o| o.union(b)));
    });
    out.unwrap_or(Rect::ZERO)
}

/// A rect far larger than any canvas, for layers that only change opacity.
const EVERYWHERE: Rect = Rect::new(-1e7, -1e7, 1e7, 1e7);

pub(crate) fn encode_node(
    scene: &mut Scene,
    doc: &Document,
    geom: &Geometry,
    node: &Node,
    opts: &SceneOptions,
    images: &ImageCache,
) {
    // An instance draws as the frame it stands for (§3.7), which has its
    // ID, visibility and opacity.
    let node = geom.shown(node);
    if !node.visible || opts.hidden.as_ref() == Some(&node.id) {
        return;
    }
    let Some(g) = geom.get(&node.id) else {
        return;
    };
    let layer = node.opacity < 1.0;
    if layer {
        scene.push_layer(
            Fill::NonZero,
            Mix::Normal,
            node.opacity.max(0.0),
            Affine::IDENTITY,
            &EVERYWHERE,
        );
    }
    let local = g.local_rect();
    match &node.kind {
        NodeKind::Frame(f) => {
            let rr = rounded(local, &f.shape.corner_radius);
            draw_shadows(scene, doc, g, node, &f.shape.corner_radius);
            draw_fills(scene, doc, images, g, node, &rr);
            if f.clip {
                scene.push_clip_layer(Fill::NonZero, g.transform, &rr);
            }
            for c in &f.children {
                encode_node(scene, doc, geom, c, opts, images);
            }
            if f.clip {
                scene.pop_layer();
            }
            draw_box_strokes(scene, doc, g, node, &f.shape, local);
        }
        NodeKind::Group(gr) => {
            for c in &gr.children {
                encode_node(scene, doc, geom, c, opts, images);
            }
        }
        NodeKind::Rect(r) => {
            let radius = CornerRadius::Each(effective_corner_radius(doc, node).map(RadiusRef::Px));
            let rr = rounded(local, &radius);
            draw_shadows(scene, doc, g, node, &radius);
            draw_fills(scene, doc, images, g, node, &rr);
            draw_box_strokes(scene, doc, g, node, &r.shape, local);
        }
        NodeKind::Ellipse(e) => {
            let radius = CornerRadius::px((local.width().min(local.height()) / 2.0) as f32);
            draw_shadows(scene, doc, g, node, &radius);
            draw_fills(scene, doc, images, g, node, &Ellipse::from_rect(local));
            let w = effective_stroke_width(doc, node) as f64;
            let r = stroke_rect(local, e.shape.stroke_align, w);
            draw_strokes(scene, doc, g, node, &Ellipse::from_rect(r));
        }
        NodeKind::Text(_) => {
            if let Some(tg) = geom.text(&node.id) {
                for paint in effective_fills(doc, node) {
                    if let Some((b, _)) = brush(doc, paint, g.size) {
                        draw_text(scene, g, tg, &b);
                    }
                }
            }
        }
        NodeKind::Vector(v) => match BezPath::from_svg(&v.path) {
            Ok(path) => {
                draw_fills(scene, doc, images, g, node, &path);
                draw_strokes(scene, doc, g, node, &path);
            }
            Err(e) => tracing::warn!("vector {}: bad path data: {e}", node.id),
        },
        NodeKind::Image(i) => {
            let radius = CornerRadius::Each(effective_corner_radius(doc, node).map(RadiusRef::Px));
            let rr = rounded(local, &radius);
            let size = (g.size.width, g.size.height);
            draw_image(scene, g.transform, &rr, size, images, &i.asset, i.fit);
        }
        // An instance whose component is missing (others draw as the frame
        // they stand for): a crossed-out box.
        NodeKind::Instance(_) => {
            let grey = Color::from_rgba8(0xcc, 0xcc, 0xcc, 0xff);
            scene.fill(
                Fill::NonZero,
                g.transform,
                Color::from_rgba8(0xee, 0xee, 0xee, 0xff),
                None,
                &local,
            );
            let mut cross = BezPath::new();
            cross.move_to(Point::ZERO);
            cross.line_to((local.x1, local.y1));
            cross.move_to((local.x1, 0.0));
            cross.line_to((0.0, local.y1));
            scene.stroke(&Stroke::new(1.0), g.transform, grey, None, &cross);
            scene.stroke(&Stroke::new(1.0), g.transform, grey, None, &local);
        }
    }
    if layer {
        scene.pop_layer();
    }
}

fn rounded(r: Rect, radius: &CornerRadius) -> RoundedRect {
    let max = r.width().min(r.height()) / 2.0;
    let c = |v: &RadiusRef| match v {
        RadiusRef::Px(v) => (*v as f64).clamp(0.0, max.max(0.0)),
        RadiusRef::Token(_) => 0.0,
    };
    let radii = match radius {
        CornerRadius::All(v) => RoundedRectRadii::from_single_radius(c(v)),
        CornerRadius::Each([tl, tr, br, bl]) => RoundedRectRadii::new(c(tl), c(tr), c(br), c(bl)),
    };
    RoundedRect::from_rect(r, radii)
}

/// The rect a stroke of width `w` follows so it sits inside, centered on or
/// outside the box.
fn stroke_rect(r: Rect, align: StrokeAlign, w: f64) -> Rect {
    match align {
        StrokeAlign::Inside => r.inset(-w / 2.0),
        StrokeAlign::Center => r,
        StrokeAlign::Outside => r.inset(w / 2.0),
    }
}

fn draw_fills(
    scene: &mut Scene,
    doc: &Document,
    images: &ImageCache,
    g: &NodeGeometry,
    node: &Node,
    path: &impl kurbo::Shape,
) {
    for paint in effective_fills(doc, node) {
        if let Paint::Image(ip) = paint {
            let size = (g.size.width, g.size.height);
            draw_image(scene, g.transform, path, size, images, &ip.asset, ip.fit);
        } else if let Some((b, tf)) = brush(doc, paint, g.size) {
            scene.fill(Fill::NonZero, g.transform, &b, tf, path);
        }
    }
}

fn draw_strokes(
    scene: &mut Scene,
    doc: &Document,
    g: &NodeGeometry,
    node: &Node,
    path: &impl kurbo::Shape,
) {
    let width = effective_stroke_width(doc, node);
    if width <= 0.0 {
        return;
    }
    let stroke = Stroke::new(width as f64);
    for paint in effective_strokes(doc, node) {
        if let Some((b, tf)) = brush(doc, paint, g.size) {
            scene.stroke(&stroke, g.transform, &b, tf, path);
        }
    }
}

fn draw_box_strokes(
    scene: &mut Scene,
    doc: &Document,
    g: &NodeGeometry,
    node: &Node,
    shape: &doc::Shape,
    local: Rect,
) {
    let w = effective_stroke_width(doc, node) as f64;
    let r = stroke_rect(local, shape.stroke_align, w);
    let offset = match shape.stroke_align {
        StrokeAlign::Inside => -w / 2.0,
        StrokeAlign::Center => 0.0,
        StrokeAlign::Outside => w / 2.0,
    };
    let radius = CornerRadius::Each(
        effective_corner_radius(doc, node)
            .map(|v| RadiusRef::Px((v as f64 + offset).max(0.0) as f32)),
    );
    draw_strokes(scene, doc, g, node, &rounded(r, &radius));
}

fn draw_shadows(
    scene: &mut Scene,
    doc: &Document,
    g: &NodeGeometry,
    node: &Node,
    radius: &CornerRadius,
) {
    // vello's blurred rect takes one radius; use the largest corner.
    let r = radius
        .corners()
        .iter()
        .map(|c| match c {
            RadiusRef::Px(v) => *v,
            RadiusRef::Token(_) => 0.0,
        })
        .fold(0.0f32, f32::max) as f64;
    for sh in effective_shadows(doc, node) {
        let color = resolve_color(doc, &sh.color);
        let rect = g.local_rect().inflate(sh.spread as f64, sh.spread as f64);
        let tf = g.transform * Affine::translate((sh.x as f64, sh.y as f64));
        scene.draw_blurred_rounded_rect(
            tf,
            rect,
            color,
            r + sh.spread.max(0.0) as f64,
            (sh.blur as f64 / 2.0).max(0.01),
        );
    }
}

fn draw_text(scene: &mut Scene, g: &NodeGeometry, tg: &TextGeometry, brush: &Brush) {
    let tf = g.transform * Affine::translate(tg.offset);
    for line in tg.layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let r = run.run();
            let coords: Vec<i16> = r.normalized_coords().iter().map(|c| c.to_bits()).collect();
            scene
                .draw_glyphs(r.font())
                .font_size(r.font_size())
                .transform(tf)
                .normalized_coords(&coords)
                .brush(brush)
                .draw(
                    Fill::NonZero,
                    run.positioned_glyphs().map(|gl| vello::Glyph {
                        id: gl.id,
                        x: gl.x,
                        y: gl.y,
                    }),
                );
            let style = run.style();
            let m = r.font_metrics();
            let x0 = run.offset() as f64;
            let x1 = x0 + run.advance() as f64;
            let base = run.baseline() as f64;
            let mut deco = |offset: f32, size: f32| {
                let y = base - offset as f64;
                let rect = Rect::new(x0, y, x1, y + size.max(1.0) as f64);
                scene.fill(Fill::NonZero, tf, brush, None, &rect);
            };
            if let Some(u) = &style.underline {
                deco(
                    u.offset.unwrap_or(m.underline_offset),
                    u.size.unwrap_or(m.underline_size),
                );
            }
            if let Some(s) = &style.strikethrough {
                deco(
                    s.offset.unwrap_or(m.strikethrough_offset),
                    s.size.unwrap_or(m.strikethrough_size),
                );
            }
        }
    }
}
