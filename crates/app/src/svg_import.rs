//! File > Import SVG…: a small hand-written reader that turns the simple
//! shapes of an SVG file into Vector nodes. `<path d>` is used as is; rect,
//! circle, ellipse, line, polyline and polygon are converted to path data.
//! Fill and stroke colours are kept where simple. Everything else (text,
//! images, gradients, transforms other than translate, ...) is skipped and
//! reported, never guessed at.

use doc::{Command, Id, IdKind, Node, NodeKind, Paint as DocPaint, Shape as DocShape, StrokeAlign};

use crate::editor::Editor;
use kurbo::{Affine, BezPath, Circle, Ellipse, Point, Rect, RoundedRect, Shape};

/// One imported shape, in the SVG's own coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgShape {
    pub name: String,
    pub path: BezPath,
    /// `#rrggbb` / `#rrggbbaa`, or `None` for no fill.
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub stroke_width: f32,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub shapes: Vec<SvgShape>,
    /// What was left out, one short note per kind of thing.
    pub skipped: Vec<String>,
}

impl Parsed {
    fn skip(&mut self, what: impl Into<String>) {
        let what = what.into();
        if !self.skipped.contains(&what) {
            self.skipped.push(what);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Paint {
    None,
    Color(String),
    Unsupported,
}

#[derive(Clone, Debug)]
struct Scope {
    fill: Paint,
    stroke: Paint,
    stroke_width: f64,
    offset: (f64, f64),
    /// Everything below is left out (already reported, or invisible).
    skipping: bool,
}

impl Scope {
    fn root() -> Scope {
        Scope {
            fill: Paint::Color("#000000".into()),
            stroke: Paint::None,
            stroke_width: 1.0,
            offset: (0.0, 0.0),
            skipping: false,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Tag {
    Open,
    Close,
    SelfClosing,
}

struct RawTag {
    kind: Tag,
    name: String,
    attrs: Vec<(String, String)>,
}

impl RawTag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse `svg` into shapes. Fails only when it does not look like SVG.
pub fn parse(svg: &str) -> Result<Parsed, String> {
    let tags = tokenize(svg)?;
    if !tags.iter().any(|t| t.name == "svg") {
        return Err("that file is not an SVG".into());
    }
    let mut out = Parsed::default();
    let mut stack: Vec<Scope> = vec![Scope::root()];
    let mut counter = 0;
    for tag in tags {
        if tag.kind == Tag::Close {
            if stack.len() > 1 {
                stack.pop();
            }
            continue;
        }
        let parent = stack.last().expect("root stays").clone();
        let mut scope = parent.clone();
        let mut leaf = None;
        if !parent.skipping {
            apply_presentation(&mut scope, &tag);
            match tag.name.as_str() {
                "svg" | "g" | "a" | "switch" => {}
                "defs" | "symbol" | "clipPath" | "mask" | "style" | "title" | "desc"
                | "metadata" | "pattern" | "linearGradient" | "radialGradient" | "filter"
                | "marker" | "script" | "namedview" => scope.skipping = true,
                "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
                    leaf = Some(());
                }
                other => {
                    out.skip(format!("<{other}> elements"));
                    scope.skipping = true;
                }
            }
            if tag.attr("display") == Some("none") || tag.attr("visibility") == Some("hidden") {
                scope.skipping = true;
                leaf = None;
            }
            match tag.attr("transform").map(parse_translate) {
                None => {}
                Some(Some(t)) => {
                    scope.offset.0 += t.0;
                    scope.offset.1 += t.1;
                }
                Some(None) => {
                    // The root `<svg>` has none; anything else stays unplaced.
                    out.skip("transforms other than translate");
                    scope.skipping = true;
                    leaf = None;
                }
            }
        }
        if leaf.is_some() && !scope.skipping {
            match shape_of(&tag, &scope) {
                Ok(path) => {
                    counter += 1;
                    out.shapes
                        .push(finish(&tag, &scope, path, counter, &mut out.skipped));
                }
                Err(e) => out.skip(e),
            }
        }
        if tag.kind == Tag::Open {
            stack.push(scope);
        }
    }
    Ok(out)
}

fn finish(
    tag: &RawTag,
    scope: &Scope,
    mut path: BezPath,
    n: usize,
    skipped: &mut Vec<String>,
) -> SvgShape {
    path.apply_affine(Affine::translate(scope.offset));
    let mut paint = |p: &Paint, what: &str| match p {
        Paint::Color(c) => Some(c.clone()),
        Paint::None => None,
        Paint::Unsupported => {
            let note = format!("{what} gradients, patterns and other non-colour paints");
            if !skipped.contains(&note) {
                skipped.push(note);
            }
            None
        }
    };
    SvgShape {
        name: tag
            .attr("id")
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Path {n}")),
        fill: paint(&scope.fill, "fill"),
        stroke: paint(&scope.stroke, "stroke"),
        stroke_width: scope.stroke_width as f32,
        path,
    }
}

/// The element's outline as a path, in the element's own coordinates.
fn shape_of(tag: &RawTag, scope: &Scope) -> Result<BezPath, String> {
    let _ = scope;
    let num = |name: &str, default: f64| -> Result<f64, String> {
        match tag.attr(name) {
            None => Ok(default),
            Some(s) => parse_length(s)
                .ok_or_else(|| format!("<{}> with {name}=\"{s}\" (unsupported unit)", tag.name)),
        }
    };
    let path = match tag.name.as_str() {
        "path" => {
            let d = tag.attr("d").unwrap_or("");
            BezPath::from_svg(d).map_err(|e| format!("<path> with bad data ({e})"))?
        }
        "rect" => {
            let (x, y) = (num("x", 0.0)?, num("y", 0.0)?);
            let (w, h) = (num("width", 0.0)?, num("height", 0.0)?);
            let rect = Rect::new(x, y, x + w, y + h);
            let (rx, ry) = (num("rx", -1.0)?, num("ry", -1.0)?);
            let r = match (rx >= 0.0, ry >= 0.0) {
                (true, true) => rx.min(ry), // one radius per rect in the model
                (true, false) => rx,
                (false, true) => ry,
                (false, false) => 0.0,
            }
            .min(w.min(h) / 2.0)
            .max(0.0);
            if r > 0.0 {
                RoundedRect::from_rect(rect, r).to_path(0.1)
            } else {
                rect.to_path(0.1)
            }
        }
        "circle" => {
            Circle::new(Point::new(num("cx", 0.0)?, num("cy", 0.0)?), num("r", 0.0)?).to_path(0.1)
        }
        "ellipse" => Ellipse::new(
            Point::new(num("cx", 0.0)?, num("cy", 0.0)?),
            (num("rx", 0.0)?, num("ry", 0.0)?),
            0.0,
        )
        .to_path(0.1),
        "line" => {
            let mut p = BezPath::new();
            p.move_to((num("x1", 0.0)?, num("y1", 0.0)?));
            p.line_to((num("x2", 0.0)?, num("y2", 0.0)?));
            p
        }
        name @ ("polyline" | "polygon") => {
            let pts = parse_numbers(tag.attr("points").unwrap_or(""))
                .filter(|v| v.len() >= 4 && v.len() % 2 == 0)
                .ok_or_else(|| format!("<{name}> with bad points"))?;
            let mut p = BezPath::new();
            for (i, xy) in pts.chunks(2).enumerate() {
                if i == 0 {
                    p.move_to((xy[0], xy[1]));
                } else {
                    p.line_to((xy[0], xy[1]));
                }
            }
            if name == "polygon" {
                p.close_path();
            }
            p
        }
        _ => unreachable!("only shape elements get here"),
    };
    if path.is_empty() {
        return Err(format!("empty <{}> elements", tag.name));
    }
    Ok(path)
}

// --- attributes -----------------------------------------------------------

/// Fill, stroke and stroke-width, from attributes and then `style="..."`.
fn apply_presentation(scope: &mut Scope, tag: &RawTag) {
    let mut set = |k: &str, v: &str| match k {
        "fill" => scope.fill = parse_paint(v, &scope.fill),
        "stroke" => scope.stroke = parse_paint(v, &scope.stroke),
        "stroke-width" => {
            if let Some(w) = parse_length(v) {
                scope.stroke_width = w.max(0.0);
            }
        }
        _ => {}
    };
    for (k, v) in &tag.attrs {
        set(k, v.trim());
    }
    if let Some(style) = tag.attr("style") {
        for decl in style.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                set(k.trim(), v.trim());
            }
        }
    }
}

fn parse_paint(v: &str, inherited: &Paint) -> Paint {
    let v = v.trim();
    match v {
        "none" | "transparent" => Paint::None,
        "inherit" => inherited.clone(),
        // The text colour is not known here; black is the usual one.
        "currentColor" => Paint::Color("#000000".into()),
        _ => match parse_color(v) {
            Some(c) => Paint::Color(c),
            None => Paint::Unsupported,
        },
    }
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(...)`, `rgba(...)` or a
/// common colour name, as `#rrggbb` / `#rrggbbaa`.
pub fn parse_color(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let h = h.to_ascii_lowercase();
        return match h.len() {
            3 | 4 => Some(format!(
                "#{}",
                h.chars().flat_map(|c| [c, c]).collect::<String>()
            )),
            6 | 8 => Some(format!("#{h}")),
            _ => None,
        };
    }
    if let Some(args) = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<&str> = args
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 3 && parts.len() != 4 {
            return None;
        }
        let channel = |p: &str| -> Option<u8> {
            match p.strip_suffix('%') {
                Some(pct) => {
                    Some((pct.parse::<f64>().ok()?.clamp(0.0, 100.0) * 2.55).round() as u8)
                }
                None => Some(p.parse::<f64>().ok()?.clamp(0.0, 255.0).round() as u8),
            }
        };
        let mut out = format!(
            "#{:02x}{:02x}{:02x}",
            channel(parts[0])?,
            channel(parts[1])?,
            channel(parts[2])?
        );
        if let Some(a) = parts.get(3) {
            let a = match a.strip_suffix('%') {
                Some(p) => p.parse::<f64>().ok()? / 100.0,
                None => a.parse::<f64>().ok()?,
            };
            out.push_str(&format!(
                "{:02x}",
                (a.clamp(0.0, 1.0) * 255.0).round() as u8
            ));
        }
        return Some(out);
    }
    let hex = match s.to_ascii_lowercase().as_str() {
        "black" => "#000000",
        "white" => "#ffffff",
        "red" => "#ff0000",
        "green" => "#008000",
        "lime" => "#00ff00",
        "blue" => "#0000ff",
        "yellow" => "#ffff00",
        "cyan" | "aqua" => "#00ffff",
        "magenta" | "fuchsia" => "#ff00ff",
        "gray" | "grey" => "#808080",
        "silver" => "#c0c0c0",
        "maroon" => "#800000",
        "olive" => "#808000",
        "navy" => "#000080",
        "purple" => "#800080",
        "teal" => "#008080",
        "orange" => "#ffa500",
        "pink" => "#ffc0cb",
        "brown" => "#a52a2a",
        _ => return None,
    };
    Some(hex.to_string())
}

/// A plain number, optionally with `px`. Other units are not converted.
fn parse_length(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix("px").unwrap_or(s);
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_numbers(s: &str) -> Option<Vec<f64>> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect()
}

/// `translate(x [y])` (possibly several in a row) as an offset; `None` for
/// anything else.
fn parse_translate(s: &str) -> Option<(f64, f64)> {
    let mut rest = s.trim();
    let mut total = (0.0, 0.0);
    while !rest.is_empty() {
        let args = rest.strip_prefix("translate(")?;
        let (inner, after) = args.split_once(')')?;
        let v = parse_numbers(inner)?;
        match v.as_slice() {
            [x] => total.0 += x,
            [x, y] => {
                total.0 += x;
                total.1 += y;
            }
            _ => return None,
        }
        rest = after.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
    }
    Some(total)
}

// --- XML ------------------------------------------------------------------

fn tokenize(src: &str) -> Result<Vec<RawTag>, String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = skip_past(r, "-->")?;
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            rest = skip_past(r, "]]>")?;
        } else if let Some(r) = rest.strip_prefix("<?") {
            rest = skip_past(r, "?>")?;
        } else if let Some(r) = rest.strip_prefix("<!") {
            // <!DOCTYPE ...>, possibly with an [internal subset].
            rest = match r.find('[') {
                Some(b) if r.find('>').is_none_or(|g| b < g) => {
                    skip_past(skip_past(&r[b..], "]")?, ">")?
                }
                _ => skip_past(r, ">")?,
            };
        } else {
            let body = &rest[1..];
            let end = tag_end(body).ok_or("the SVG ends inside a tag")?;
            out.push(parse_tag(&body[..end])?);
            rest = &body[end + 1..];
        }
    }
    Ok(out)
}

fn skip_past<'a>(s: &'a str, end: &str) -> Result<&'a str, String> {
    s.find(end)
        .map(|i| &s[i + end.len()..])
        .ok_or_else(|| "the SVG ends inside a comment or declaration".to_string())
}

/// Index of the `>` that closes a tag, ignoring any inside quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_tag(body: &str) -> Result<RawTag, String> {
    let (kind, body) = if let Some(b) = body.strip_prefix('/') {
        (Tag::Close, b)
    } else if let Some(b) = body.strip_suffix('/') {
        (Tag::SelfClosing, b)
    } else {
        (Tag::Open, body)
    };
    let body = body.trim();
    let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
    // `svg:path` is `path`.
    let name = body[..name_end]
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        return Err("a tag without a name".into());
    }
    let mut attrs = Vec::new();
    let mut rest = body[name_end..].trim_start();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else {
            break; // a bare attribute: nothing we read
        };
        let key = rest[..eq].trim().rsplit(':').next().unwrap_or_default();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            return Err(format!("attribute {key} has no quoted value"));
        };
        let value_end = after[1..]
            .find(q)
            .ok_or_else(|| format!("attribute {key} is not closed"))?;
        attrs.push((key.to_string(), unescape(&after[1..1 + value_end])));
        rest = after[value_end + 2..].trim_start();
    }
    Ok(RawTag { kind, name, attrs })
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

// --- Into the document ----------------------------------------------------

impl Editor {
    /// Import the shapes of `svg` as Vector nodes, centered in `view` (page
    /// coordinates), in one undo step. Returns how many were created and
    /// reports skipped content in the status bar.
    pub fn import_svg(&mut self, svg: &str, view: Rect) -> Result<usize, String> {
        let parsed = parse(svg)?;
        let mut notes = String::new();
        if !parsed.skipped.is_empty() {
            notes = format!(" Skipped: {}.", parsed.skipped.join(", "));
        }
        if parsed.shapes.is_empty() {
            self.status = Some(format!("Nothing to import.{notes}"));
            return Err(format!("No supported shapes found in that SVG.{notes}"));
        }
        let union = parsed
            .shapes
            .iter()
            .map(|s| s.path.bounding_box())
            .reduce(|a, b| a.union(b))
            .expect("not empty");
        let shift = (
            (view.center().x - union.center().x).round(),
            (view.center().y - union.center().y).round(),
        );
        let parent = self.insert_parent();
        let mut ids = Vec::new();
        let mut cmds = Vec::new();
        for s in parsed.shapes {
            let b = s.path.bounding_box();
            let mut path = s.path;
            path.apply_affine(Affine::translate((-b.x0, -b.y0)));
            let solid = |c: &str| DocPaint::solid(doc::ColorRef::literal(c));
            let shape = DocShape {
                fills: s.fill.iter().map(|c| solid(c)).collect(),
                strokes: s.stroke.iter().map(|c| solid(c)).collect(),
                stroke_width: s.stroke_width,
                stroke_align: StrokeAlign::Center,
                ..Default::default()
            };
            let id = Id::new(IdKind::Node);
            let node = Node::new(
                id.clone(),
                s.name,
                NodeKind::Vector(doc::Vector {
                    path: path.to_svg(),
                    shape,
                    extra: Default::default(),
                }),
            )
            .with_geometry(
                (b.x0 + shift.0) as f32,
                (b.y0 + shift.1) as f32,
                b.width() as f32,
                b.height() as f32,
            );
            cmds.push(Command::CreateNode {
                parent: parent.clone(),
                index: None,
                node,
            });
            ids.push(id);
        }
        let n = ids.len();
        self.commit("Import SVG", vec![Command::Batch(cmds)], ids)
            .map_err(|e| e.to_string())?;
        let plural = if n == 1 { "" } else { "s" };
        self.status = Some(format!("Imported {n} shape{plural}.{notes}"));
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(svg: &str) -> SvgShape {
        let p = parse(svg).unwrap();
        assert_eq!(p.shapes.len(), 1, "{p:?}");
        p.shapes.into_iter().next().unwrap()
    }

    fn bbox(s: &SvgShape) -> (f64, f64, f64, f64) {
        let b = s.path.bounding_box();
        (b.x0, b.y0, b.x1, b.y1)
    }

    #[test]
    fn path_with_fill_and_stroke() {
        let s = one(r##"<?xml version="1.0"?>
            <!-- a comment with <path d="M0 0"/> inside -->
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20">
              <path id="tri" d="M0 0 L10 0 L5 8 Z" fill="#f00" stroke="rgb(0, 128, 255)" stroke-width="2"/>
            </svg>"##);
        assert_eq!(s.name, "tri");
        assert_eq!(s.fill.as_deref(), Some("#ff0000"));
        assert_eq!(s.stroke.as_deref(), Some("#0080ff"));
        assert_eq!(s.stroke_width, 2.0);
        assert_eq!(bbox(&s), (0.0, 0.0, 10.0, 8.0));
    }

    #[test]
    fn defaults_inheritance_and_style_attribute() {
        let p = parse(
            r##"<svg><g fill="blue" stroke="black"><rect width="4" height="4"/>
                 <rect width="4" height="4" style="fill:none; stroke: #0f0"/></g>
                 <circle r="3"/></svg>"##,
        )
        .unwrap();
        assert_eq!(p.shapes.len(), 3);
        assert_eq!(p.shapes[0].fill.as_deref(), Some("#0000ff"));
        assert_eq!(p.shapes[0].stroke.as_deref(), Some("#000000"));
        assert_eq!(p.shapes[1].fill, None);
        assert_eq!(p.shapes[1].stroke.as_deref(), Some("#00ff00"));
        // SVG's default fill is black, and there is no stroke.
        assert_eq!(p.shapes[2].fill.as_deref(), Some("#000000"));
        assert_eq!(p.shapes[2].stroke, None);
    }

    #[test]
    fn basic_shapes_become_paths() {
        assert_eq!(
            bbox(&one(
                r#"<svg><rect x="2" y="3" width="10" height="5"/></svg>"#
            )),
            (2.0, 3.0, 12.0, 8.0)
        );
        let r = one(r#"<svg><rect width="10" height="10" rx="3"/></svg>"#);
        assert!(r.path.elements().len() > 5, "rounded corners add curves");
        assert_eq!(
            bbox(&one(r#"<svg><circle cx="5" cy="5" r="4"/></svg>"#)),
            (1.0, 1.0, 9.0, 9.0)
        );
        let e = bbox(&one(
            r#"<svg><ellipse cx="10" cy="5" rx="6" ry="2"/></svg>"#,
        ));
        assert!(
            (e.0 - 4.0).abs() < 1e-6 && (e.3 - 7.0).abs() < 1e-6,
            "{e:?}"
        );
        assert_eq!(
            bbox(&one(
                r#"<svg><line x1="1" y1="2" x2="9" y2="6" stroke="red"/></svg>"#
            )),
            (1.0, 2.0, 9.0, 6.0)
        );
        let pl = one(r#"<svg><polyline points="0,0 10,0 10,10"/></svg>"#);
        assert_eq!(pl.path.to_svg(), "M0,0 L10,0 L10,10");
        let pg = one(r#"<svg><polygon points="0 0 10 0 5 5"/></svg>"#);
        assert_eq!(pg.path.to_svg(), "M0,0 L10,0 L5,5 Z");
    }

    #[test]
    fn translate_is_applied_and_other_transforms_are_skipped() {
        let s = one(
            r#"<svg><g transform="translate(10 20)"><rect transform="translate(1,2)" width="2" height="2"/></g></svg>"#,
        );
        assert_eq!(bbox(&s), (11.0, 22.0, 13.0, 24.0));
        let p = parse(r#"<svg><rect transform="rotate(45)" width="2" height="2"/></svg>"#).unwrap();
        assert!(p.shapes.is_empty());
        assert_eq!(p.skipped, vec!["transforms other than translate"]);
    }

    #[test]
    fn unsupported_things_are_reported_not_imported() {
        let p = parse(
            r##"<svg><defs><linearGradient id="g"><stop offset="0"/></linearGradient><path d="M0 0L1 1"/></defs>
                <text x="1">Hi <tspan>there</tspan></text>
                <image href="a.png"/>
                <path d="M0 0 L oops"/>
                <path d="M0 0L5 5" fill="url(#g)"/>
                <rect width="5em" height="2"/>
                <rect width="5" height="2" display="none"/>
                <circle r="1"/></svg>"##,
        )
        .unwrap();
        let names: Vec<_> = p.shapes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Path 1", "Path 2"], "{p:?}");
        assert_eq!(p.shapes[0].fill, None, "gradient fill is dropped");
        for note in ["<text> elements", "<image> elements"] {
            assert!(
                p.skipped.iter().any(|s| s == note),
                "{note} in {:?}",
                p.skipped
            );
        }
        assert!(p.skipped.iter().any(|s| s.contains("bad data")));
        assert!(p.skipped.iter().any(|s| s.contains("gradients")));
        assert!(p.skipped.iter().any(|s| s.contains("unsupported unit")));
    }

    #[test]
    fn colours() {
        assert_eq!(parse_color("#abc").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_color("#AABBCCDD").as_deref(), Some("#aabbccdd"));
        assert_eq!(
            parse_color("rgba(255, 0, 0, 0.5)").as_deref(),
            Some("#ff000080")
        );
        assert_eq!(parse_color("rgb(100%, 0%, 0%)").as_deref(), Some("#ff0000"));
        assert_eq!(parse_color("Orange").as_deref(), Some("#ffa500"));
        assert_eq!(parse_color("url(#x)"), None);
        assert_eq!(parse_color("#12"), None);
    }

    #[test]
    fn rejects_non_svg_and_broken_xml() {
        assert!(parse("hello").is_err());
        assert!(parse("<html><body/></html>").is_err());
        assert!(parse("<svg><path d=\"M0 0").is_err());
        // Namespaced tags and entities.
        let s = one(r#"<svg:svg xmlns:svg="x"><svg:path id="a&amp;b" d="M0 0L2 2"/></svg:svg>"#);
        assert_eq!(s.name, "a&b");
    }
}
