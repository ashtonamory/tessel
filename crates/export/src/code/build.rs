//! Nodes to the element tree ([`super::ir`]).
//!
//! Placement follows §3.5:
//! - The exported node is the root: `position: relative` at its own size.
//! - A flow child of an auto-layout frame is a flex item: `fixed` is
//!   `width: Wpx`, `hug` is no size, `fill` is `flex: 1 1 0` on the main
//!   axis and `align-self: stretch` on the cross axis; anything else does
//!   not grow or shrink (`flex-shrink: 0`), like taffy.
//! - Children of frames without auto-layout, and `position: absolute`
//!   children, are `position: absolute` with their constraints as
//!   `left`/`right`/`top`/`bottom` (`center` is `calc(50% ± d)`, `scale`
//!   percentages) inside a `position: relative` parent.
//! - Groups have no box: their children are written in place, offset by the
//!   group's position. A group that auto-layout places, or one with its own
//!   opacity or rotation, is a box the size of its content instead.
//!
//! Strokes: an `inside` stroke of a leaf is a `border` (boxes are
//! `border-box`, so its size is unchanged). Frames with children, and
//! `center`/`outside` strokes, use `outline` with an `outline-offset`,
//! because a border would move the children while the design's strokes
//! never take space. Only the topmost stroke paint is used; a gradient
//! stroke takes its first stop's color.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use doc::resolve::{
    effective_fills, effective_shadows, effective_stroke_width, effective_strokes,
    effective_text_style,
};
use doc::{
    Align, AssetRef, AutoLayout, ChildPosition, Color, ColorRef, Component, ComponentId,
    CornerRadius, Direction, Document, HConstraint, ImageFit, Instance, Justify, LineHeight, Node,
    NodeId, NodeKind, Override, Paint, PropKind, RadiusRef, Sizing, SpaceRef, Stop, StrokeAlign,
    StyleId, StyleKind, TextAlign, TextDecoration, TextSizing, TextStyle, TextTransform, TokenId,
    VAlign, VConstraint,
};
use layout::{effective_sizing, Geometry};

use super::ir::{num, path_data, px, Body, Call, Decl, El, Item, Part, PropVal, TextStyleRef, Val};
use super::names::{kebab, TokKind, TokenNames, Unique};
use super::{expand_instance, CodeOptions, ImageMode};

/// Tags a `semantic` hint may pick. Anything else falls back to the
/// node kind's default.
const SEMANTIC: &[&str] = &[
    "a",
    "article",
    "aside",
    "blockquote",
    "button",
    "em",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "label",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "section",
    "small",
    "span",
    "strong",
    "ul",
];

/// Tags whose content must be phrasing content: boxes inside them are
/// `span`s rather than `div`s/`p`s.
const PHRASING: &[&str] = &[
    "a", "button", "em", "h1", "h2", "h3", "h4", "h5", "h6", "label", "p", "small", "span",
    "strong",
];

/// Where a node is written.
#[derive(Clone, Copy)]
pub(crate) struct Parent<'p> {
    /// The parent's auto-layout, which places its flow children.
    pub flex: Option<&'p AutoLayout>,
    /// The parent's size, for constraints.
    pub size: (f64, f64),
    /// Added to the node's `x`/`y` (children of flattened groups).
    pub offset: (f64, f64),
    /// The node is the exported root.
    pub root: bool,
    /// The parent only allows phrasing content.
    pub phrasing: bool,
}

impl Parent<'static> {
    pub fn root() -> Parent<'static> {
        Parent {
            flex: None,
            size: (0.0, 0.0),
            offset: (0.0, 0.0),
            root: true,
            phrasing: false,
        }
    }
}

/// The text styles of the document with their class names, in document
/// order.
pub(crate) fn text_style_names(doc: &Document) -> Vec<(StyleId, String, &TextStyle)> {
    let mut names = Unique::default();
    doc.styles
        .iter()
        .filter_map(|s| match &s.kind {
            StyleKind::Text(t) => Some((
                s.id.clone(),
                names.take(&kebab(&s.name, "text-style"), "-"),
                &t.style,
            )),
            _ => None,
        })
        .collect()
}

/// A color as written: lowercase hex, or magenta when malformed (as the
/// renderer draws it).
pub(crate) fn color_lit(c: &Color) -> String {
    if c.is_well_formed() {
        c.0.to_ascii_lowercase()
    } else {
        "#ff00ff".to_string()
    }
}

/// A font family name as code may carry it: ASCII letters and digits,
/// spaces, `-`, `_` and `.`; everything else is dropped.
pub(crate) fn family_name(family: &str) -> String {
    family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect()
}

/// A font family as a CSS value with a generic fallback.
pub(crate) fn family_lit(family: &str) -> String {
    format!("'{}', sans-serif", family_name(family))
}

pub(crate) struct Builder<'a> {
    pub doc: &'a Document,
    geom: &'a Geometry,
    opts: &'a CodeOptions<'a>,
    pub toks: TokenNames,
    text_styles: HashMap<StyleId, String>,
    classes: Unique,
    /// Assets the output references as files.
    pub assets: BTreeSet<AssetRef>,
    /// Components being expanded, against cycles.
    stack: Vec<ComponentId>,
    /// React: instances are component calls where their overrides allow.
    pub calls: bool,
    pub comp_names: HashMap<ComponentId, String>,
    /// While building a component: its props by target node.
    pub props: HashMap<NodeId, Vec<(PropKind, String)>>,
    /// While building a component: the default content of swap props.
    pub slot_defaults: BTreeMap<String, Vec<Item>>,
    /// Components called by what was built since the last reset.
    pub used: BTreeSet<String>,
}

impl<'a> Builder<'a> {
    pub fn new(doc: &'a Document, geom: &'a Geometry, opts: &'a CodeOptions<'a>) -> Builder<'a> {
        let mut classes = Unique::default();
        let mut text_styles = HashMap::new();
        for (id, name, _) in text_style_names(doc) {
            classes.take(&name, "-");
            text_styles.insert(id, name);
        }
        Builder {
            doc,
            geom,
            opts,
            toks: TokenNames::new(doc),
            text_styles,
            classes,
            assets: BTreeSet::new(),
            stack: Vec::new(),
            calls: false,
            comp_names: HashMap::new(),
            props: HashMap::new(),
            slot_defaults: BTreeMap::new(),
            used: BTreeSet::new(),
        }
    }

    /// The exported node as the root of the output.
    pub fn root(&mut self, n: &Node) -> Vec<Item> {
        let mut out = Vec::new();
        self.node(n, Parent::root(), &mut out);
        out
    }

    // -----------------------------------------------------------------------
    // Values

    fn tok(&self, kind: TokKind, id: &TokenId, lit: String) -> Option<Val> {
        let name = self.toks.get(kind, id)?;
        Some(Val(vec![Part::Tok {
            kind,
            css: name.css.clone(),
            key: name.key.clone(),
            lit,
        }]))
    }

    pub fn color(&self, c: &ColorRef) -> Val {
        match c {
            ColorRef::Literal(c) => Val::lit(color_lit(c)),
            ColorRef::Token(t) => self
                .doc
                .tokens
                .colors
                .get(t)
                .and_then(|c| self.tok(TokKind::Color, t, color_lit(c)))
                .unwrap_or_else(|| Val::lit("#ff00ff")),
        }
    }

    fn space(&self, s: &SpaceRef) -> Val {
        match s {
            SpaceRef::Px(v) => Val::lit(px(v.max(0.0) as f64)),
            SpaceRef::Token(t) => self
                .doc
                .tokens
                .spacing
                .get(t)
                .and_then(|v| self.tok(TokKind::Space, t, px(v.max(0.0) as f64)))
                .unwrap_or_else(|| Val::lit("0")),
        }
    }

    fn radius(&self, r: &RadiusRef) -> Val {
        match r {
            RadiusRef::Px(v) => Val::lit(px(v.max(0.0) as f64)),
            RadiusRef::Token(t) => self
                .doc
                .tokens
                .radii
                .get(t)
                .and_then(|v| self.tok(TokKind::Radius, t, px(v.max(0.0) as f64)))
                .unwrap_or_else(|| Val::lit("0")),
        }
    }

    pub fn font(&self, family: &str) -> Val {
        if let Some(t) = TokenId::from_reference(family) {
            if let Some(v) = self
                .doc
                .tokens
                .fonts
                .get(&t)
                .and_then(|f| self.tok(TokKind::Font, &t, family_lit(f)))
            {
                return v;
            }
        }
        Val::lit(family_lit(&doc::resolve::resolve_font(self.doc, family)))
    }

    fn size_of(&self, n: &Node) -> (f64, f64) {
        match self.geom.get(&n.id) {
            Some(g) => (g.size.width, g.size.height),
            None => (n.width.max(0.0) as f64, n.height.max(0.0) as f64),
        }
    }

    fn class_for(&mut self, n: &Node) -> String {
        self.classes.take(&kebab(&n.name, n.kind.tag()), "-")
    }

    fn new_el(&mut self, tag: &str, n: &Node) -> El {
        let mut el = El::new(tag, self.class_for(n));
        if self.opts.node_ids {
            el.node_id = Some(n.id.to_string());
        }
        el
    }

    fn image_src(&mut self, asset: &AssetRef) -> String {
        if self.opts.images == ImageMode::DataUri {
            if let Some(bytes) = self.opts.assets.and_then(|a| a.bytes(asset)) {
                return format!(
                    "data:{};base64,{}",
                    crate::svg::mime(&bytes),
                    crate::svg::base64(&bytes)
                );
            }
        }
        self.assets.insert(asset.clone());
        super::asset_path(asset)
    }

    // -----------------------------------------------------------------------
    // Nodes

    fn node(&mut self, n: &Node, parent: Parent<'_>, out: &mut Vec<Item>) {
        let prop = |kind: PropKind| {
            self.props
                .get(&n.id)
                .and_then(|ps| ps.iter().find(|(k, _)| *k == kind))
                .map(|(_, name)| name.clone())
        };
        let cond = prop(PropKind::Visible);
        let swap = prop(PropKind::Swap);
        if !n.visible && cond.is_none() {
            return;
        }
        let start = out.len();
        if let Some(name) = swap {
            let mut default = Vec::new();
            self.node_inner(n, parent, &mut default);
            self.slot_defaults.insert(name.clone(), default);
            out.push(Item::Slot(name, None));
        } else {
            self.node_inner(n, parent, out);
        }
        if let Some(c) = cond {
            for item in &mut out[start..] {
                item.set_cond(&c);
            }
        }
    }

    fn node_inner(&mut self, n: &Node, parent: Parent<'_>, out: &mut Vec<Item>) {
        match &n.kind {
            NodeKind::Group(g) => self.group(n, &g.children, parent, out),
            NodeKind::Instance(i) => self.instance(n, i, parent, out),
            _ => {
                let el = self.element(n, parent);
                out.push(Item::El(el));
            }
        }
    }

    fn in_flow(n: &Node, parent: Parent<'_>) -> bool {
        !parent.root && parent.flex.is_some() && n.layout_child.position == ChildPosition::Flow
    }

    fn tag(n: &Node, parent: Parent<'_>) -> &'static str {
        let semantic = match &n.kind {
            NodeKind::Frame(f) => f.semantic.as_deref(),
            NodeKind::Text(t) => t.semantic.as_deref(),
            _ => None,
        };
        if let Some(s) = semantic.and_then(|s| {
            let s = s.trim().to_ascii_lowercase();
            SEMANTIC.iter().find(|t| **t == s)
        }) {
            return s;
        }
        match &n.kind {
            NodeKind::Text(_) | NodeKind::Frame(_) | NodeKind::Rect(_) | NodeKind::Ellipse(_)
                if parent.phrasing =>
            {
                "span"
            }
            NodeKind::Text(_) => "p",
            NodeKind::Image(i) if i.fit != ImageFit::Tile => "img",
            NodeKind::Vector(_) => "svg",
            _ => "div",
        }
    }

    fn element(&mut self, n: &Node, parent: Parent<'_>) -> El {
        let flow = Self::in_flow(n, parent);
        let sizing = effective_sizing(n, flow);
        let size = self.size_of(n);
        let tag = Self::tag(n, parent);
        let mut el = self.new_el(tag, n);
        let container = n.children().is_some_and(|c| !c.is_empty());
        let bx = (
            parent.offset.0 + n.x as f64,
            parent.offset.1 + n.y as f64,
            size.0,
            size.1,
        );
        el.place = self.place(n, sizing, bx, parent, container);
        match &n.kind {
            NodeKind::Frame(f) => {
                if let Some(al) = &f.layout {
                    self.flex_decls(al, &mut el.decls);
                }
                if f.clip && f.children.iter().any(|c| c.visible) {
                    el.decls.push(Decl::lit("overflow", "hidden"));
                }
                self.background(effective_fills(self.doc, n), size, &mut el.decls);
                self.stroke(n, container, &mut el.decls);
                if let Some(r) = self.corner_radius(&f.shape.corner_radius) {
                    el.decls.push(Decl::new("border-radius", r));
                }
                self.box_shadow(n, &mut el.decls);
                let child_parent = Parent {
                    flex: f.layout.as_ref(),
                    size,
                    offset: (0.0, 0.0),
                    root: false,
                    phrasing: parent.phrasing || PHRASING.contains(&tag),
                };
                let mut items = Vec::new();
                for c in &f.children {
                    self.node(c, child_parent, &mut items);
                }
                if !items.is_empty() {
                    el.body = Body::Children(items);
                }
            }
            NodeKind::Rect(r) => {
                self.background(effective_fills(self.doc, n), size, &mut el.decls);
                self.stroke(n, false, &mut el.decls);
                if let Some(r) = self.corner_radius(&r.shape.corner_radius) {
                    el.decls.push(Decl::new("border-radius", r));
                }
                self.box_shadow(n, &mut el.decls);
            }
            NodeKind::Ellipse(_) => {
                self.background(effective_fills(self.doc, n), size, &mut el.decls);
                self.stroke(n, false, &mut el.decls);
                el.decls.push(Decl::lit("border-radius", "50%"));
                self.box_shadow(n, &mut el.decls);
            }
            NodeKind::Text(t) => {
                let style = effective_text_style(self.doc, n).expect("a text node");
                el.font = self.font_decls(style);
                el.text_style = t
                    .text_style
                    .as_ref()
                    .filter(|id| {
                        doc::resolve::style(self.doc, id)
                            .is_some_and(|s| matches!(s.kind, StyleKind::Text(_)))
                    })
                    .and_then(|id| self.text_styles.get(id))
                    .map(|name| TextStyleRef { name: name.clone() });
                self.text_fill(effective_fills(self.doc, n), size, &mut el.decls);
                if t.align != TextAlign::Left {
                    el.decls.push(Decl::lit(
                        "text-align",
                        match t.align {
                            TextAlign::Center => "center",
                            TextAlign::Right => "right",
                            _ => "justify",
                        },
                    ));
                }
                let wraps = if flow {
                    t.sizing != TextSizing::AutoWidth || n.layout_child.width == Sizing::Fill
                } else {
                    t.sizing != TextSizing::AutoWidth
                };
                el.decls.push(Decl::lit(
                    "white-space",
                    if wraps { "pre-wrap" } else { "pre" },
                ));
                let fixed_height = t.sizing == TextSizing::Fixed
                    || (flow && n.layout_child.height == Sizing::Fill);
                if fixed_height && t.vertical_align != VAlign::Top {
                    el.decls.push(Decl::lit("display", "flex"));
                    el.decls.push(Decl::lit("flex-direction", "column"));
                    el.decls.push(Decl::lit(
                        "justify-content",
                        if t.vertical_align == VAlign::Middle {
                            "center"
                        } else {
                            "flex-end"
                        },
                    ));
                }
                let text_prop = self
                    .props
                    .get(&n.id)
                    .and_then(|ps| ps.iter().find(|(k, _)| *k == PropKind::Text));
                el.body = match text_prop {
                    Some((_, name)) => Body::Prop(name.clone()),
                    None => Body::Text(t.content.clone()),
                };
            }
            NodeKind::Image(i) => {
                let src = self.image_src(&i.asset);
                if i.fit == ImageFit::Tile {
                    let mut v = Val::lit(format!("url(\"{src}\") 0 0 / auto repeat"));
                    el.decls
                        .push(Decl::new("background", std::mem::take(&mut v)));
                } else {
                    el.attrs.push(("src".into(), src));
                    el.attrs.push(("alt".into(), n.name.clone()));
                    el.decls.push(Decl::lit(
                        "object-fit",
                        match i.fit {
                            ImageFit::Fill => "fill",
                            ImageFit::Fit => "contain",
                            _ => "cover",
                        },
                    ));
                }
                if let Some(r) = self.corner_radius(&i.corner_radius) {
                    el.decls.push(Decl::new("border-radius", r));
                }
            }
            NodeKind::Vector(v) => {
                // The path is in the node's local px, the svg's user units
                // without a viewBox (a viewBox would break for lines with
                // zero width or height).
                el.decls.push(Decl::lit("overflow", "visible"));
                let shadows = effective_shadows(self.doc, n);
                if !shadows.is_empty() {
                    let parts: Vec<Val> = shadows
                        .iter()
                        .rev()
                        .map(|s| {
                            let mut v = Val::lit(format!(
                                "drop-shadow({} {} {} ",
                                px(s.x as f64),
                                px(s.y as f64),
                                px(s.blur.max(0.0) as f64)
                            ));
                            v.append(&self.color(&s.color)).push(")");
                            v
                        })
                        .collect();
                    el.decls.push(Decl::new("filter", Val::join(&parts, " ")));
                }
                let mut path = El::new(
                    "path",
                    self.classes.take(&format!("{}-path", el.class), "-"),
                );
                path.attrs.push(("d".into(), path_data(&v.path)));
                let fill = match effective_fills(self.doc, n).last() {
                    Some(p) => self.flat_color(p),
                    None => None,
                };
                path.decls
                    .push(Decl::new("fill", fill.unwrap_or_else(|| Val::lit("none"))));
                let w = effective_stroke_width(self.doc, n);
                if let Some(c) = effective_strokes(self.doc, n)
                    .last()
                    .and_then(|p| self.flat_color(p))
                    .filter(|_| w > 0.0)
                {
                    path.decls.push(Decl::new("stroke", c));
                    path.decls.push(Decl::lit("stroke-width", num(w as f64)));
                }
                el.body = Body::Children(vec![Item::El(path)]);
            }
            NodeKind::Group(_) | NodeKind::Instance(_) => unreachable!("handled by node_inner"),
        }
        Self::common(n, &mut el.decls);
        el
    }

    /// Opacity and rotation (CSS rotates about the center, as the design).
    fn common(n: &Node, decls: &mut Vec<Decl>) {
        if n.opacity < 1.0 {
            decls.push(Decl::lit("opacity", num(n.opacity.max(0.0) as f64)));
        }
        if num(n.rotation as f64) != "0" {
            decls.push(Decl::lit(
                "transform",
                format!("rotate({}deg)", num(n.rotation as f64)),
            ));
        }
    }

    fn group(&mut self, n: &Node, children: &[Node], parent: Parent<'_>, out: &mut Vec<Item>) {
        let flow = Self::in_flow(n, parent);
        if !parent.root && !flow && n.opacity >= 1.0 && num(n.rotation as f64) == "0" {
            let p = Parent {
                offset: (parent.offset.0 + n.x as f64, parent.offset.1 + n.y as f64),
                ..parent
            };
            for c in children {
                self.node(c, p, out);
            }
            return;
        }
        let b = self.content_bounds(children);
        let mut el = self.new_el(if parent.phrasing { "span" } else { "div" }, n);
        let bx = (
            parent.offset.0 + n.x as f64 + b.0,
            parent.offset.1 + n.y as f64 + b.1,
            b.2 - b.0,
            b.3 - b.1,
        );
        el.place = self.place(n, (Sizing::Fixed, Sizing::Fixed), bx, parent, true);
        Self::common(n, &mut el.decls);
        let p = Parent {
            flex: None,
            size: (bx.2, bx.3),
            offset: (-b.0, -b.1),
            root: false,
            phrasing: parent.phrasing,
        };
        let mut items = Vec::new();
        for c in children {
            self.node(c, p, &mut items);
        }
        if !items.is_empty() {
            el.body = Body::Children(items);
        }
        out.push(Item::El(el));
    }

    /// `(x0, y0, x1, y1)` of the visible children, in their parent's
    /// coordinates (groups count as their content).
    fn content_bounds(&self, children: &[Node]) -> (f64, f64, f64, f64) {
        let mut b: Option<(f64, f64, f64, f64)> = None;
        for c in children.iter().filter(|c| c.visible) {
            let r = match &c.kind {
                NodeKind::Group(g) => {
                    let r = self.content_bounds(&g.children);
                    (
                        r.0 + c.x as f64,
                        r.1 + c.y as f64,
                        r.2 + c.x as f64,
                        r.3 + c.y as f64,
                    )
                }
                _ => {
                    let (w, h) = self.size_of(c);
                    (c.x as f64, c.y as f64, c.x as f64 + w, c.y as f64 + h)
                }
            };
            b = Some(match b {
                None => r,
                Some(b) => (b.0.min(r.0), b.1.min(r.1), b.2.max(r.2), b.3.max(r.3)),
            });
        }
        b.unwrap_or_default()
    }

    fn instance(&mut self, n: &Node, inst: &Instance, parent: Parent<'_>, out: &mut Vec<Item>) {
        if self.stack.contains(&inst.component) {
            return; // a component cycle; validation rejects these
        }
        let Some(comp) = self.doc.components.iter().find(|c| c.id == inst.component) else {
            return;
        };
        let Some(shell) = expand_instance(self.doc, n) else {
            return;
        };
        let name = self.comp_names.get(&comp.id).cloned();
        if let Some(name) = name.filter(|_| self.calls && overrides_are_props(comp, inst)) {
            let flow = Self::in_flow(n, parent);
            let size = self.size_of(n);
            let bx = (
                parent.offset.0 + n.x as f64,
                parent.offset.1 + n.y as f64,
                size.0,
                size.1,
            );
            let mut place = self.place(&shell, effective_sizing(&shell, flow), bx, parent, true);
            Self::common(n, &mut place);
            let mut props = Vec::new();
            for (p, prop) in comp.props.iter().zip(prop_names(comp)) {
                match (p.kind, inst.overrides.get(p.target.as_str())) {
                    (PropKind::Text, Some(Override::Text { content })) => {
                        props.push((prop, PropVal::Str(content.clone())));
                    }
                    (PropKind::Visible, Some(Override::Visible { visible })) => {
                        props.push((prop, PropVal::Bool(*visible)));
                    }
                    (PropKind::Swap, Some(Override::Swap { component })) => {
                        let items = self.swap_value(comp, &p.target, component);
                        props.push((prop, PropVal::Node(items)));
                    }
                    _ => {}
                }
            }
            self.used.insert(name.clone());
            out.push(Item::Call(Call {
                component: name,
                props,
                place,
                cond: None,
            }));
            return;
        }
        self.stack.push(comp.id.clone());
        self.node_inner(&shell, parent, out);
        self.stack.pop();
    }

    /// The content of a swap prop: the nested instance `target` of `comp`
    /// showing `swapped`, placed where the master places it.
    fn swap_value(
        &mut self,
        comp: &Component,
        target: &NodeId,
        swapped: &ComponentId,
    ) -> Vec<Item> {
        let Some((p, t)) = find_with_parent(&comp.root, target) else {
            return Vec::new();
        };
        let NodeKind::Instance(i) = &t.kind else {
            return Vec::new();
        };
        let mut node = t.clone();
        node.kind = NodeKind::Instance(Instance {
            component: swapped.clone(),
            ..i.clone()
        });
        let flex = match &p.kind {
            NodeKind::Frame(f) => f.layout.as_ref(),
            _ => None,
        };
        let parent = Parent {
            flex,
            size: self.size_of(p),
            offset: (0.0, 0.0),
            root: false,
            phrasing: false,
        };
        let mut out = Vec::new();
        self.stack.push(comp.id.clone());
        self.node_inner(&node, parent, &mut out);
        self.stack.pop();
        out
    }

    // -----------------------------------------------------------------------
    // Placement (§3.5)

    /// `bx` is the node's box `(x, y, width, height)` in its parent.
    fn place(
        &self,
        n: &Node,
        (sw, sh): (Sizing, Sizing),
        bx: (f64, f64, f64, f64),
        parent: Parent<'_>,
        container: bool,
    ) -> Vec<Decl> {
        let (x, y, w, h) = bx;
        let mut d = Vec::new();
        if parent.root {
            d.push(Decl::lit("position", "relative"));
            match sw {
                Sizing::Hug => d.push(Decl::lit("width", "fit-content")),
                _ if self.opts.responsive => {
                    d.push(Decl::lit("width", "100%"));
                    d.push(Decl::lit("max-width", px(w)));
                }
                _ => d.push(Decl::lit("width", px(w))),
            }
            match sh {
                Sizing::Hug => {}
                _ if self.opts.responsive => d.push(Decl::lit("min-height", px(h))),
                _ => d.push(Decl::lit("height", px(h))),
            }
            return d;
        }
        if let Some(al) = parent.flex.filter(|_| Self::in_flow(n, parent)) {
            if container {
                d.push(Decl::lit("position", "relative"));
            }
            if sw == Sizing::Fixed {
                d.push(Decl::lit("width", px(w)));
            }
            if sh == Sizing::Fixed {
                d.push(Decl::lit("height", px(h)));
            }
            let (main, cross) = match al.direction {
                Direction::Row => (sw, sh),
                Direction::Column => (sh, sw),
            };
            if main == Sizing::Fill {
                d.push(Decl::lit("flex", "1 1 0"));
            } else {
                d.push(Decl::lit("flex-shrink", "0"));
            }
            if cross == Sizing::Fill {
                d.push(Decl::lit("align-self", "stretch"));
            } else if let Some(a) = n.layout_child.align_self {
                d.push(Decl::lit("align-self", align(a)));
            }
            let lc = &n.layout_child;
            for (prop, v) in [
                ("min-width", lc.min_width),
                ("max-width", lc.max_width),
                ("min-height", lc.min_height),
                ("max-height", lc.max_height),
            ] {
                if let Some(v) = v {
                    d.push(Decl::lit(prop, px(v.max(0.0) as f64)));
                }
            }
            return d;
        }
        d.push(Decl::lit("position", "absolute"));
        let (pw, ph) = parent.size;
        let h_axis = match n.constraints.horizontal {
            HConstraint::Left => Axis::Start,
            HConstraint::Right => Axis::End,
            HConstraint::LeftRight => Axis::Both,
            HConstraint::Center => Axis::Center,
            HConstraint::Scale => Axis::Scale,
        };
        let v_axis = match n.constraints.vertical {
            VConstraint::Top => Axis::Start,
            VConstraint::Bottom => Axis::End,
            VConstraint::TopBottom => Axis::Both,
            VConstraint::Center => Axis::Center,
            VConstraint::Scale => Axis::Scale,
        };
        constrain(&mut d, h_axis, ["left", "right", "width"], x, w, pw, sw);
        constrain(&mut d, v_axis, ["top", "bottom", "height"], y, h, ph, sh);
        d
    }

    fn flex_decls(&self, al: &AutoLayout, d: &mut Vec<Decl>) {
        d.push(Decl::lit("display", "flex"));
        if al.direction == Direction::Column {
            d.push(Decl::lit("flex-direction", "column"));
        }
        if al.wrap {
            d.push(Decl::lit("flex-wrap", "wrap"));
        }
        let gap = self.space(&al.gap);
        let zero = |v: &Val| v.as_lit() == Some("0");
        let cross = al.cross_gap.as_ref().map(|c| self.space(c));
        match cross {
            Some(c) if c != gap => {
                let (main_prop, cross_prop) = match al.direction {
                    Direction::Row => ("column-gap", "row-gap"),
                    Direction::Column => ("row-gap", "column-gap"),
                };
                if !zero(&gap) {
                    d.push(Decl::new(main_prop, gap));
                }
                if !zero(&c) && al.wrap {
                    d.push(Decl::new(cross_prop, c));
                }
            }
            _ => {
                if !zero(&gap) {
                    d.push(Decl::new("gap", gap));
                }
            }
        }
        let pad: Vec<Val> = al.padding.iter().map(|s| self.space(s)).collect();
        if !pad.iter().all(zero) {
            for (prop, v) in [
                "padding-top",
                "padding-right",
                "padding-bottom",
                "padding-left",
            ]
            .into_iter()
            .zip(pad)
            {
                d.push(Decl::new(prop, v));
            }
        }
        match al.justify {
            Justify::Start => {}
            Justify::Center => d.push(Decl::lit("justify-content", "center")),
            Justify::End => d.push(Decl::lit("justify-content", "flex-end")),
            Justify::SpaceBetween => d.push(Decl::lit("justify-content", "space-between")),
        }
        if al.align != Align::Stretch {
            d.push(Decl::lit("align-items", align(al.align)));
        }
        if al.wrap {
            d.push(Decl::lit("align-content", "flex-start"));
        }
    }

    // -----------------------------------------------------------------------
    // Paint

    fn stops(&self, stops: &[Stop], at: impl Fn(f64) -> f64) -> Val {
        let parts: Vec<Val> = stops
            .iter()
            .map(|s| {
                let mut v = self.color(&s.color);
                v.push(&format!(" {}%", num(at(s.offset as f64) * 100.0)));
                v
            })
            .collect();
        Val::join(&parts, ", ")
    }

    /// A gradient paint as a CSS `<image>` for a box of `size`.
    fn gradient(&self, p: &Paint, size: (f64, f64)) -> Option<Val> {
        let (w, h) = (size.0.max(1e-6), size.1.max(1e-6));
        match p {
            Paint::LinearGradient(g) => {
                // CSS draws along an angle through the center, over a line
                // whose length depends on the box; map the design's start and
                // end points onto that line.
                let (x0, y0) = (g.start[0] as f64 * w, g.start[1] as f64 * h);
                let (x1, y1) = (g.end[0] as f64 * w, g.end[1] as f64 * h);
                let (dx, dy) = (x1 - x0, y1 - y0);
                let theta = if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
                    std::f64::consts::PI
                } else {
                    dx.atan2(-dy)
                };
                let (ux, uy) = (theta.sin(), -theta.cos());
                let len = (w * theta.sin()).abs() + (h * theta.cos()).abs();
                let t = |x: f64, y: f64| ((x - w / 2.0) * ux + (y - h / 2.0) * uy) / len + 0.5;
                let (t0, t1) = (t(x0, y0), t(x1, y1));
                let deg = theta.to_degrees().rem_euclid(360.0);
                let mut v = Val::lit(format!("linear-gradient({}deg, ", num(deg)));
                v.append(&self.stops(&g.stops, |s| t0 + s * (t1 - t0)))
                    .push(")");
                Some(v)
            }
            Paint::RadialGradient(g) => {
                let mut v = Val::lit(format!(
                    "radial-gradient({} {} at {}% {}%, ",
                    px((g.radius[0] as f64 * w).max(0.01)),
                    px((g.radius[1] as f64 * h).max(0.01)),
                    num(g.center[0] as f64 * 100.0),
                    num(g.center[1] as f64 * 100.0)
                ));
                v.append(&self.stops(&g.stops, |s| s)).push(")");
                Some(v)
            }
            _ => None,
        }
    }

    fn image_layer(&mut self, asset: &AssetRef, fit: ImageFit) -> Val {
        let src = self.image_src(asset);
        Val::lit(format!(
            "url(\"{src}\") {}",
            match fit {
                ImageFit::Fill => "center / 100% 100% no-repeat",
                ImageFit::Fit => "center / contain no-repeat",
                ImageFit::Crop => "center / cover no-repeat",
                ImageFit::Tile => "0 0 / auto repeat",
            }
        ))
    }

    /// Fills, painted bottom-to-top, as CSS backgrounds (listed top first).
    fn background(&mut self, fills: &[Paint], size: (f64, f64), d: &mut Vec<Decl>) {
        if let [Paint::Solid(s)] = fills {
            d.push(Decl::new("background-color", self.color(&s.color)));
            return;
        }
        let mut layers = Vec::new();
        for (i, p) in fills.iter().enumerate().rev() {
            let layer = match p {
                // Only the bottom layer may be a plain color.
                Paint::Solid(s) if i == 0 => self.color(&s.color),
                Paint::Solid(s) => {
                    let c = self.color(&s.color);
                    let mut v = Val::lit("linear-gradient(");
                    v.append(&c).push(", ").append(&c).push(")");
                    v
                }
                Paint::Image(ip) => self.image_layer(&ip.asset, ip.fit),
                g => match self.gradient(g, size) {
                    Some(v) => v,
                    None => continue,
                },
            };
            layers.push(layer);
        }
        if !layers.is_empty() {
            d.push(Decl::new("background", Val::join(&layers, ", ")));
        }
    }

    /// Text paints its topmost fill: a color, or a gradient/image clipped
    /// to the glyphs.
    fn text_fill(&mut self, fills: &[Paint], size: (f64, f64), d: &mut Vec<Decl>) {
        let layer = match fills.last() {
            None => {
                d.push(Decl::lit("color", "transparent"));
                return;
            }
            Some(Paint::Solid(s)) => {
                d.push(Decl::new("color", self.color(&s.color)));
                return;
            }
            Some(Paint::Image(ip)) => self.image_layer(&ip.asset, ip.fit),
            Some(g) => self.gradient(g, size).unwrap_or_default(),
        };
        d.push(Decl::new("background", layer));
        d.push(Decl::lit("-webkit-background-clip", "text"));
        d.push(Decl::lit("background-clip", "text"));
        d.push(Decl::lit("color", "transparent"));
    }

    /// One color for a paint where CSS takes only a color: a gradient's
    /// first stop.
    fn flat_color(&self, p: &Paint) -> Option<Val> {
        match p {
            Paint::Solid(s) => Some(self.color(&s.color)),
            Paint::LinearGradient(g) => g.stops.first().map(|s| self.color(&s.color)),
            Paint::RadialGradient(g) => g.stops.first().map(|s| self.color(&s.color)),
            Paint::Image(_) => None,
        }
    }

    fn stroke(&self, n: &Node, container: bool, d: &mut Vec<Decl>) {
        let w = effective_stroke_width(self.doc, n) as f64;
        let Some(color) = effective_strokes(self.doc, n)
            .last()
            .and_then(|p| self.flat_color(p))
            .filter(|_| w > 0.0)
        else {
            return;
        };
        let align = n.shape().map_or(StrokeAlign::Inside, |s| s.stroke_align);
        if align == StrokeAlign::Inside && !container {
            d.push(Decl::lit("border-width", px(w)));
            d.push(Decl::lit("border-style", "solid"));
            d.push(Decl::new("border-color", color));
            return;
        }
        let mut v = Val::lit(format!("{} solid ", px(w)));
        v.append(&color);
        d.push(Decl::new("outline", v));
        d.push(Decl::lit(
            "outline-offset",
            px(match align {
                StrokeAlign::Inside => -w,
                StrokeAlign::Center => -w / 2.0,
                StrokeAlign::Outside => 0.0,
            }),
        ));
    }

    fn corner_radius(&self, r: &CornerRadius) -> Option<Val> {
        let corners = r.corners();
        let vals: Vec<Val> = corners.iter().map(|c| self.radius(c)).collect();
        if vals.iter().all(|v| v.as_lit() == Some("0")) {
            return None;
        }
        if corners.iter().all(|c| *c == corners[0]) {
            return vals.into_iter().next();
        }
        Some(Val::join(&vals, " "))
    }

    fn box_shadow(&self, n: &Node, d: &mut Vec<Decl>) {
        let shadows = effective_shadows(self.doc, n);
        if shadows.is_empty() {
            return;
        }
        // CSS lists the topmost shadow first.
        let parts: Vec<Val> = shadows
            .iter()
            .rev()
            .map(|s| {
                let mut v = Val::lit(format!(
                    "{} {} {} {} ",
                    px(s.x as f64),
                    px(s.y as f64),
                    px(s.blur.max(0.0) as f64),
                    px(s.spread as f64)
                ));
                v.append(&self.color(&s.color));
                v
            })
            .collect();
        d.push(Decl::new("box-shadow", Val::join(&parts, ", ")));
    }

    /// The declarations of a text style (also the body of its class).
    pub fn font_decls(&self, s: &TextStyle) -> Vec<Decl> {
        let mut d = vec![
            Decl::new("font-family", self.font(&s.font_family)),
            Decl::lit("font-size", px(s.font_size as f64)),
            Decl::lit("font-weight", s.font_weight.to_string()),
        ];
        if s.italic {
            d.push(Decl::lit("font-style", "italic"));
        }
        d.push(Decl::lit(
            "line-height",
            match s.line_height {
                LineHeight::Auto => "normal".to_string(),
                LineHeight::Px(v) => px(v as f64),
                LineHeight::Percent(p) => format!("{}%", num(p as f64)),
            },
        ));
        if num(s.letter_spacing as f64) != "0" {
            d.push(Decl::lit("letter-spacing", px(s.letter_spacing as f64)));
        }
        match s.text_transform {
            TextTransform::None => {}
            TextTransform::Upper => d.push(Decl::lit("text-transform", "uppercase")),
            TextTransform::Lower => d.push(Decl::lit("text-transform", "lowercase")),
            TextTransform::Title => d.push(Decl::lit("text-transform", "capitalize")),
        }
        match s.decoration {
            TextDecoration::None => {}
            TextDecoration::Underline => d.push(Decl::lit("text-decoration", "underline")),
            TextDecoration::Strike => d.push(Decl::lit("text-decoration", "line-through")),
        }
        d
    }
}

fn align(a: Align) -> &'static str {
    match a {
        Align::Start => "flex-start",
        Align::Center => "center",
        Align::End => "flex-end",
        Align::Stretch => "stretch",
        Align::Baseline => "baseline",
    }
}

#[derive(Clone, Copy)]
enum Axis {
    Start,
    End,
    Both,
    Center,
    Scale,
}

/// One axis of an absolute child: `[start, end, size]` property names.
fn constrain(
    d: &mut Vec<Decl>,
    axis: Axis,
    [start, end, size]: [&'static str; 3],
    pos: f64,
    len: f64,
    parent: f64,
    sizing: Sizing,
) {
    let fixed = sizing != Sizing::Hug;
    let axis = match axis {
        Axis::Scale if parent <= 0.0 => Axis::Start,
        a => a,
    };
    match axis {
        Axis::Start => d.push(Decl::lit(start, px(pos))),
        Axis::End => d.push(Decl::lit(end, px(parent - pos - len))),
        Axis::Both => {
            d.push(Decl::lit(start, px(pos)));
            d.push(Decl::lit(end, px(parent - pos - len)));
            return;
        }
        Axis::Center => {
            let off = pos - parent / 2.0;
            let v = match px(off.abs()).as_str() {
                "0" => "50%".to_string(),
                a if off < 0.0 => format!("calc(50% - {a})"),
                a => format!("calc(50% + {a})"),
            };
            d.push(Decl::lit(start, v));
        }
        Axis::Scale => {
            let pct = |v: f64| match num(v / parent * 100.0).as_str() {
                "0" => "0".to_string(),
                p => format!("{p}%"),
            };
            d.push(Decl::lit(start, pct(pos)));
            d.push(Decl::lit(size, pct(len)));
            return;
        }
    }
    if fixed {
        d.push(Decl::lit(size, px(len)));
    }
}

/// Whether every override of an instance is a prop of its component, so
/// the instance can be a component call.
fn overrides_are_props(comp: &Component, inst: &Instance) -> bool {
    inst.overrides.iter().all(|(key, o)| {
        comp.props.iter().any(|p| {
            p.target.as_str() == key
                && matches!(
                    (p.kind, o),
                    (PropKind::Text, Override::Text { .. })
                        | (PropKind::Visible, Override::Visible { .. })
                        | (PropKind::Swap, Override::Swap { .. })
                )
        })
    })
}

/// The React prop names of a component's props, distinct, in order.
pub(crate) fn prop_names(comp: &Component) -> Vec<String> {
    let mut names = Unique::default();
    names.take("className", "");
    comp.props
        .iter()
        .map(|p| names.take(&super::names::camel(&p.name, "prop"), ""))
        .collect()
}

/// `(parent, node)` for the node `id` below `root`.
fn find_with_parent<'n>(root: &'n Node, id: &NodeId) -> Option<(&'n Node, &'n Node)> {
    for c in root.children()? {
        if &c.id == id {
            return Some((root, c));
        }
        if let Some(found) = find_with_parent(c, id) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_names_keep_only_safe_characters() {
        assert_eq!(family_lit("Inter"), "'Inter', sans-serif");
        assert_eq!(
            family_lit("Source Sans-3_x.y"),
            "'Source Sans-3_x.y', sans-serif"
        );
        for hostile in [
            "x`+alert(1)+`",
            "a${b}",
            "a</style><script>",
            "a\nb\r\u{2028}c",
            "a'b\"c\\d;{e}",
            "a/*b*/c",
        ] {
            let lit = family_lit(hostile);
            let inner = lit
                .strip_prefix('\'')
                .unwrap()
                .strip_suffix("', sans-serif")
                .unwrap();
            assert!(
                inner
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || " -_.".contains(c)),
                "{hostile:?} -> {lit:?}"
            );
        }
        assert_eq!(family_lit("x`+alert(1)+`"), "'xalert1', sans-serif");
    }
}
