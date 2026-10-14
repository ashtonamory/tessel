//! Component edits (§3.7, §10 M5): create a component from the selection,
//! place, detach and delete instances and components, and set or reset
//! instance overrides. Each is one `Editor::commit`, so each is one undo
//! step. The panels in `components_panel.rs` and `instance_inspector.rs`
//! call these.

use std::collections::BTreeMap;

use doc::expand::{component, owning_instance};
use doc::{
    Command, ComponentId, Id, IdKind, Instance, Node, NodeId, NodeKind, NodePatch, Override,
};
use kurbo::Point;

use crate::canvas::Canvas;
use crate::editor::Editor;

/// The last segment of a component name: "Button/Primary" → "Primary".
pub fn short_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name).trim()
}

impl Editor {
    /// A node as it is shown: a node of the page (or master), or a node
    /// inside an instance by its derived ID.
    pub fn shown_node(&self, id: &NodeId) -> Option<Node> {
        doc::shown_node(&self.doc, id)
    }

    /// Ctrl+Alt+K: turn the selected frame into a component, or wrap the
    /// selection in a new frame first and turn that into one. The instance
    /// that takes the frame's place stays selected.
    pub fn create_component_from_selection(&mut self) {
        if self.editing_component().is_some() {
            self.status = Some("Leave the component to create another one".into());
            return;
        }
        let top: Vec<NodeId> = self
            .top_selected()
            .into_iter()
            .filter(|id| !id.is_derived())
            .collect();
        if top.is_empty() {
            self.status = Some("Select layers to make a component".into());
            return;
        }
        let single_frame = match top.as_slice() {
            [id] => matches!(self.node(id).map(|n| &n.kind), Some(NodeKind::Frame(_))),
            _ => false,
        };
        let (mut cmds, from) = if single_frame {
            (Vec::new(), top[0].clone())
        } else {
            self.selection = top;
            match self.wrap_commands(true) {
                Some((cmds, frame)) => (vec![Command::Batch(cmds)], frame),
                None => return,
            }
        };
        cmds.push(Command::CreateComponent { from: from.clone() });
        let _ = self.commit("Create component", cmds, vec![from]);
    }

    /// Place an instance of `id` centred on `at` (page coordinates), in the
    /// innermost frame there, or in the middle of `view` when `at` is
    /// `None`. The new instance is selected.
    pub fn insert_instance(
        &mut self,
        id: &ComponentId,
        at: Option<Point>,
        view: kurbo::Rect,
    ) -> Option<NodeId> {
        if self.editing_component() == Some(id) {
            self.status = Some("A component cannot contain an instance of itself".into());
            return None;
        }
        let Some(c) = component(&self.doc, id) else {
            self.status = Some(format!("Component {id} not found"));
            return None;
        };
        let (w, h) = (c.root.width, c.root.height);
        let name = short_name(&c.name).to_string();
        let at = at.unwrap_or(view.center());
        let (parent, to_local) = Canvas::parent_at(self, at);
        let local = to_local * at;
        let node_id = Id::new(IdKind::Node);
        let node = Node::new(
            node_id.clone(),
            if name.is_empty() { "Instance" } else { &name },
            NodeKind::Instance(Instance {
                component: id.clone(),
                overrides: BTreeMap::new(),
                extra: Default::default(),
            }),
        )
        .with_geometry(
            (local.x as f32 - w / 2.0).round(),
            (local.y as f32 - h / 2.0).round(),
            w,
            h,
        );
        let cmd = Command::CreateNode {
            parent,
            index: None,
            node,
        };
        self.commit("Insert instance", vec![cmd], vec![node_id.clone()])
            .ok()
            .map(|_| node_id)
    }

    /// The selected instances that can be detached (stored ones, not
    /// instances nested inside other instances).
    fn selected_instances(&self) -> Vec<NodeId> {
        self.top_selected()
            .into_iter()
            .filter(|id| matches!(self.node(id).map(|n| &n.kind), Some(NodeKind::Instance(_))))
            .collect()
    }

    /// Ctrl+Alt+B: turn the selected instances into ordinary frames.
    pub fn detach_selection(&mut self) {
        let ids = self.selected_instances();
        if ids.is_empty() {
            self.status = Some(if self.selection.iter().any(|id| id.is_derived()) {
                "Detach the outer instance first".into()
            } else {
                "Select an instance to detach".into()
            });
            return;
        }
        let label = if ids.len() == 1 {
            "Detach instance".to_string()
        } else {
            format!("Detach {} instances", ids.len())
        };
        let cmds = ids
            .iter()
            .map(|id| Command::DetachInstance { id: id.clone() })
            .collect();
        let _ = self.commit(&label, vec![Command::Batch(cmds)], ids);
    }

    /// Instances of component `id` in the document (on pages and in other
    /// masters), and instances that swap a nested instance to it.
    pub fn component_users(&self, id: &ComponentId) -> usize {
        let doc = self.document();
        let mut n = 0;
        let mut visit = |node: &Node| {
            if let NodeKind::Instance(i) = &node.kind {
                if &i.component == id
                    || i.overrides
                        .values()
                        .any(|o| matches!(o, Override::Swap { component } if component == id))
                {
                    n += 1;
                }
            }
        };
        doc.walk_nodes(&mut visit);
        for c in doc.components.iter().filter(|c| &c.id != id) {
            c.root.walk(&mut visit);
        }
        n
    }

    /// Delete component `id`. With `detach`, its instances first become
    /// ordinary frames; without, a used component is not deleted.
    pub fn delete_component(&mut self, id: &ComponentId, detach: bool) {
        // Instances live on pages, so show the document.
        self.leave_component();
        let sel = self.selection.clone();
        let _ = self.commit(
            "Delete component",
            vec![Command::DeleteComponent {
                id: id.clone(),
                detach,
            }],
            sel,
        );
    }

    pub fn rename_component(&mut self, id: &ComponentId, name: String) {
        let _ = self.update_component("Rename component", id, |c| c.name = name);
    }

    /// The component an instance (stored or nested) shows.
    pub fn instance_component(&self, id: &NodeId) -> Option<ComponentId> {
        match self.shown_node(id)?.kind {
            NodeKind::Instance(i) => Some(i.component),
            _ => None,
        }
    }

    /// "Go to master": edit the component the instance `id` shows.
    pub fn go_to_master(&mut self, id: &NodeId) -> bool {
        match self.instance_component(id) {
            Some(c) => self.edit_component(&c),
            None => false,
        }
    }

    /// Override a prop of the node `id` inside an instance (a derived ID):
    /// `patch` sets one of content, fills, visible or component.
    pub fn set_override(&mut self, label: &str, id: &NodeId, patch: NodePatch) {
        let sel = self.selection.clone();
        let _ = self.commit(
            label,
            vec![Command::SetProps {
                id: id.clone(),
                patch,
            }],
            sel,
        );
    }

    /// The instance stored in the document that holds the override for a
    /// derived ID, and its key.
    pub fn override_slot(&self, id: &NodeId) -> Option<(NodeId, String)> {
        let instance = owning_instance(id)?;
        let key = id.as_str()[instance.as_str().len() + 1..].to_string();
        Some((instance, key))
    }

    /// The override the instance holds for the derived node `id`.
    pub fn override_of(&self, id: &NodeId) -> Option<Override> {
        let (instance, key) = self.override_slot(id)?;
        match &self.doc.node_or_master(&instance)?.kind {
            NodeKind::Instance(i) => i.overrides.get(&key).cloned(),
            _ => None,
        }
    }

    /// Reset the derived node `id` to what its master shows.
    pub fn reset_override(&mut self, id: &NodeId) {
        let Some((instance, key)) = self.override_slot(id) else {
            return;
        };
        self.clear_overrides(&instance, vec![key], "Reset override");
    }

    /// Reset every override of the instance `id`.
    pub fn reset_all_overrides(&mut self, id: &NodeId) {
        let keys = match self.node(id).map(|n| &n.kind) {
            Some(NodeKind::Instance(i)) => i.overrides.keys().cloned().collect(),
            _ => return,
        };
        self.clear_overrides(id, keys, "Reset all overrides");
    }

    fn clear_overrides(&mut self, instance: &NodeId, keys: Vec<String>, label: &str) {
        if keys.is_empty() {
            return;
        }
        let patch = NodePatch {
            overrides: Some(keys.into_iter().map(|k| (k, None)).collect()),
            ..Default::default()
        };
        let sel = self.selection.clone();
        let _ = self.commit(
            label,
            vec![Command::SetProps {
                id: instance.clone(),
                patch,
            }],
            sel,
        );
    }
}
