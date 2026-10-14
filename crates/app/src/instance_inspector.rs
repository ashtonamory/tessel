//! The inspector's component section (§9 "component overrides for
//! instances"). For a selected instance: its component, Go to master,
//! Detach, a swap picker, the component's named props, and every child it
//! shows with its overridable props (text, fills, visible, swap) and a
//! reset to the master. For a node inside an instance: the same for that
//! node. While a master is edited: the component's props, and exposing
//! the selected layer as a new one. Edits are collected by `ui` and run by
//! `apply`, like the main inspector.

use std::collections::HashMap;

use doc::expand::component;
use doc::{
    ColorRef, ComponentId, ComponentProp, Id, Node, NodeId, NodeKind, NodePatch, Paint, PropKind,
};
use egui::{Color32, Rect, RichText, Ui};

use crate::editor::Editor;
use crate::inspector::{resolve_ref, to_hex};

enum Action {
    Override {
        label: &'static str,
        id: NodeId,
        patch: Box<NodePatch>,
        /// Part of a drag (the colour picker): one undo step until release.
        live: bool,
    },
    Reset(NodeId),
    ResetAll(NodeId),
    /// Show another component in a stored instance (`component`) or a
    /// nested one (a swap override).
    Swap(NodeId, ComponentId),
    Detach(NodeId),
    GoToMaster(NodeId),
    Select(NodeId),
    AddProp(ComponentId, ComponentProp),
    RemoveProp(ComponentId, usize),
    RenameProp(ComponentId, usize, String),
}

#[derive(Default)]
pub struct InstanceInspector {
    actions: Vec<Action>,
    /// Text field buffers by field key, refreshed from the document while
    /// the field is not focused.
    buffers: HashMap<String, String>,
    /// A colour drag in progress.
    live: bool,
    /// The name typed for a new component prop, and the layer it is for.
    prop_name: Option<(NodeId, String)>,
    fields: Vec<(String, Rect)>,
}

/// The first fill's colour, if it is a solid one.
fn solid_fill(ed: &Editor, n: &Node) -> Option<[u8; 4]> {
    let fills = match &n.kind {
        NodeKind::Text(t) => &t.fills,
        _ => &n.shape()?.fills,
    };
    match fills.first()? {
        Paint::Solid(s) => resolve_ref(&ed.doc, &s.color),
        _ => None,
    }
}

fn fills_of(n: &Node) -> Option<Vec<Paint>> {
    match &n.kind {
        NodeKind::Text(t) => Some(t.fills.clone()),
        _ => n.shape().map(|s| s.fills.clone()),
    }
}

/// The derived ID of the instance that shows a derived node.
fn parent_instance(id: &NodeId) -> Option<NodeId> {
    id.as_str()
        .rsplit_once('/')
        .map(|(outer, _)| Id::from_raw(outer))
}

fn kind_name(k: PropKind) -> &'static str {
    match k {
        PropKind::Text => "text",
        PropKind::Visible => "visible",
        PropKind::Swap => "swap",
    }
}

impl InstanceInspector {
    /// Where field `key` was drawn last frame ("detach", "master",
    /// "reset-all", "text:<id>", "visible:<id>", "reset:<id>",
    /// "prop:<name>", "expose:<kind>", "remove-prop:<name>").
    pub fn field_rect(&self, key: &str) -> Option<Rect> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, r)| *r)
    }

    pub fn field_keys(&self) -> Vec<&str> {
        self.fields.iter().map(|(k, _)| k.as_str()).collect()
    }

    fn field(&mut self, key: String, r: &egui::Response) {
        self.fields.push((key, r.rect));
    }

    pub fn ui(&mut self, ui: &mut Ui, ed: &Editor) {
        self.fields.clear();
        let [id] = ed.selection.as_slice() else {
            return;
        };
        let read_only = ed.read_only;
        let shown = ed.shown_node(id);
        let is_instance = matches!(shown.as_ref().map(|n| &n.kind), Some(NodeKind::Instance(_)));
        let in_master = ed.editing_component().is_some() && !id.is_derived();
        if !(is_instance || id.is_derived() || in_master) {
            return;
        }
        ui.add_enabled_ui(!read_only, |ui| {
            if id.is_derived() {
                self.derived_section(ui, ed, id);
            } else if is_instance {
                self.instance_section(ui, ed, id);
            }
            if in_master {
                self.props_section(ui, ed, id);
            }
        });
        ui.separator();
    }

    fn component_name(ed: &Editor, c: &ComponentId) -> String {
        component(ed.document(), c)
            .or_else(|| component(&ed.doc, c))
            .map_or_else(|| c.to_string(), |c| c.name.clone())
    }

    fn instance_section(&mut self, ui: &mut Ui, ed: &Editor, id: &NodeId) {
        let Some(node) = ed.node(id) else {
            return;
        };
        let NodeKind::Instance(inst) = &node.kind else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("◇").color(crate::canvas::SELECTION));
            ui.strong(Self::component_name(ed, &inst.component));
        });
        ui.horizontal(|ui| {
            let r = ui
                .button("Go to master")
                .on_hover_text("Edit the component; every instance updates");
            self.field("master".into(), &r);
            if r.clicked() {
                self.actions.push(Action::GoToMaster(id.clone()));
            }
            let r = ui
                .button("Detach")
                .on_hover_text("Turn into a plain frame (Ctrl+Alt+B)");
            self.field("detach".into(), &r);
            if r.clicked() {
                self.actions.push(Action::Detach(id.clone()));
            }
        });
        self.swap_picker(ui, ed, id, &inst.component);
        if let Some(c) = component(&ed.doc, &inst.component) {
            if !c.props.is_empty() {
                ui.separator();
                ui.label("Properties");
                for p in &c.props {
                    self.prop_field(ui, ed, id, p);
                }
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("Overrides ({})", inst.overrides.len()));
            if !inst.overrides.is_empty() {
                let r = ui.small_button("Reset all");
                self.field("reset-all".into(), &r);
                if r.clicked() {
                    self.actions.push(Action::ResetAll(id.clone()));
                }
            }
        });
        let Some(expanded) = ed.geometry.expanded(id) else {
            ui.weak("This instance's component is missing.");
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("instance-overrides")
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for c in expanded.children().into_iter().flatten() {
                    self.child_rows(ui, ed, c, 0);
                }
            });
    }

    fn child_rows(&mut self, ui: &mut Ui, ed: &Editor, node: &Node, depth: usize) {
        self.node_row(ui, ed, node, depth);
        let shown = ed.geometry.shown(node);
        for c in shown.children().into_iter().flatten() {
            self.child_rows(ui, ed, c, depth + 1);
        }
    }

    /// One node inside an instance: visibility, name, reset, and its text,
    /// fill or swap.
    fn node_row(&mut self, ui: &mut Ui, ed: &Editor, node: &Node, depth: usize) {
        let id = &node.id;
        let overridden = ed.override_of(id).is_some();
        let indent = depth as f32 * 12.0;
        ui.horizontal(|ui| {
            ui.add_space(indent);
            let mut visible = node.visible;
            let r = ui.checkbox(&mut visible, "").on_hover_text("Visible");
            self.field(format!("visible:{id}"), &r);
            if r.changed() {
                self.actions.push(Action::Override {
                    label: if visible { "Show layer" } else { "Hide layer" },
                    id: id.clone(),
                    patch: Box::new(NodePatch {
                        visible: Some(visible),
                        ..Default::default()
                    }),
                    live: false,
                });
            }
            let name = RichText::new(&node.name);
            ui.label(if overridden {
                name.strong()
            } else {
                name.weak()
            });
            if let Some(c) = solid_fill(ed, node) {
                let [r, g, b, a] = c;
                let mut color = Color32::from_rgba_unmultiplied(r, g, b, a);
                let resp = ui.color_edit_button_srgba(&mut color);
                if resp.changed() {
                    let mut fills = fills_of(node).unwrap_or_default();
                    let hex = to_hex(color.to_srgba_unmultiplied());
                    if let Some(Paint::Solid(s)) = fills.first_mut() {
                        s.color = ColorRef::literal(&hex);
                    }
                    self.actions.push(Action::Override {
                        label: "Override fill",
                        id: id.clone(),
                        patch: Box::new(NodePatch {
                            fills: Some(fills),
                            ..Default::default()
                        }),
                        live: true,
                    });
                }
            }
            if overridden {
                let r = ui.small_button("↺").on_hover_text("Reset to the master");
                self.field(format!("reset:{id}"), &r);
                if r.clicked() {
                    self.actions.push(Action::Reset(id.clone()));
                }
            }
        });
        match &node.kind {
            NodeKind::Text(t) => {
                ui.horizontal(|ui| {
                    ui.add_space(indent + 24.0);
                    self.text_field(ui, format!("text:{id}"), &t.content, |content| {
                        Action::Override {
                            label: "Override text",
                            id: id.clone(),
                            patch: Box::new(NodePatch {
                                content: Some(content),
                                ..Default::default()
                            }),
                            live: false,
                        }
                    });
                });
            }
            NodeKind::Instance(i) => {
                ui.horizontal(|ui| {
                    ui.add_space(indent + 24.0);
                    self.swap_picker(ui, ed, id, &i.component);
                });
            }
            _ => {}
        }
    }

    /// A single-line field that shows `current` and, when typing ends with
    /// a different value, pushes `action(value)`.
    fn text_field(
        &mut self,
        ui: &mut Ui,
        key: String,
        current: &str,
        action: impl FnOnce(String) -> Action,
    ) {
        let wid = egui::Id::new(("instance-inspector", &key));
        let focused = ui.memory(|m| m.has_focus(wid));
        let buf = self.buffers.entry(key.clone()).or_default();
        if !focused {
            *buf = current.to_string();
        }
        let r = ui.add(
            egui::TextEdit::singleline(buf)
                .id(wid)
                .desired_width(ui.available_width()),
        );
        let value = buf.clone();
        self.field(key, &r);
        if r.lost_focus() && value != current {
            self.actions.push(action(value));
        }
    }

    fn swap_picker(&mut self, ui: &mut Ui, ed: &Editor, id: &NodeId, current: &ComponentId) {
        let mut picked = None;
        egui::ComboBox::from_id_salt(("swap", id.as_str()))
            .selected_text(Self::component_name(ed, current))
            .show_ui(ui, |ui| {
                for c in &ed.document().components {
                    if ui.selectable_label(&c.id == current, &c.name).clicked() && &c.id != current
                    {
                        picked = Some(c.id.clone());
                    }
                }
            })
            .response
            .on_hover_text("Swap to another component");
        if let Some(c) = picked {
            self.actions.push(Action::Swap(id.clone(), c));
        }
    }

    /// A named component prop of the instance `inst`.
    fn prop_field(&mut self, ui: &mut Ui, ed: &Editor, inst: &NodeId, p: &ComponentProp) {
        let target = Id::derived(inst, &p.target);
        let Some(node) = ed.shown_node(&target) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(&p.name);
            match (p.kind, &node.kind) {
                (PropKind::Text, NodeKind::Text(t)) => {
                    self.text_field(ui, format!("prop:{}", p.name), &t.content, |content| {
                        Action::Override {
                            label: "Set property",
                            id: target.clone(),
                            patch: Box::new(NodePatch {
                                content: Some(content),
                                ..Default::default()
                            }),
                            live: false,
                        }
                    })
                }
                (PropKind::Visible, _) => {
                    let mut visible = node.visible;
                    let r = ui.checkbox(&mut visible, "");
                    self.field(format!("prop:{}", p.name), &r);
                    if r.changed() {
                        self.actions.push(Action::Override {
                            label: "Set property",
                            id: target.clone(),
                            patch: Box::new(NodePatch {
                                visible: Some(visible),
                                ..Default::default()
                            }),
                            live: false,
                        });
                    }
                }
                (PropKind::Swap, NodeKind::Instance(i)) => {
                    self.swap_picker(ui, ed, &target, &i.component)
                }
                _ => {
                    ui.weak("(does not fit its layer)");
                }
            }
        });
    }

    /// A node inside an instance.
    fn derived_section(&mut self, ui: &mut Ui, ed: &Editor, id: &NodeId) {
        let Some(node) = ed.shown_node(id) else {
            return;
        };
        let outer = ed.override_slot(id).map(|(i, _)| i);
        let parent = parent_instance(id);
        let parent_component = parent.as_ref().and_then(|p| ed.instance_component(p));
        ui.horizontal(|ui| {
            ui.label(RichText::new("◇").color(crate::canvas::SELECTION));
            ui.label("Inside");
            ui.strong(
                parent_component
                    .as_ref()
                    .map_or(String::new(), |c| Self::component_name(ed, c)),
            );
        });
        ui.weak("Only text, fills, visibility and swaps can change here.");
        ui.horizontal(|ui| {
            if let Some(outer) = outer {
                let r = ui.button("Select instance");
                self.field("select-instance".into(), &r);
                if r.clicked() {
                    self.actions.push(Action::Select(outer));
                }
            }
            let target = match &node.kind {
                NodeKind::Instance(_) => Some(id.clone()),
                _ => parent.clone(),
            };
            if let Some(t) = target {
                let r = ui.button("Go to master");
                self.field("master".into(), &r);
                if r.clicked() {
                    self.actions.push(Action::GoToMaster(t));
                }
            }
        });
        ui.separator();
        self.node_row(ui, ed, &node, 0);
    }

    /// While a master is edited: the component's props, and exposing the
    /// selected layer as one.
    fn props_section(&mut self, ui: &mut Ui, ed: &Editor, selected: &NodeId) {
        let Some(cid) = ed.editing_component().cloned() else {
            return;
        };
        let Some(c) = component(ed.document(), &cid) else {
            return;
        };
        ui.separator();
        ui.strong("Component properties");
        if c.props.is_empty() {
            ui.weak(
                "None yet. Select a layer of the master to expose its text, visibility or swap.",
            );
        }
        for (i, p) in c.props.iter().enumerate() {
            let target = ed
                .doc
                .node(&p.target)
                .map_or_else(|| p.target.to_string(), |n| n.name.clone());
            ui.horizontal(|ui| {
                let cid2 = cid.clone();
                self.text_field(ui, format!("prop-name:{}", p.name), &p.name, |name| {
                    Action::RenameProp(cid2, i, name)
                });
            });
            ui.horizontal(|ui| {
                ui.weak(format!("{} of {target}", kind_name(p.kind)));
                let r = ui.small_button("×").on_hover_text("Remove property");
                self.field(format!("remove-prop:{}", p.name), &r);
                if r.clicked() {
                    self.actions.push(Action::RemoveProp(cid.clone(), i));
                }
            });
        }
        let root = ed.master_root();
        let Some(node) = ed.node(selected).filter(|_| root != Some(selected)) else {
            return;
        };
        let mut kinds = vec![PropKind::Visible];
        match node.kind {
            NodeKind::Text(_) => kinds.insert(0, PropKind::Text),
            NodeKind::Instance(_) => kinds.push(PropKind::Swap),
            _ => {}
        }
        ui.separator();
        ui.label(format!("Expose {} as a property", node.name));
        if self.prop_name.as_ref().is_none_or(|(n, _)| n != selected) {
            let default = node.name.trim().to_lowercase().replace(' ', "_");
            self.prop_name = Some((selected.clone(), default));
        }
        let (_, name) = self.prop_name.as_mut().expect("set above");
        let r = ui.add(egui::TextEdit::singleline(name).hint_text("name"));
        let name = name.trim().to_string();
        self.fields.push(("prop-name".into(), r.rect));
        ui.horizontal(|ui| {
            for k in kinds {
                let taken = c.props.iter().any(|p| p.name == name);
                let r = ui
                    .add_enabled(!name.is_empty() && !taken, egui::Button::new(kind_name(k)))
                    .on_hover_text(format!("Expose its {} as \"{name}\"", kind_name(k)));
                self.field(format!("expose:{}", kind_name(k)), &r);
                if r.clicked() {
                    self.actions.push(Action::AddProp(
                        cid.clone(),
                        ComponentProp {
                            name: name.clone(),
                            target: selected.clone(),
                            kind: k,
                        },
                    ));
                }
            }
        });
    }

    /// Run the edits collected by `ui`, and end a colour drag once the
    /// pointer is released.
    pub fn apply(&mut self, ctx: &egui::Context, ed: &mut Editor) {
        for a in std::mem::take(&mut self.actions) {
            self.run(ed, a);
        }
        if self.live && !ctx.input(|i| i.pointer.any_down()) {
            self.live = false;
            ed.end_gesture();
        }
    }

    fn run(&mut self, ed: &mut Editor, a: Action) {
        match a {
            Action::Override {
                label,
                id,
                patch,
                live: true,
            } => {
                if (!self.live || !ed.in_gesture()) && ed.begin_gesture(label).is_err() {
                    return;
                }
                self.live = true;
                let patch = *patch;
                if let Err(e) = ed.gesture_apply(doc::Command::SetProps { id, patch }) {
                    ed.status = Some(format!("{label}: {e}"));
                }
            }
            Action::Override {
                label, id, patch, ..
            } => ed.set_override(label, &id, *patch),
            Action::Reset(id) => ed.reset_override(&id),
            Action::ResetAll(id) => ed.reset_all_overrides(&id),
            Action::Swap(id, component) => ed.set_override(
                "Swap instance",
                &id,
                NodePatch {
                    component: Some(component),
                    ..Default::default()
                },
            ),
            Action::Detach(id) => {
                ed.selection = vec![id];
                ed.detach_selection();
            }
            Action::GoToMaster(id) => {
                ed.go_to_master(&id);
            }
            Action::Select(id) => ed.selection = vec![id],
            Action::AddProp(c, prop) => {
                let _ = ed.update_component("Add property", &c, |comp| comp.props.push(prop));
            }
            Action::RemoveProp(c, i) => {
                let _ = ed.update_component("Remove property", &c, |comp| {
                    if i < comp.props.len() {
                        comp.props.remove(i);
                    }
                });
            }
            Action::RenameProp(c, i, name) => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let _ = ed.update_component("Rename property", &c, |comp| {
                    if let Some(p) = comp.props.get_mut(i) {
                        p.name = name;
                    }
                });
            }
        }
    }
}
