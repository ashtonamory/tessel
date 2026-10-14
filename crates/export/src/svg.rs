//! SVG export (§8): a hand-written, deterministic writer. Attribute order,
//! number formatting and generated IDs (`c1` clip paths, `g1` gradients,
//! `p1` patterns, `b1` blur filters, numbered in document order) are fixed so
//! the output can be golden-tested.
//!
//! Every node is a `<g id="<node id>">`; positions come from `layout`'s
//! geometry, so auto-layout is already resolved. A node's own paint sits in
//! a `<g transform>` (translate, plus rotate) in its local box; children of
//! frames and groups carry their own absolute transforms, so a clipping
//! frame is `<g clip-path>` around them. Text is `<text>` (not outlined)
//! with one `<tspan>` per line at the positions layout computed.

use std::collections::HashMap;
use std::fmt::Write as _;

use doc::resolve::{
    effective_corner_radius, effective_fills, effective_shadows, effective_stroke_width,
    effective_strokes, effective_text_style,
};
use doc::{
    Assets, ColorRef, Document, ImageFit, Node, NodeId, NodeKind, Paint, StrokeAlign, TextAlign,
    TextDecoration,
};
use layout::kurbo::{Rect, Size};
use layout::{Geometry, NodeGeometry};

use crate::ExportError;

#[derive(Clone, Copy, Debug)]
pub struct SvgOptions<'a> {
    /// The design file's name, for the header comment.
    pub file: &'a str,
    /// The document revision the export was made from.
    pub revision: u64,
    /// Where image bytes come from; images are skipped without it.
    pub assets: Option<&'a Assets>,
}

/// The node and its subtree as an SVG document. Hidden nodes are skipped.
pub fn export_svg(
    doc: &Document,
    geom: &Geometry,
    id: &NodeId,
    opts: &SvgOptions<'_>,
) -> Result<String, ExportError> {
    let (node, bounds) = doc
        .node(id)
        .zip(geom.get(id).map(|g| g.bounds))
        .ok_or_else(|| ExportError::NodeNotFound(id.to_string()))?;
    let mut w = Writer {
        doc,
        geom,
        assets: opts.assets,
        defs: String::new(),
        clips: 0,
        grads: 0,
        pats: 0,
        blurs: HashMap::new(),
    };
    let mut body = String::new();
    w.node(node, 1, &mut body);

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    // `--` may not appear inside an XML comment.
    let comment = |s: &str| esc(s).replace("--", "- -");
    let _ = writeln!(
        out,
        "<!-- tessel export: file=\"{}\" node=\"{}\" revision={} -->",
        comment(opts.file),
        comment(&id.to_string()),
        opts.revision
    );
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\">",
        n(bounds.width()),
        n(bounds.height()),
        n(bounds.x0),
        n(bounds.y0),
        n(bounds.width()),
        n(bounds.height()),
    );
    if !w.defs.is_empty() {
        out.push_str("  <defs>\n");
        out.push_str(&w.defs);
        out.push_str("  </defs>\n");
    }
    out.push_str(&body);
    out.push_str("</svg>\n");
    Ok(out)
}

// ---------------------------------------------------------------------------
// Formatting

/// A number with at most 3 decimals and no trailing zeros.
fn n(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 || !r.is_finite() {
        "0".to_string()
    } else {
        format!("{r}")
    }
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn pad(out: &mut String, depth: usize, line: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(line);
    out.push('\n');
}

/// `(#rrggbb, alpha)` of a literal or token color; unknown or malformed
/// colors come out magenta, as in the renderer.
fn color(doc: &Document, c: &ColorRef) -> (String, f64) {
    let parsed = doc::resolve::resolve_color(doc, c).and_then(|c| {
        let h = c.0.strip_prefix('#')?;
        if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
            return None;
        }
        let a = if h.len() == 8 {
            u8::from_str_radix(&h[6..8], 16).ok()? as f64 / 255.0
        } else {
            1.0
        };
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some((format!("#{}", h[0..6].to_ascii_lowercase()), a))
    });
    parsed.unwrap_or_else(|| ("#ff00ff".to_string(), 1.0))
}

/// `fill="#rrggbb" fill-opacity="0.5"` style attributes for a color.
fn color_attrs(doc: &Document, prop: &str, c: &ColorRef) -> String {
    let (hex, a) = color(doc, c);
    let mut s = format!("{prop}=\"{hex}\"");
    if n(a) != "1" {
        let _ = write!(s, " {prop}-opacity=\"{}\"", n(a));
    }
    s
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let v = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(v >> 18) as usize & 63] as char);
        out.push(T[(v >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(v >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[v as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub(crate) fn mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    }
}

// ---------------------------------------------------------------------------
// Shapes

/// The outline of a box-like node, in its local coordinates.
#[derive(Clone, Debug)]
enum Outline {
    /// A rect with `[tl, tr, br, bl]` radii.
    Rect(Rect, [f64; 4]),
    Ellipse(Rect),
    Path(String),
}

impl Outline {
    fn rect(r: Rect, radii: [f32; 4]) -> Outline {
        let max = (r.width().min(r.height()) / 2.0).max(0.0);
        Outline::Rect(r, radii.map(|v| (v as f64).clamp(0.0, max)))
    }

    /// The outline grown by `d` on every side (negative shrinks).
    fn grow(&self, d: f64) -> Outline {
        match self {
            Outline::Rect(r, radii) => {
                let g = r.inflate(d, d);
                let max = (g.width().min(g.height()) / 2.0).max(0.0);
                Outline::Rect(
                    g,
                    radii.map(|v| if v > 0.0 { (v + d).max(0.0) } else { 0.0 }.min(max)),
                )
            }
            Outline::Ellipse(r) => Outline::Ellipse(r.inflate(d, d)),
            Outline::Path(p) => Outline::Path(p.clone()),
        }
    }

    /// An element for the outline, with `attrs` (already formatted, may be
    /// empty) after the geometry.
    fn element(&self, attrs: &str) -> String {
        let tail = if attrs.is_empty() {
            String::new()
        } else {
            format!(" {attrs}")
        };
        match self {
            Outline::Rect(r, radii) if radii.iter().all(|v| *v == radii[0]) => {
                let rx = if radii[0] > 0.0 {
                    format!(" rx=\"{0}\" ry=\"{0}\"", n(radii[0]))
                } else {
                    String::new()
                };
                format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{rx}{tail}/>",
                    n(r.x0),
                    n(r.y0),
                    n(r.width()),
                    n(r.height())
                )
            }
            Outline::Rect(r, [tl, tr, br, bl]) => {
                let mut d = format!("M{} {}", n(r.x0 + tl), n(r.y0));
                let _ = write!(d, "H{}", n(r.x1 - tr));
                if *tr > 0.0 {
                    let _ = write!(d, "A{0} {0} 0 0 1 {1} {2}", n(*tr), n(r.x1), n(r.y0 + tr));
                }
                let _ = write!(d, "V{}", n(r.y1 - br));
                if *br > 0.0 {
                    let _ = write!(d, "A{0} {0} 0 0 1 {1} {2}", n(*br), n(r.x1 - br), n(r.y1));
                }
                let _ = write!(d, "H{}", n(r.x0 + bl));
                if *bl > 0.0 {
                    let _ = write!(d, "A{0} {0} 0 0 1 {1} {2}", n(*bl), n(r.x0), n(r.y1 - bl));
                }
                let _ = write!(d, "V{}", n(r.y0 + tl));
                if *tl > 0.0 {
                    let _ = write!(d, "A{0} {0} 0 0 1 {1} {2}", n(*tl), n(r.x0 + tl), n(r.y0));
                }
                d.push('Z');
                format!("<path d=\"{d}\"{tail}/>")
            }
            Outline::Ellipse(r) => format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{tail}/>",
                n((r.x0 + r.x1) / 2.0),
                n((r.y0 + r.y1) / 2.0),
                n(r.width() / 2.0),
                n(r.height() / 2.0)
            ),
            Outline::Path(p) => format!("<path d=\"{}\"{tail}/>", esc(p)),
        }
    }

    fn rounded(&self) -> bool {
        match self {
            Outline::Rect(_, radii) => radii.iter().any(|v| *v > 0.0),
            _ => true,
        }
    }
}

/// The `transform` of a node's local box: translation, plus the rotation
/// layout applied about its center.
fn transform(g: &NodeGeometry) -> Option<String> {
    let c = g.transform.as_coeffs();
    let deg = n(c[1].atan2(c[0]).to_degrees());
    let (x, y) = (n(c[4]), n(c[5]));
    match (deg.as_str(), x.as_str(), y.as_str()) {
        ("0", "0", "0") => None,
        ("0", _, _) => Some(format!("translate({x} {y})")),
        _ => Some(format!("translate({x} {y}) rotate({deg})")),
    }
}

// ---------------------------------------------------------------------------

struct Writer<'a> {
    doc: &'a Document,
    geom: &'a Geometry,
    assets: Option<&'a Assets>,
    defs: String,
    clips: u32,
    grads: u32,
    pats: u32,
    /// Blur filters by standard deviation, so equal blurs share one.
    blurs: HashMap<String, String>,
}

impl Writer<'_> {
    fn node(&mut self, node: &Node, depth: usize, out: &mut String) {
        // Instances draw as the frame they stand for, as on the canvas.
        let node = self.geom.shown(node);
        if !node.visible {
            return;
        }
        let Some(g) = self.geom.get(&node.id) else {
            return;
        };
        let mut open = format!("<g id=\"{}\"", esc(&node.id.to_string()));
        if !node.name.is_empty() {
            let _ = write!(open, " data-name=\"{}\"", esc(&node.name));
        }
        let container = matches!(node.kind, NodeKind::Frame(_) | NodeKind::Group(_));
        if !container {
            if let Some(t) = transform(g) {
                let _ = write!(open, " transform=\"{t}\"");
            }
        }
        if node.opacity < 1.0 {
            let _ = write!(open, " opacity=\"{}\"", n(node.opacity.max(0.0) as f64));
        }
        open.push('>');
        pad(out, depth, &open);
        let d = depth + 1;
        let local = g.local_rect();
        match &node.kind {
            NodeKind::Frame(f) => {
                let outline = Outline::rect(local, effective_corner_radius(self.doc, node));
                let mut paint = String::new();
                self.shadows(node, &outline, d + 1, &mut paint);
                self.fills(node, g.size, &outline, d + 1, &mut paint);
                self.wrap_local(g, d, &paint, out);
                if f.clip {
                    self.clips += 1;
                    let id = format!("c{}", self.clips);
                    let attrs =
                        transform(g).map_or(String::new(), |t| format!("transform=\"{t}\""));
                    let _ = writeln!(
                        self.defs,
                        "    <clipPath id=\"{id}\">\n      {}\n    </clipPath>",
                        outline.element(&attrs)
                    );
                    pad(out, d, &format!("<g clip-path=\"url(#{id})\">"));
                    for c in &f.children {
                        self.node(c, d + 1, out);
                    }
                    pad(out, d, "</g>");
                } else {
                    for c in &f.children {
                        self.node(c, d, out);
                    }
                }
                let mut strokes = String::new();
                self.box_strokes(node, g.size, d + 1, &mut strokes);
                self.wrap_local(g, d, &strokes, out);
            }
            NodeKind::Group(gr) => {
                for c in &gr.children {
                    self.node(c, d, out);
                }
            }
            NodeKind::Rect(_) => {
                let outline = Outline::rect(local, effective_corner_radius(self.doc, node));
                self.shadows(node, &outline, d, out);
                self.fills(node, g.size, &outline, d, out);
                self.box_strokes(node, g.size, d, out);
            }
            NodeKind::Ellipse(_) => {
                let outline = Outline::Ellipse(local);
                self.shadows(node, &outline, d, out);
                self.fills(node, g.size, &outline, d, out);
                self.box_strokes(node, g.size, d, out);
            }
            NodeKind::Vector(v) => {
                let outline = Outline::Path(v.path.clone());
                self.shadows(node, &outline, d, out);
                self.fills(node, g.size, &outline, d, out);
                self.strokes(node, g.size, &outline, d, out);
            }
            NodeKind::Text(_) => self.text(node, g, d, out),
            NodeKind::Image(i) => {
                let outline = Outline::rect(local, effective_corner_radius(self.doc, node));
                self.image(&i.asset, i.fit, g.size, &outline, d, out);
            }
            NodeKind::Instance(_) => {
                let outline = Outline::Rect(local, [0.0; 4]);
                pad(
                    out,
                    d,
                    &outline.element("fill=\"#eeeeee\" stroke=\"#cccccc\" stroke-width=\"1\""),
                );
            }
        }
        pad(out, depth, "</g>");
    }

    /// `body` (already indented one level deeper than `depth`) inside the
    /// node's local transform.
    fn wrap_local(&self, g: &NodeGeometry, depth: usize, body: &str, out: &mut String) {
        if body.is_empty() {
            return;
        }
        match transform(g) {
            Some(t) => {
                pad(out, depth, &format!("<g transform=\"{t}\">"));
                out.push_str(body);
                pad(out, depth, "</g>");
            }
            None => {
                // No transform: dedent the body by one level.
                for l in body.lines() {
                    out.push_str(l.strip_prefix("  ").unwrap_or(l));
                    out.push('\n');
                }
            }
        }
    }

    /// `fill`/`stroke` attributes for a paint; gradients add a definition.
    /// Image paints have none.
    fn paint_attrs(&mut self, prop: &str, paint: &Paint, size: Size) -> Option<String> {
        let (w, h) = (size.width, size.height);
        match paint {
            Paint::Solid(s) => Some(color_attrs(self.doc, prop, &s.color)),
            Paint::LinearGradient(lg) => {
                self.grads += 1;
                let id = format!("g{}", self.grads);
                let _ = writeln!(
                    self.defs,
                    "    <linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
                    n(lg.start[0] as f64 * w),
                    n(lg.start[1] as f64 * h),
                    n(lg.end[0] as f64 * w),
                    n(lg.end[1] as f64 * h),
                );
                self.stops(&lg.stops);
                self.defs.push_str("    </linearGradient>\n");
                Some(format!("{prop}=\"url(#{id})\""))
            }
            Paint::RadialGradient(rg) => {
                self.grads += 1;
                let id = format!("g{}", self.grads);
                let (cx, cy) = (rg.center[0] as f64 * w, rg.center[1] as f64 * h);
                let rx = (rg.radius[0] as f64 * w).max(1e-3);
                let ry = (rg.radius[1] as f64 * h).max(1e-3);
                // Elliptical: stretch the circle vertically about its center.
                let tf = if n(ry / rx) == "1" {
                    String::new()
                } else {
                    format!(
                        " gradientTransform=\"translate({} {}) scale(1 {}) translate({} {})\"",
                        n(cx),
                        n(cy),
                        n(ry / rx),
                        n(-cx),
                        n(-cy)
                    )
                };
                let _ = writeln!(
                    self.defs,
                    "    <radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{}\" cy=\"{}\" r=\"{}\"{tf}>",
                    n(cx),
                    n(cy),
                    n(rx),
                );
                self.stops(&rg.stops);
                self.defs.push_str("    </radialGradient>\n");
                Some(format!("{prop}=\"url(#{id})\""))
            }
            Paint::Image(_) => None,
        }
    }

    fn stops(&mut self, stops: &[doc::Stop]) {
        for s in stops {
            let (hex, a) = color(self.doc, &s.color);
            let op = if n(a) == "1" {
                String::new()
            } else {
                format!(" stop-opacity=\"{}\"", n(a))
            };
            let _ = writeln!(
                self.defs,
                "      <stop offset=\"{}\" stop-color=\"{hex}\"{op}/>",
                n(s.offset as f64)
            );
        }
    }

    fn fills(
        &mut self,
        node: &Node,
        size: Size,
        outline: &Outline,
        depth: usize,
        out: &mut String,
    ) {
        for paint in effective_fills(self.doc, node) {
            if let Paint::Image(ip) = paint {
                self.image(&ip.asset, ip.fit, size, outline, depth, out);
            } else if let Some(a) = self.paint_attrs("fill", paint, size) {
                pad(out, depth, &outline.element(&a));
            }
        }
    }

    fn strokes(
        &mut self,
        node: &Node,
        size: Size,
        outline: &Outline,
        depth: usize,
        out: &mut String,
    ) {
        let width = effective_stroke_width(self.doc, node) as f64;
        if width <= 0.0 {
            return;
        }
        for paint in effective_strokes(self.doc, node) {
            if let Some(a) = self.paint_attrs("stroke", paint, size) {
                pad(
                    out,
                    depth,
                    &outline.element(&format!("fill=\"none\" {a} stroke-width=\"{}\"", n(width))),
                );
            }
        }
    }

    /// Strokes of rect-like nodes, following the box inside, centered on or
    /// outside it as the renderer does.
    fn box_strokes(&mut self, node: &Node, size: Size, depth: usize, out: &mut String) {
        let Some(shape) = node.shape() else { return };
        let w = effective_stroke_width(self.doc, node) as f64;
        let d = match shape.stroke_align {
            StrokeAlign::Inside => -w / 2.0,
            StrokeAlign::Center => 0.0,
            StrokeAlign::Outside => w / 2.0,
        };
        let local = Rect::from_origin_size((0.0, 0.0), size);
        let outline = match &node.kind {
            NodeKind::Ellipse(_) => Outline::Ellipse(local),
            _ => Outline::rect(local, effective_corner_radius(self.doc, node)),
        };
        self.strokes(node, size, &outline.grow(d), depth, out);
    }

    fn shadows(&mut self, node: &Node, outline: &Outline, depth: usize, out: &mut String) {
        for sh in effective_shadows(self.doc, node) {
            let shape = outline.grow(sh.spread as f64);
            let mut attrs = color_attrs(self.doc, "fill", &sh.color);
            if sh.blur > 0.0 {
                let dev = n(sh.blur as f64 / 2.0);
                let next = self.blurs.len() + 1;
                let id = self
                    .blurs
                    .entry(dev.clone())
                    .or_insert_with(|| {
                        let id = format!("b{next}");
                        let _ = writeln!(
                            self.defs,
                            "    <filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">\n      <feGaussianBlur stdDeviation=\"{dev}\"/>\n    </filter>"
                        );
                        id
                    })
                    .clone();
                let _ = write!(attrs, " filter=\"url(#{id})\"");
            }
            if n(sh.x as f64) == "0" && n(sh.y as f64) == "0" {
                pad(out, depth, &shape.element(&attrs));
            } else {
                pad(
                    out,
                    depth,
                    &format!(
                        "<g transform=\"translate({} {})\">",
                        n(sh.x as f64),
                        n(sh.y as f64)
                    ),
                );
                pad(out, depth + 1, &shape.element(&attrs));
                pad(out, depth, "</g>");
            }
        }
    }

    fn image(
        &mut self,
        asset: &doc::AssetRef,
        fit: ImageFit,
        size: Size,
        outline: &Outline,
        depth: usize,
        out: &mut String,
    ) {
        let Some(bytes) = self.assets.and_then(|a| a.bytes(asset)) else {
            return;
        };
        let href = format!("data:{};base64,{}", mime(&bytes), base64(&bytes));
        let (w, h) = (n(size.width), n(size.height));
        if fit == ImageFit::Tile {
            let Some((iw, ih)) = render::natural_size(&bytes) else {
                return;
            };
            self.pats += 1;
            let id = format!("p{}", self.pats);
            let _ = writeln!(
                self.defs,
                "    <pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" width=\"{iw}\" height=\"{ih}\">\n      <image x=\"0\" y=\"0\" width=\"{iw}\" height=\"{ih}\" href=\"{href}\"/>\n    </pattern>"
            );
            pad(
                out,
                depth,
                &outline.element(&format!("fill=\"url(#{id})\"")),
            );
            return;
        }
        let aspect = match fit {
            ImageFit::Fill => "none",
            ImageFit::Fit => "xMidYMid meet",
            _ => "xMidYMid slice",
        };
        let image = format!(
            "<image x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"{aspect}\" href=\"{href}\"/>"
        );
        if outline.rounded() || fit == ImageFit::Crop {
            self.clips += 1;
            let id = format!("c{}", self.clips);
            let _ = writeln!(
                self.defs,
                "    <clipPath id=\"{id}\">\n      {}\n    </clipPath>",
                outline.element("")
            );
            pad(out, depth, &format!("<g clip-path=\"url(#{id})\">"));
            pad(out, depth + 1, &image);
            pad(out, depth, "</g>");
        } else {
            pad(out, depth, &image);
        }
    }

    fn text(&mut self, node: &Node, g: &NodeGeometry, depth: usize, out: &mut String) {
        let (NodeKind::Text(t), Some(style), Some(tg)) = (
            &node.kind,
            effective_text_style(self.doc, node),
            self.geom.text(&node.id),
        ) else {
            return;
        };
        let family = layout::resolve_family(self.doc, style);
        let shaped = layout::apply_transform(&t.content, style.text_transform);
        let shaped = if shaped.is_empty() { " " } else { &shaped };
        let mut head = format!(
            "<text font-family=\"{}, sans-serif\" font-size=\"{}\" font-weight=\"{}\"",
            esc(&family.replace('"', "'")),
            n(style.font_size as f64),
            style.font_weight
        );
        if style.italic {
            head.push_str(" font-style=\"italic\"");
        }
        if n(style.letter_spacing as f64) != "0" {
            let _ = write!(
                head,
                " letter-spacing=\"{}\"",
                n(style.letter_spacing as f64)
            );
        }
        match style.decoration {
            TextDecoration::None => {}
            TextDecoration::Underline => head.push_str(" text-decoration=\"underline\""),
            TextDecoration::Strike => head.push_str(" text-decoration=\"line-through\""),
        }
        let anchor = match t.align {
            TextAlign::Center => "middle",
            TextAlign::Right => "end",
            _ => "start",
        };
        let mut lines = String::new();
        for line in tg.layout.lines() {
            let m = line.metrics();
            let text = shaped
                .get(line.text_range())
                .unwrap_or("")
                .trim_end_matches(char::is_whitespace);
            if text.is_empty() {
                continue;
            }
            let x = tg.offset.x
                + m.offset as f64
                + match t.align {
                    TextAlign::Center => m.advance as f64 / 2.0,
                    TextAlign::Right => m.advance as f64,
                    _ => 0.0,
                };
            let y = tg.offset.y + m.baseline as f64;
            let _ = writeln!(
                lines,
                "<tspan x=\"{}\" y=\"{}\">{}</tspan>",
                n(x),
                n(y),
                esc(text)
            );
        }
        if lines.is_empty() {
            return;
        }
        for paint in effective_fills(self.doc, node) {
            let Some(a) = self.paint_attrs("fill", paint, g.size) else {
                continue;
            };
            pad(out, depth, &format!("{head} {a} text-anchor=\"{anchor}\">"));
            for l in lines.lines() {
                pad(out, depth + 1, l);
            }
            pad(out, depth, "</text>");
        }
    }
}
