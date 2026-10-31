//! The layer map written beside an exported image: every visible layer of
//! the exported node with its box in the image's pixels, back to front. A
//! phone shows the image and, from a tap, finds the topmost layer under the
//! finger (the last one listed whose box holds the point) to edit it.

use doc::{Document, Node, NodeId, NodeKind, Paint};
use layout::Geometry;
use serde::Serialize;

/// The format of the layer map; bumped when a field changes meaning.
pub const LAYER_MAP_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayerMap {
    pub version: u32,
    /// The exported node.
    pub node: String,
    /// The image's size in pixels.
    pub width: u32,
    pub height: u32,
    /// Image pixels per document px.
    pub scale: f64,
    /// Back to front: the exported node first, then its subtree in paint order.
    pub layers: Vec<Layer>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    /// The containing layer, `None` for the exported node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The layer's axis-aligned box in image pixels.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// A text layer's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The image asset an image layer, or a layer filled with an image, shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

/// The layer map of `root` rendered at `scale` into a `width` x `height` image.
pub fn layer_map(
    doc: &Document,
    geom: &Geometry,
    root: &NodeId,
    scale: f64,
    width: u32,
    height: u32,
) -> LayerMap {
    let mut layers = Vec::new();
    let origin = geom.get(root).map(|g| (g.bounds.x0, g.bounds.y0));
    let node = doc
        .node(root)
        .cloned()
        .or_else(|| doc::shown_node(doc, root));
    if let (Some(node), Some(origin)) = (node, origin) {
        walk(geom, &node, None, origin, scale, &mut layers);
    }
    LayerMap {
        version: LAYER_MAP_VERSION,
        node: root.to_string(),
        width,
        height,
        scale,
        layers,
    }
}

fn walk(
    geom: &Geometry,
    node: &Node,
    parent: Option<&NodeId>,
    origin: (f64, f64),
    scale: f64,
    out: &mut Vec<Layer>,
) {
    if !node.visible {
        return;
    }
    if let Some(g) = geom.get(&node.id) {
        let b = g.bounds;
        out.push(Layer {
            id: node.id.to_string(),
            name: node.name.clone(),
            kind: node.kind.tag(),
            parent: parent.map(|p| p.to_string()),
            x: round2((b.x0 - origin.0) * scale),
            y: round2((b.y0 - origin.1) * scale),
            width: round2(b.width() * scale),
            height: round2(b.height() * scale),
            text: match &node.kind {
                NodeKind::Text(t) => Some(t.content.clone()),
                _ => None,
            },
            image: image_of(node),
            locked: node.locked,
        });
    }
    // An instance's children are the component's, laid out as the instance.
    let children = match &node.kind {
        NodeKind::Instance(_) => geom.expanded(&node.id).and_then(Node::children),
        _ => node.children(),
    };
    for child in children.into_iter().flatten() {
        walk(geom, child, Some(&node.id), origin, scale, out);
    }
}

fn image_of(node: &Node) -> Option<String> {
    let fills = match &node.kind {
        NodeKind::Image(i) => return Some(i.asset.0.clone()),
        NodeKind::Frame(f) => &f.shape.fills,
        NodeKind::Rect(r) => &r.shape.fills,
        NodeKind::Ellipse(e) => &e.shape.fills,
        _ => return None,
    };
    fills.iter().rev().find_map(|p| match p {
        Paint::Image(i) => Some(i.asset.0.clone()),
        _ => None,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout::LayoutEngine;

    const DOC: &str = r##"{
      "format": { "name": "tessel", "version": 1 },
      "pages": [{ "id": "p_page000001", "name": "Page", "children": [
        { "id": "n_post000001", "name": "Post", "kind": "frame",
          "x": 100, "y": 50, "width": 200, "height": 250,
          "fills": [{ "type": "solid", "color": "#ffffff" }],
          "children": [
            { "id": "n_photo00001", "name": "Photo", "kind": "rect",
              "x": 0, "y": 0, "width": 200, "height": 150 },
            { "id": "n_title00001", "name": "Headline", "kind": "text",
              "x": 10, "y": 160, "width": 180, "height": 30,
              "content": "Fall sale", "sizing": "fixed" },
            { "id": "n_hidden0001", "name": "Draft", "kind": "rect", "visible": false,
              "x": 0, "y": 0, "width": 10, "height": 10 }
          ]}
      ]}]
    }"##;

    #[test]
    fn lists_visible_layers_back_to_front_in_image_pixels() {
        let doc = doc::load_str(DOC).unwrap().doc;
        let geom = LayoutEngine::new().compute(&doc);
        let map = layer_map(
            &doc,
            &geom,
            &NodeId::from_raw("n_post000001"),
            2.0,
            400,
            500,
        );
        let names: Vec<_> = map.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            ["Post", "Photo", "Headline"],
            "the hidden layer is left out"
        );
        let post = &map.layers[0];
        assert_eq!(
            (post.x, post.y, post.width, post.height),
            (0.0, 0.0, 400.0, 500.0)
        );
        assert_eq!(post.parent, None);
        let title = &map.layers[2];
        assert_eq!(
            (title.x, title.y, title.width, title.height),
            (20.0, 320.0, 360.0, 60.0)
        );
        assert_eq!(title.text.as_deref(), Some("Fall sale"));
        assert_eq!(title.parent.as_deref(), Some("n_post000001"));
        let json = serde_json::to_value(&map).unwrap();
        assert_eq!(json["version"], 1);
        assert!(
            json["layers"][1].get("text").is_none(),
            "only text layers carry text"
        );
    }
}
