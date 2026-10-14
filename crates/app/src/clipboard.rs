//! Copy and paste of nodes through the system clipboard as JSON text (§9).
//! The text carries a marker so a paste can tell our nodes from other text.

use doc::{Command, Node, NodeId};

use crate::editor::Editor;
use crate::structure::{fresh_ids, OFFSET};

/// The value of the `clipboard` key in copied text.
pub const MARKER: &str = "tessel/nodes@1";

/// The nodes in `text`, if it is our clipboard format: a JSON object with
/// `clipboard` set to `MARKER` and the nodes, back to front, under `nodes`
/// with page-absolute positions.
pub fn parse(text: &str) -> Option<Vec<Node>> {
    if !text.contains(MARKER) {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("clipboard")?.as_str()? != MARKER {
        return None;
    }
    serde_json::from_value(v.get_mut("nodes")?.take()).ok()
}

impl Editor {
    /// The selection as clipboard text, with positions on the page rather
    /// than in the parent so a paste lands where the nodes were seen.
    pub fn copy_text(&self) -> Option<String> {
        let mut nodes = Vec::new();
        // Document order is back to front.
        let top = self.top_selected();
        self.doc.walk_nodes(&mut |n| {
            if top.contains(&n.id) {
                nodes.push(n.clone());
            }
        });
        if nodes.is_empty() {
            return None;
        }
        for n in &mut nodes {
            if let Some(g) = self.geometry.get(&n.id) {
                let c = g.bounds.center();
                n.x = ((c.x - g.size.width / 2.0) as f32 * 100.0).round() / 100.0;
                n.y = ((c.y - g.size.height / 2.0) as f32 * 100.0).round() / 100.0;
            }
        }
        serde_json::to_string(&serde_json::json!({ "clipboard": MARKER, "nodes": nodes })).ok()
    }

    /// Paste nodes copied by `copy_text` onto the current page, with fresh
    /// IDs, and select them. Returns false when `text` is not our format.
    /// Pasting over the source (or an earlier paste) shifts the nodes down
    /// and right until they land on empty ground.
    pub fn paste_text(&mut self, text: &str) -> bool {
        let Some(mut nodes) = parse(text) else {
            return false;
        };
        if nodes.is_empty() {
            return true;
        }
        let first = nodes[0].clone();
        // Is a node like the first one already at its spot, shifted by `k`?
        let taken = |k: f32, ed: &Editor| {
            let mut found = false;
            for top in &ed.page().children {
                top.walk(&mut |n| {
                    let Some(g) = ed.geometry.get(&n.id) else {
                        return;
                    };
                    let c = g.bounds.center();
                    let (x, y) = (c.x - g.size.width / 2.0, c.y - g.size.height / 2.0);
                    found |= n.kind.tag() == first.kind.tag()
                        && (x - f64::from(first.x + k * OFFSET)).abs() < 0.01
                        && (y - f64::from(first.y + k * OFFSET)).abs() < 0.01
                        && n.width == first.width
                        && n.height == first.height;
                });
            }
            found
        };
        let mut k = 0.0;
        while taken(k, self) {
            k += 1.0;
        }
        let parent = self.insert_parent();
        let mut ids: Vec<NodeId> = Vec::new();
        let mut cmds = Vec::new();
        for n in &mut nodes {
            fresh_ids(n);
            n.x += k * OFFSET;
            n.y += k * OFFSET;
            ids.push(n.id.clone());
            cmds.push(Command::CreateNode {
                parent: parent.clone(),
                index: None,
                node: n.clone(),
            });
        }
        let label = if ids.len() == 1 {
            "Paste layer".to_string()
        } else {
            format!("Paste {} layers", ids.len())
        };
        let _ = self.commit(&label, vec![Command::Batch(cmds)], ids);
        true
    }

    /// Ctrl+X: the text to put on the clipboard; the selection is deleted.
    pub fn cut_text(&mut self) -> Option<String> {
        let text = self.copy_text()?;
        self.delete_selection();
        Some(text)
    }
}
