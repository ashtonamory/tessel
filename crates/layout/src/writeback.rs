//! Size write-back (§3.5): nodes whose size comes from layout store the
//! size the user sees, written only when the rounded value changes so the
//! file stays stable.

use doc::{Document, Node, NodeId, NodeKind, TextSizing};

use crate::flex::in_flow;
use crate::geometry::Geometry;

/// A computed size to store in a node's `width`/`height`.
#[derive(Clone, Debug, PartialEq)]
pub struct SizeWrite {
    pub id: NodeId,
    pub width: f32,
    pub height: f32,
}

/// The nodes whose computed size, rounded as the file rounds (0.01 px),
/// differs from their stored, rounded `width`/`height`: auto-layout frames,
/// instances of auto-layout components, flow children of auto-layout
/// frames, and text with automatic sizing.
/// Groups are skipped (their size is always derived). In document order.
pub fn size_writeback(doc: &Document, geometry: &Geometry) -> Vec<SizeWrite> {
    let mut out = Vec::new();
    for page in &doc.pages {
        for n in &page.children {
            visit(n, false, geometry, &mut out);
        }
    }
    out
}

fn round(v: f64) -> f32 {
    ((v * 100.0).round() / 100.0) as f32
}

fn visit(node: &Node, placed: bool, geometry: &Geometry, out: &mut Vec<SizeWrite>) {
    // An instance sizes as the frame it stands for; the nodes it shows are
    // derived, so nothing below it is stored.
    let auto_layout =
        matches!(&geometry.shown(node).kind, NodeKind::Frame(f) if f.layout.is_some());
    let sized = match &node.kind {
        NodeKind::Group(_) => false,
        NodeKind::Text(t) => placed || t.sizing != TextSizing::Fixed,
        _ => placed || auto_layout,
    };
    if let (true, Some(g)) = (sized, geometry.get(&node.id)) {
        let (w, h) = (round(g.size.width), round(g.size.height));
        if w != round(node.width as f64) || h != round(node.height as f64) {
            out.push(SizeWrite {
                id: node.id.clone(),
                width: w,
                height: h,
            });
        }
    }
    if let Some(children) = node.children() {
        for c in children {
            visit(c, auto_layout && in_flow(c), geometry, out);
        }
    }
}
