//! Node JSON for tool results and arguments: file-format JSON (§6.2), tree
//! summaries below a depth, and node specs turned into subtrees with fresh
//! IDs.

use std::collections::{BTreeMap, HashMap};

use doc::{Id, IdKind, Node, NodeId, NodePatch};
use layout::Geometry;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::error::ToolError;

/// `v` as it is written to a file (§4.3: numbers rounded to 2 decimals).
pub fn file_json<T: Serialize + ?Sized>(v: &T) -> Value {
    let s = doc::to_canonical_string(v).expect("document values serialize");
    serde_json::from_str(&s).expect("canonical JSON parses")
}

/// Round to the 2 decimals the file format uses.
pub fn round2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// The size the user sees: computed if laid out, else stored.
fn size(node: &Node, geometry: &Geometry) -> (f64, f64) {
    match geometry.get(&node.id) {
        Some(g) => (round2(g.size.width), round2(g.size.height)),
        None => (round2(node.width as f64), round2(node.height as f64)),
    }
}

/// `NodeSummary = { id, name, kind, width, height, children?: int }`.
pub fn summary(node: &Node, geometry: &Geometry) -> Value {
    summary_tree(node, geometry, 0)
}

/// A summary whose children are listed as summaries for `depth` levels, then
/// counted.
pub fn summary_tree(node: &Node, geometry: &Geometry, depth: u32) -> Value {
    let (w, h) = size(node, geometry);
    let mut m = Map::new();
    m.insert("id".into(), json!(node.id));
    m.insert("name".into(), json!(node.name));
    m.insert("kind".into(), json!(node.kind.tag()));
    m.insert("width".into(), json!(w));
    m.insert("height".into(), json!(h));
    if let Some(children) = node.children() {
        m.insert(
            "children".into(),
            children_value(children, depth, |c| summary_tree(c, geometry, depth - 1)),
        );
    }
    Value::Object(m)
}

/// The children array, or its length once `depth` levels are used up.
pub fn children_value(children: &[Node], depth: u32, f: impl Fn(&Node) -> Value) -> Value {
    if depth == 0 {
        json!(children.len())
    } else {
        Value::Array(children.iter().map(f).collect())
    }
}

/// File-format node JSON with children in full for `depth` levels; below
/// that `"children": <count>`.
pub fn node_json(node: &Node, depth: u32) -> Value {
    let mut v = file_json(node);
    truncate_children(&mut v, depth);
    v
}

fn truncate_children(v: &mut Value, depth: u32) {
    let Some(children) = v.get_mut("children") else {
        return;
    };
    if depth == 0 {
        let n = children.as_array().map_or(0, Vec::len);
        *children = json!(n);
    } else if let Some(arr) = children.as_array_mut() {
        for c in arr {
            truncate_children(c, depth - 1);
        }
    }
}

/// A node built from a spec, with the IDs assigned to it and its subtree.
pub struct Built {
    pub node: Node,
    /// Name path ("Card/Title"; repeated sibling names get "#2", "#3") to ID.
    pub ids: BTreeMap<String, NodeId>,
}

/// Turn a `NodeSpec` (node JSON without `id`, possibly with nested
/// `children`) into a node with fresh IDs.
pub fn build_node(spec: Map<String, Value>) -> Result<Built, ToolError> {
    let mut ids = BTreeMap::new();
    let root = path_label(&spec);
    let mut spec = Value::Object(spec);
    assign_ids(&mut spec, &root, &mut ids, "node")?;
    let node: Node = serde_json::from_value(spec.clone())
        .map_err(|e| ToolError::invalid_args(format!("node: {e}")))?;
    check_keys(&spec, &node, "node")?;
    Ok(Built { node, ids })
}

/// The file format keeps unknown keys, so a spec with a misspelt or misplaced
/// key (`font_size` beside `style` rather than in it) would build a node that
/// silently ignores it. Hold each node's keys to what `update_node` accepts.
fn check_keys(spec: &Value, node: &Node, at: &str) -> Result<(), ToolError> {
    let Value::Object(obj) = spec else {
        return Ok(());
    };
    let props: Map<String, Value> = obj
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "id" | "kind" | "children"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut probe = node.clone();
    if let Some(children) = probe.children_mut() {
        children.clear();
    }
    NodePatch::from_json(Value::Object(props))
        .and_then(|patch| patch.apply_to(&mut probe))
        .map_err(|e| ToolError::invalid_args(format!("{at}: {e}")))?;
    if let (Some(Value::Array(specs)), Some(children)) = (obj.get("children"), node.children()) {
        for (i, (s, c)) in specs.iter().zip(children).enumerate() {
            check_keys(s, c, &format!("{at}.children[{i}]"))?;
        }
    }
    Ok(())
}

fn path_label(obj: &Map<String, Value>) -> String {
    match obj.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => obj
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("node")
            .to_string(),
    }
}

fn assign_ids(
    v: &mut Value,
    path: &str,
    ids: &mut BTreeMap<String, NodeId>,
    at: &str,
) -> Result<(), ToolError> {
    let Value::Object(obj) = v else {
        return Err(ToolError::invalid_args(format!(
            "{at}: expected a node object"
        )));
    };
    if obj.contains_key("id") {
        return Err(ToolError::invalid_args(format!(
            "{at}: remove \"id\"; IDs are assigned"
        )));
    }
    let id = Id::new(IdKind::Node);
    obj.insert("id".into(), json!(id));
    ids.insert(path.to_string(), id);
    if let Some(children) = obj.get_mut("children") {
        let Value::Array(children) = children else {
            return Err(ToolError::invalid_args(format!(
                "{at}.children: expected an array"
            )));
        };
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (i, c) in children.iter_mut().enumerate() {
            let label = c.as_object().map(path_label).unwrap_or_default();
            let n = seen.entry(label.clone()).or_default();
            *n += 1;
            let label = if *n == 1 {
                label
            } else {
                format!("{label}#{n}")
            };
            assign_ids(
                c,
                &format!("{path}/{label}"),
                ids,
                &format!("{at}.children[{i}]"),
            )?;
        }
    }
    Ok(())
}

/// Give a node and its subtree fresh IDs (for duplicates).
pub fn refresh_ids(node: &mut Node) {
    node.id = Id::new(IdKind::Node);
    if let Some(children) = node.children_mut() {
        for c in children {
            refresh_ids(c);
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use doc::NodeKind;

    fn build(v: Value) -> Result<Built, ToolError> {
        build_node(v.as_object().unwrap().clone())
    }

    #[test]
    fn create_node_refuses_keys_update_node_would() {
        let misplaced = build(json!({ "kind": "text", "content": "Hi", "font_size": 88 }));
        assert!(misplaced.err().unwrap().to_json().contains("font_size"));
        let wrong_kind = build(json!({ "kind": "rect", "content": "Hi" }));
        assert!(wrong_kind
            .err()
            .unwrap()
            .to_json()
            .contains("not applicable"));
        let nested = build(json!({ "kind": "frame", "children": [
            { "kind": "rect" },
            { "kind": "text", "colour": "#fff" }
        ]}));
        assert!(nested.err().unwrap().to_json().contains("node.children[1]"));

        let ok = build(
            json!({ "kind": "frame", "name": "Card", "fills": [], "children": [
                { "kind": "text", "content": "Hi", "style": { "font_size": 88 }, "sizing": "fixed" }
            ]}),
        )
        .unwrap();
        let NodeKind::Text(t) = &ok.node.children().unwrap()[0].kind else {
            panic!("a text node")
        };
        assert_eq!(t.style.font_size, 88.0);
    }
}
