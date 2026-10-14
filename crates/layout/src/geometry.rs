//! `Geometry`: absolute transform and bounds of every node (§7.3).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use doc::{AutoLayout, Document, Node, NodeId, NodeKind, Page, Sizing, TextSizing, VAlign};
use kurbo::{Affine, Point, Rect, Shape, Size, Vec2};

use crate::flex::{in_flow, node_style, text_leaf, text_wraps, FlexTree};
use crate::text::{effective_text_style, resolve_family, TextLayout, TextMeasurer};

/// Where a node is on its page.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeGeometry {
    /// Maps the node's local box `(0,0)..(width,height)` to page coordinates
    /// (parent offsets, then rotation about the node's center).
    pub transform: Affine,
    /// The node's computed size. Text with automatic sizing is measured.
    pub size: Size,
    /// Axis-aligned bounds of the transformed box, in page coordinates.
    pub bounds: Rect,
    /// The containing node, or `None` for page-level nodes.
    pub parent: Option<NodeId>,
}

impl NodeGeometry {
    /// The node's box in its own coordinates.
    pub fn local_rect(&self) -> Rect {
        Rect::from_origin_size(Point::ZERO, self.size)
    }

    /// A page point in the node's local coordinates.
    pub fn to_local(&self, p: Point) -> Point {
        self.transform.inverse() * p
    }

    /// The page position of the node's local origin (its top-left corner
    /// before rotation).
    pub fn origin(&self) -> Point {
        self.transform * Point::ZERO
    }
}

/// A shaped text node: its layout and where the layout's top-left sits in
/// the node's local box (vertical alignment of fixed-height text).
#[derive(Clone)]
pub struct TextGeometry {
    pub layout: Arc<TextLayout>,
    pub offset: Vec2,
}

/// Geometry of every node of a document, including the nodes instances
/// show under derived IDs (§3.7).
#[derive(Clone, Default)]
pub struct Geometry {
    nodes: HashMap<NodeId, NodeGeometry>,
    text: HashMap<NodeId, TextGeometry>,
    /// The frame each instance was laid out as, by instance ID.
    expanded: HashMap<NodeId, Arc<Node>>,
}

impl Geometry {
    pub fn get(&self, id: &NodeId) -> Option<&NodeGeometry> {
        self.nodes.get(id)
    }

    pub fn text(&self, id: &NodeId) -> Option<&TextGeometry> {
        self.text.get(id)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// The frame an instance (on a page, or nested in another instance) was
    /// laid out as: its one-level expansion (`doc::expand_instance`), whose
    /// children have derived IDs. `None` for other nodes and for instances
    /// whose component is missing.
    pub fn expanded(&self, id: &NodeId) -> Option<&Node> {
        self.expanded.get(id).map(|n| &**n)
    }

    /// The node as it is shown: an instance's expansion, else the node.
    pub fn shown<'a>(&'a self, node: &'a Node) -> &'a Node {
        match &node.kind {
            NodeKind::Instance(_) => self.expanded(&node.id).unwrap_or(node),
            _ => node,
        }
    }

    /// Calls `f` on the shown form of `node` (see [`Geometry::shown`]) and
    /// of every node it shows, depth first, through instances.
    pub fn walk_shown<'a>(&'a self, node: &'a Node, f: &mut impl FnMut(&'a Node)) {
        let node = self.shown(node);
        f(node);
        if let Some(children) = node.children() {
            for c in children {
                self.walk_shown(c, f);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Union of the bounds of `ids` that have geometry.
    pub fn union_bounds<'a>(&self, ids: impl IntoIterator<Item = &'a NodeId>) -> Option<Rect> {
        ids.into_iter()
            .filter_map(|id| self.get(id))
            .map(|g| g.bounds)
            .reduce(|a, b| a.union(b))
    }

    /// Bounds of everything on a page.
    pub fn page_bounds(&self, page: &Page) -> Option<Rect> {
        self.union_bounds(page.children.iter().map(|n| &n.id))
    }
}

/// Computes `Geometry` from a document. Keeps the text measurer (and its
/// caches) and the auto-layout tree alive between passes.
#[derive(Default)]
pub struct LayoutEngine {
    pub text: TextMeasurer,
    pub(crate) flex: FlexTree,
    /// Instance expansions of the current pass, by instance ID.
    expansions: HashMap<NodeId, Arc<Node>>,
}

impl LayoutEngine {
    pub fn new() -> LayoutEngine {
        LayoutEngine::default()
    }

    /// Lay out every page of the document, re-syncing all auto-layout.
    pub fn compute(&mut self, doc: &Document) -> Geometry {
        self.run(doc, None)
    }

    /// Lay out after a change to the nodes in `touched` (§5.5): only the
    /// auto-layout subtrees containing them are re-synced, and taffy
    /// recomputes only what is dirty. Token and style changes (which touch
    /// no node) re-sync everything.
    pub fn update(&mut self, doc: &Document, touched: &BTreeSet<NodeId>) -> Geometry {
        self.run(doc, Some(touched))
    }

    fn run(&mut self, doc: &Document, touched: Option<&BTreeSet<NodeId>>) -> Geometry {
        let _span = tracing::trace_span!("layout").entered();
        self.flex.begin_pass(doc, touched);
        self.expansions.clear();
        let mut out = Geometry::default();
        for page in &doc.pages {
            for n in &page.children {
                self.visit(doc, n, Affine::IDENTITY, None, None, &mut out);
            }
        }
        self.flex.end_pass();
        self.text.end_pass();
        out
    }

    /// The size a text node takes under its sizing mode, measured with its
    /// current content. Used to store the visible size on commit (§3.5).
    pub fn measure_text(&mut self, doc: &Document, node: &Node) -> Option<Size> {
        let NodeKind::Text(_) = &node.kind else {
            return None;
        };
        Some(self.text_layout(doc, node, None).0)
    }

    /// Lay out a text node. `placed` is the size auto-layout gave it.
    fn text_layout(
        &mut self,
        doc: &Document,
        node: &Node,
        placed: Option<Size>,
    ) -> (Size, TextGeometry) {
        let NodeKind::Text(t) = &node.kind else {
            unreachable!("text_layout on a non-text node");
        };
        let style = effective_text_style(doc, node).expect("text node");
        let family = resolve_family(doc, style);
        let width = match placed {
            Some(s) => text_wraps(node).then_some(s.width as f32),
            None => match t.sizing {
                TextSizing::AutoWidth => None,
                TextSizing::AutoHeight | TextSizing::Fixed => Some(node.width.max(0.0)),
            },
        };
        let layout = self.text.layout(&t.content, &family, style, t.align, width);
        let measured = Size::new(layout.width() as f64, layout.height() as f64);
        let size = match (placed, t.sizing) {
            (Some(s), _) => s,
            (None, TextSizing::AutoWidth) => measured,
            (None, TextSizing::AutoHeight) => Size::new(node.width as f64, measured.height),
            (None, TextSizing::Fixed) => Size::new(node.width as f64, node.height as f64),
        };
        // Vertical alignment applies where the height is not the text's own.
        let fixed_height = t.sizing == TextSizing::Fixed
            || (placed.is_some() && node.layout_child.height == Sizing::Fill);
        let free = size.height - measured.height;
        let dy = match (fixed_height, t.vertical_align) {
            (true, VAlign::Middle) => free / 2.0,
            (true, VAlign::Bottom) => free,
            _ => 0.0,
        };
        (
            size,
            TextGeometry {
                layout,
                offset: Vec2::new(0.0, dy),
            },
        )
    }

    /// The frame an instance stands for, expanded once per pass.
    fn expansion(&mut self, doc: &Document, node: &Node) -> Option<Arc<Node>> {
        if !matches!(node.kind, NodeKind::Instance(_)) {
            return None;
        }
        if let Some(e) = self.expansions.get(&node.id) {
            return Some(e.clone());
        }
        let e = Arc::new(doc::expand_instance(doc, node)?);
        self.expansions.insert(node.id.clone(), e.clone());
        Some(e)
    }

    /// Sync (if needed) and lay out the auto-layout root `node`; its size.
    fn layout_root(&mut self, doc: &Document, node: &Node) -> Option<Size> {
        if self.flex.needs_sync(&node.id) {
            self.sync(doc, node, None, &node.id);
            self.flex.mark_synced(&node.id);
        }
        self.flex.compute(&node.id, &mut self.text)
    }

    /// Mirror `node` (and, for auto-layout frames, its flow children) into
    /// the taffy tree of `root`.
    fn sync(
        &mut self,
        doc: &Document,
        node: &Node,
        parent: Option<&AutoLayout>,
        root: &NodeId,
    ) -> taffy::NodeId {
        // An instance lays out as the frame it stands for.
        if let Some(e) = self.expansion(doc, node) {
            return self.sync(doc, &e, parent, root);
        }
        let group_size = match (&node.kind, parent) {
            (NodeKind::Group(_), Some(_)) => {
                let r = self.group_content(doc, node);
                self.flex.set_group_content(&node.id, r);
                Some(r.size())
            }
            _ => None,
        };
        let style = node_style(doc, node, parent, group_size);
        let leaf = text_leaf(doc, node);
        let children: Vec<taffy::NodeId> = match &node.kind {
            NodeKind::Frame(f) if f.layout.is_some() => f
                .children
                .iter()
                .filter(|c| in_flow(c))
                .map(|c| self.sync(doc, c, f.layout.as_ref(), root))
                .collect(),
            _ => Vec::new(),
        };
        self.flex.upsert(&node.id, root, style, leaf, &children)
    }

    /// Bounds of a group's children in the group's coordinates (before its
    /// own `x`/`y`).
    fn group_content(&mut self, doc: &Document, node: &Node) -> Rect {
        let mut scratch = Geometry::default();
        let children = node.children().map(Vec::as_slice).unwrap_or_default();
        for c in children {
            self.visit(doc, c, Affine::IDENTITY, None, None, &mut scratch);
        }
        scratch
            .union_bounds(children.iter().map(|c| &c.id))
            .unwrap_or(Rect::ZERO)
    }

    /// `placed` is the node's box in its parent's coordinates when an
    /// auto-layout parent placed it.
    fn visit(
        &mut self,
        doc: &Document,
        node: &Node,
        parent_tf: Affine,
        parent: Option<&NodeId>,
        placed: Option<Rect>,
        out: &mut Geometry,
    ) {
        // An instance is laid out as the frame it stands for; its children
        // get geometry under their derived IDs.
        if let Some(e) = self.expansion(doc, node) {
            self.visit(doc, &e, parent_tf, parent, placed, out);
            out.expanded.insert(node.id.clone(), e);
            return;
        }
        let auto_layout = matches!(&node.kind, NodeKind::Frame(f) if f.layout.is_some());
        let mut origin = Vec2::new(node.x as f64, node.y as f64);
        let mut size = Size::new(node.width.max(0.0) as f64, node.height.max(0.0) as f64);
        if let Some(r) = placed {
            origin = r.origin().to_vec2();
            size = r.size();
        } else if auto_layout {
            if let Some(s) = self.layout_root(doc, node) {
                size = s;
            }
        }
        if let NodeKind::Text(_) = node.kind {
            let (s, tg) = self.text_layout(doc, node, placed.map(|r| r.size()));
            size = s;
            out.text.insert(node.id.clone(), tg);
        }
        let center = Point::new(size.width / 2.0, size.height / 2.0);
        let tf = parent_tf
            * Affine::translate(origin)
            * Affine::rotate_about((node.rotation as f64).to_radians(), center);
        if let NodeKind::Group(g) = &node.kind {
            // A placed group is a leaf the size of its content: shift the
            // content so its top-left lands on the placed origin.
            let child_tf = match placed {
                Some(r) => {
                    let content = self.flex.group_content(&node.id);
                    parent_tf * Affine::translate(r.origin().to_vec2() - content.origin().to_vec2())
                }
                None => tf,
            };
            for c in &g.children {
                self.visit(doc, c, child_tf, Some(&node.id), None, out);
            }
        } else if let Some(children) = node.children() {
            for c in children {
                let placed = if auto_layout {
                    self.flex.placed(&node.id, &c.id)
                } else {
                    None
                };
                self.visit(doc, c, tf, Some(&node.id), placed, out);
            }
        }
        let geom = if let NodeKind::Group(g) = &node.kind {
            // A group's bounds are the union of its children (§3.4).
            let bounds = out
                .union_bounds(g.children.iter().map(|c| &c.id))
                .unwrap_or_else(|| Rect::from_origin_size(tf * Point::ZERO, Size::ZERO));
            NodeGeometry {
                transform: Affine::translate(bounds.origin().to_vec2()),
                size: bounds.size(),
                bounds,
                parent: parent.cloned(),
            }
        } else {
            let local = Rect::from_origin_size(Point::ZERO, size);
            NodeGeometry {
                transform: tf,
                size,
                bounds: (tf * local.to_path(0.1)).bounding_box(),
                parent: parent.cloned(),
            }
        };
        out.nodes.insert(node.id.clone(), geom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Frame, Group, Id, IdKind, Rect as RectKind, Text};

    fn id(n: u64) -> NodeId {
        Id::from_entropy(IdKind::Node, n)
    }

    fn rect(n: u64, x: f32, y: f32, w: f32, h: f32) -> Node {
        Node::new(id(n), "r", NodeKind::Rect(RectKind::default())).with_geometry(x, y, w, h)
    }

    fn doc_with(children: Vec<Node>) -> Document {
        let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
        page.children = children;
        Document::new("t", page)
    }

    fn approx(a: Rect, b: Rect) -> bool {
        (a.x0 - b.x0).abs() < 1e-3
            && (a.y0 - b.y0).abs() < 1e-3
            && (a.x1 - b.x1).abs() < 1e-3
            && (a.y1 - b.y1).abs() < 1e-3
    }

    #[test]
    fn nested_positions_are_absolute() {
        let mut frame = Node::new(id(1), "f", NodeKind::Frame(Frame::default()))
            .with_geometry(100.0, 50.0, 400.0, 300.0);
        frame
            .children_mut()
            .unwrap()
            .push(rect(2, 10.0, 20.0, 30.0, 40.0));
        let doc = doc_with(vec![frame]);
        let g = LayoutEngine::new().compute(&doc);
        assert!(approx(
            g.get(&id(1)).unwrap().bounds,
            Rect::new(100.0, 50.0, 500.0, 350.0)
        ));
        let child = g.get(&id(2)).unwrap();
        assert!(approx(child.bounds, Rect::new(110.0, 70.0, 140.0, 110.0)));
        assert_eq!(child.parent, Some(id(1)));
        assert_eq!(child.origin(), Point::new(110.0, 70.0));
    }

    #[test]
    fn rotation_is_about_the_center() {
        let mut r = rect(1, 0.0, 0.0, 100.0, 20.0);
        r.rotation = 90.0;
        let g = LayoutEngine::new().compute(&doc_with(vec![r]));
        let geom = g.get(&id(1)).unwrap();
        // A 100x20 box rotated a quarter turn about (50, 10).
        assert!(approx(geom.bounds, Rect::new(40.0, -40.0, 60.0, 60.0)));
        // Clockwise: the local top-left corner ends up at the top-right.
        let tl = geom.origin();
        assert!(
            (tl.x - 60.0).abs() < 1e-3 && (tl.y + 40.0).abs() < 1e-3,
            "{tl:?}"
        );
        let local = geom.to_local(Point::new(50.0, 10.0));
        assert!((local.x - 50.0).abs() < 1e-3 && (local.y - 10.0).abs() < 1e-3);
    }

    #[test]
    fn group_bounds_are_the_union_of_children() {
        let g = Node::new(
            id(1),
            "g",
            NodeKind::Group(Group {
                children: vec![
                    rect(2, 0.0, 0.0, 10.0, 10.0),
                    rect(3, 50.0, 40.0, 10.0, 10.0),
                ],
                ..Default::default()
            }),
        )
        .with_geometry(100.0, 100.0, 0.0, 0.0);
        let geom = LayoutEngine::new().compute(&doc_with(vec![g]));
        assert!(approx(
            geom.get(&id(1)).unwrap().bounds,
            Rect::new(100.0, 100.0, 160.0, 150.0)
        ));
    }

    fn text(n: u64, content: &str, sizing: TextSizing, w: f32, h: f32) -> Node {
        Node::new(
            id(n),
            "t",
            NodeKind::Text(Text {
                content: content.into(),
                sizing,
                ..Default::default()
            }),
        )
        .with_geometry(0.0, 0.0, w, h)
    }

    #[test]
    fn auto_width_text_is_measured() {
        let mut engine = LayoutEngine::new();
        let doc = doc_with(vec![
            text(1, "Hi", TextSizing::AutoWidth, 1.0, 1.0),
            text(2, "Hello, wide world", TextSizing::AutoWidth, 1.0, 1.0),
            text(3, "", TextSizing::AutoWidth, 1.0, 1.0),
        ]);
        let g = engine.compute(&doc);
        let short = g.get(&id(1)).unwrap().size;
        let long = g.get(&id(2)).unwrap().size;
        let empty = g.get(&id(3)).unwrap().size;
        if long.width == 0.0 {
            eprintln!("no system fonts; skipping text measurement checks");
            return;
        }
        assert!(long.width > short.width, "{long:?} vs {short:?}");
        assert!(short.height >= 16.0 && short.height < 40.0, "{short:?}");
        assert!((short.height - long.height).abs() < 1e-3);
        assert!(empty.height > 0.0, "empty text keeps a line height");
        assert!(g.text(&id(1)).is_some());
    }

    #[test]
    fn auto_height_text_wraps_at_its_width() {
        let mut engine = LayoutEngine::new();
        let content = "one two three four five six seven eight nine ten";
        let doc = doc_with(vec![
            text(1, content, TextSizing::AutoHeight, 60.0, 1.0),
            text(2, content, TextSizing::AutoWidth, 1.0, 1.0),
            text(3, content, TextSizing::Fixed, 60.0, 500.0),
        ]);
        let g = engine.compute(&doc);
        let wrapped = g.get(&id(1)).unwrap().size;
        let line = g.get(&id(2)).unwrap().size;
        if line.width == 0.0 {
            eprintln!("no system fonts; skipping text measurement checks");
            return;
        }
        assert_eq!(wrapped.width, 60.0);
        assert!(
            wrapped.height > line.height * 2.0,
            "{wrapped:?} vs {line:?}"
        );
        assert_eq!(g.get(&id(3)).unwrap().size, Size::new(60.0, 500.0));
        let n = doc.node(&id(1)).unwrap();
        assert_eq!(engine.measure_text(&doc, n), Some(wrapped));
    }
}
