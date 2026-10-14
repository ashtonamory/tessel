//! CPU hit-testing from `Geometry` (§7.5).

use doc::{Document, Node, NodeId, NodeKind, Page};
use kurbo::{Point, Rect};

use crate::geometry::{Geometry, NodeGeometry};

#[derive(Clone, Copy, Debug, Default)]
pub struct HitOptions<'a> {
    /// Slack around shapes, in page units (a few screen pixels / zoom).
    pub tolerance: f64,
    /// The current selection. A group that is selected or holds a selected
    /// node is "entered": clicks inside it reach its children.
    pub selected: &'a [NodeId],
    /// Select the deepest node, ignoring the group rule (Ctrl+click).
    pub deep: bool,
}

/// The node under `p`, front to back. Hidden and locked nodes (and their
/// subtrees) are skipped; clipping frames hide children outside their box;
/// a click inside an unentered group selects the outermost such group.
pub fn hit_test(geom: &Geometry, page: &Page, p: Point, opts: HitOptions<'_>) -> Option<NodeId> {
    page.children
        .iter()
        .rev()
        .find_map(|n| hit_node(geom, n, p, &opts))
}

fn hit_node(geom: &Geometry, node: &Node, p: Point, opts: &HitOptions<'_>) -> Option<NodeId> {
    if !node.visible || node.locked {
        return None;
    }
    let g = geom.get(&node.id)?;
    match &node.kind {
        NodeKind::Frame(f) => {
            let inside = contains_box(g, p, opts.tolerance);
            if inside || !f.clip {
                if let Some(hit) = f
                    .children
                    .iter()
                    .rev()
                    .find_map(|c| hit_node(geom, c, p, opts))
                {
                    return Some(hit);
                }
            }
            inside.then(|| node.id.clone())
        }
        NodeKind::Group(gr) => {
            let hit = gr
                .children
                .iter()
                .rev()
                .find_map(|c| hit_node(geom, c, p, opts))?;
            let entered = opts.deep || opts.selected.iter().any(|s| node.contains(s));
            Some(if entered { hit } else { node.id.clone() })
        }
        NodeKind::Instance(_) => {
            // Hit as the frame it stands for, selected whole until entered
            // (it or a node inside it is selected), like a group.
            let Some(shown) = geom.expanded(&node.id) else {
                return contains_box(g, p, opts.tolerance).then(|| node.id.clone());
            };
            let hit = hit_node(geom, shown, p, opts)?;
            Some(if opts.deep || entered_instance(&node.id, opts.selected) {
                hit
            } else {
                node.id.clone()
            })
        }
        NodeKind::Ellipse(_) => {
            let local = g.to_local(p);
            let (rx, ry) = (g.size.width / 2.0, g.size.height / 2.0);
            let (rx, ry) = (rx + opts.tolerance, ry + opts.tolerance);
            if rx <= 0.0 || ry <= 0.0 {
                return None;
            }
            let dx = (local.x - g.size.width / 2.0) / rx;
            let dy = (local.y - g.size.height / 2.0) / ry;
            (dx * dx + dy * dy <= 1.0).then(|| node.id.clone())
        }
        _ => contains_box(g, p, opts.tolerance).then(|| node.id.clone()),
    }
}

/// Whether the instance `id` is selected or holds a selected derived node.
fn entered_instance(id: &NodeId, selected: &[NodeId]) -> bool {
    selected.iter().any(|s| {
        s == id
            || s.as_str()
                .strip_prefix(id.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn contains_box(g: &NodeGeometry, p: Point, tolerance: f64) -> bool {
    g.local_rect()
        .inflate(tolerance, tolerance)
        .contains(g.to_local(p))
}

/// Nodes selected by a marquee: page-level nodes it touches, except that a
/// frame with children that the marquee touches but does not fully enclose
/// contributes the children it touches instead.
pub fn marquee_select(geom: &Geometry, page: &Page, rect: Rect) -> Vec<NodeId> {
    let mut out = Vec::new();
    for n in &page.children {
        marquee_node(geom, n, rect, &mut out);
    }
    out
}

fn marquee_node(geom: &Geometry, node: &Node, rect: Rect, out: &mut Vec<NodeId>) {
    if !node.visible || node.locked {
        return;
    }
    let Some(g) = geom.get(&node.id) else {
        return;
    };
    if !intersects(g.bounds, rect) {
        return;
    }
    if let NodeKind::Frame(f) = &node.kind {
        if !f.children.is_empty() && !encloses(rect, g.bounds) {
            for c in &f.children {
                marquee_node(geom, c, rect, out);
            }
            return;
        }
    }
    out.push(node.id.clone());
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

fn encloses(outer: Rect, inner: Rect) -> bool {
    outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
}

/// Nodes of `doc` that are ancestors of `id`, nearest first. For a node
/// inside an instance (a derived ID) that starts with the instances it is
/// in, innermost first.
pub fn ancestors(doc: &Document, id: &NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut s = id.as_str();
    while let Some((outer, _)) = s.rsplit_once('/') {
        out.push(doc::Id::from_raw(outer));
        s = outer;
    }
    let mut cur = doc::Id::from_raw(s);
    while let Some((doc::ParentRef::Node(p), _)) = doc.parent_of(&cur) {
        out.push(p.clone());
        cur = p;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LayoutEngine;
    use doc::{Ellipse, Frame, Group, Id, IdKind, Rect as RectKind};

    fn id(n: u64) -> NodeId {
        Id::from_entropy(IdKind::Node, n)
    }

    fn rect(n: u64, x: f32, y: f32, w: f32, h: f32) -> Node {
        Node::new(id(n), "r", NodeKind::Rect(RectKind::default())).with_geometry(x, y, w, h)
    }

    fn frame(n: u64, x: f32, y: f32, w: f32, h: f32, children: Vec<Node>) -> Node {
        Node::new(
            id(n),
            "f",
            NodeKind::Frame(Frame {
                children,
                ..Default::default()
            }),
        )
        .with_geometry(x, y, w, h)
    }

    fn setup(children: Vec<Node>) -> (Document, Geometry) {
        let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
        page.children = children;
        let doc = Document::new("t", page);
        let g = LayoutEngine::new().compute(&doc);
        (doc, g)
    }

    fn hit(doc: &Document, g: &Geometry, x: f64, y: f64) -> Option<NodeId> {
        hit_test(g, &doc.pages[0], Point::new(x, y), HitOptions::default())
    }

    #[test]
    fn front_most_wins() {
        let (doc, g) = setup(vec![
            rect(1, 0.0, 0.0, 100.0, 100.0),
            rect(2, 50.0, 50.0, 100.0, 100.0),
        ]);
        assert_eq!(hit(&doc, &g, 75.0, 75.0), Some(id(2)));
        assert_eq!(hit(&doc, &g, 25.0, 25.0), Some(id(1)));
        assert_eq!(hit(&doc, &g, 200.0, 25.0), None);
    }

    #[test]
    fn hidden_and_locked_are_skipped() {
        let mut a = rect(1, 0.0, 0.0, 100.0, 100.0);
        let mut b = rect(2, 0.0, 0.0, 100.0, 100.0);
        let c = rect(3, 0.0, 0.0, 100.0, 100.0);
        a.visible = true;
        b.locked = true;
        let mut d = c.clone();
        d.id = id(4);
        d.visible = false;
        let (doc, g) = setup(vec![a, b, d]);
        assert_eq!(hit(&doc, &g, 10.0, 10.0), Some(id(1)));
    }

    #[test]
    fn ellipse_corners_miss() {
        let e = Node::new(id(1), "e", NodeKind::Ellipse(Ellipse::default()))
            .with_geometry(0.0, 0.0, 100.0, 100.0);
        let (doc, g) = setup(vec![e]);
        assert_eq!(hit(&doc, &g, 50.0, 50.0), Some(id(1)));
        assert_eq!(hit(&doc, &g, 5.0, 5.0), None);
        let opts = HitOptions {
            tolerance: 3.0,
            ..Default::default()
        };
        assert_eq!(
            hit_test(&g, &doc.pages[0], Point::new(50.0, -2.0), opts),
            Some(id(1))
        );
    }

    #[test]
    fn frames_return_children_then_themselves_and_clip() {
        let f = frame(
            1,
            100.0,
            100.0,
            200.0,
            200.0,
            vec![
                rect(2, 10.0, 10.0, 50.0, 50.0),
                rect(3, 180.0, 0.0, 100.0, 50.0),
            ],
        );
        let (doc, g) = setup(vec![f]);
        assert_eq!(hit(&doc, &g, 120.0, 120.0), Some(id(2)));
        assert_eq!(hit(&doc, &g, 250.0, 250.0), Some(id(1)));
        // Child 3 pokes out of the clipping frame; the outside part is not hittable.
        assert_eq!(hit(&doc, &g, 290.0, 110.0), Some(id(3)));
        assert_eq!(hit(&doc, &g, 350.0, 110.0), None);
    }

    #[test]
    fn rotated_nodes_hit_their_rotated_box() {
        let mut r = rect(1, 0.0, 0.0, 100.0, 20.0);
        r.rotation = 90.0;
        let (doc, g) = setup(vec![r]);
        assert_eq!(hit(&doc, &g, 50.0, -30.0), Some(id(1)));
        assert_eq!(hit(&doc, &g, 90.0, 10.0), None);
    }

    #[test]
    fn groups_are_selected_whole_until_entered() {
        let grp = Node::new(
            id(1),
            "g",
            NodeKind::Group(Group {
                children: vec![
                    rect(2, 0.0, 0.0, 10.0, 10.0),
                    rect(3, 50.0, 0.0, 10.0, 10.0),
                ],
                ..Default::default()
            }),
        );
        let (doc, g) = setup(vec![grp]);
        assert_eq!(hit(&doc, &g, 5.0, 5.0), Some(id(1)));
        // Between the children there is nothing to hit.
        assert_eq!(hit(&doc, &g, 30.0, 5.0), None);
        let sel = [id(1)];
        let opts = HitOptions {
            selected: &sel,
            ..Default::default()
        };
        assert_eq!(
            hit_test(&g, &doc.pages[0], Point::new(55.0, 5.0), opts),
            Some(id(3))
        );
        let deep = HitOptions {
            deep: true,
            ..Default::default()
        };
        assert_eq!(
            hit_test(&g, &doc.pages[0], Point::new(5.0, 5.0), deep),
            Some(id(2))
        );
        assert_eq!(ancestors(&doc, &id(2)), vec![id(1)]);
    }

    #[test]
    fn marquee_selects_touching_nodes_and_enters_frames() {
        let f = frame(
            1,
            100.0,
            0.0,
            200.0,
            200.0,
            vec![
                rect(2, 10.0, 10.0, 20.0, 20.0),
                rect(3, 150.0, 150.0, 20.0, 20.0),
            ],
        );
        let (doc, g) = setup(vec![rect(4, 0.0, 0.0, 50.0, 50.0), f]);
        let page = &doc.pages[0];
        assert_eq!(
            marquee_select(&g, page, Rect::new(-10.0, -10.0, 20.0, 20.0)),
            vec![id(4)]
        );
        // Touches the frame without enclosing it: picks the child it touches.
        assert_eq!(
            marquee_select(&g, page, Rect::new(40.0, 0.0, 120.0, 20.0)),
            vec![id(4), id(2)]
        );
        // Encloses the frame: picks the frame.
        assert_eq!(
            marquee_select(&g, page, Rect::new(90.0, -10.0, 400.0, 400.0)),
            vec![id(1)]
        );
        assert!(marquee_select(&g, page, Rect::new(500.0, 500.0, 600.0, 600.0)).is_empty());
    }
}
