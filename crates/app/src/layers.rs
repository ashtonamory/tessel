//! The left panel (§9): the pages list, then the page's node tree,
//! front-most on top, with multi-select (Ctrl toggles, Shift selects a
//! range), rename (double-click), visibility and lock toggles, collapse,
//! and drag-and-drop reorder/reparent. Every change is an editor command.

use std::collections::HashSet;

use doc::{Node, NodeId, NodeKind, ParentRef};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, TextEdit, Ui, Vec2};

use crate::canvas::SELECTION;
use crate::editor::{drop_index, Editor};
use crate::pages::PagesPanel;

const ROW: f32 = 22.0;
const INDENT: f32 = 14.0;

enum Action {
    Select {
        id: NodeId,
        mode: SelectMode,
    },
    SetVisible(NodeId, bool),
    SetLocked(NodeId, bool),
    Rename(NodeId, String),
    Move {
        id: NodeId,
        parent: ParentRef,
        index: usize,
    },
    ToggleCollapsed(NodeId),
}

/// How a click changes the selection.
#[derive(Clone, Copy, PartialEq)]
enum SelectMode {
    Replace,
    /// Ctrl: add or remove one layer.
    Toggle,
    /// Shift: everything from the last clicked layer to this one.
    Range,
}

#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Above,
    Inside,
    Below,
}

struct Renaming {
    id: NodeId,
    buffer: String,
    focused: bool,
}

#[derive(Default)]
pub struct LayersPanel {
    pub pages: PagesPanel,
    /// The layer a Shift-click range starts from.
    anchor: Option<NodeId>,
    renaming: Option<Renaming>,
    collapsed: HashSet<NodeId>,
    /// Instances whose derived children are listed (closed by default).
    open_instances: HashSet<NodeId>,
    rows: Vec<(NodeId, Rect)>,
}

impl LayersPanel {
    /// Rows drawn last frame, top to bottom, in screen points.
    pub fn rows(&self) -> &[(NodeId, Rect)] {
        &self.rows
    }

    pub fn is_renaming(&self) -> bool {
        self.renaming.is_some() || self.pages.is_renaming()
    }

    pub fn is_collapsed(&self, id: &NodeId) -> bool {
        self.collapsed.contains(id)
    }

    pub fn ui(&mut self, ui: &mut Ui, ed: &mut Editor) {
        ui.add_space(4.0);
        if ed.editing_component().is_some() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("❖ Components").color(SELECTION));
                let back = format!("← {}", ed.document_page().name);
                if ui
                    .small_button(back)
                    .on_hover_text("Leave the component")
                    .clicked()
                {
                    ed.leave_component();
                }
            });
            ui.separator();
        } else {
            self.pages.ui(ui, ed);
        }
        ui.strong("Layers");
        ui.separator();
        let mut actions = Vec::new();
        self.rows.clear();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let page = ed.page_ref();
                let children = &ed.page().children;
                if children.is_empty() {
                    ui.weak("No layers yet. Pick a tool (R, O, T, F) and draw.");
                }
                for (i, n) in children.iter().enumerate().rev() {
                    self.row(ui, ed, n, &page, i, 0, false, &mut actions);
                }
            });
        for a in actions {
            match a {
                Action::Select { id, mode } => self.select(ed, id, mode),
                Action::SetVisible(id, v) => ed.set_visible(&id, v),
                Action::SetLocked(id, l) => ed.set_locked(&id, l),
                Action::Rename(id, name) => ed.rename(&id, name),
                Action::Move { id, parent, index } => ed.move_node(&id, parent, index),
                Action::ToggleCollapsed(id) => {
                    let set = if is_instance_id(ed, &id) {
                        &mut self.open_instances
                    } else {
                        &mut self.collapsed
                    };
                    if !set.remove(&id) {
                        set.insert(id);
                    }
                }
            }
        }
    }

    fn select(&mut self, ed: &mut Editor, id: NodeId, mode: SelectMode) {
        match mode {
            SelectMode::Replace => {
                ed.selection = vec![id.clone()];
                self.anchor = Some(id);
            }
            SelectMode::Toggle => {
                if let Some(i) = ed.selection.iter().position(|s| s == &id) {
                    ed.selection.remove(i);
                } else {
                    ed.selection.push(id.clone());
                }
                self.anchor = Some(id);
            }
            SelectMode::Range => {
                // `rows` lists the visible layers top to bottom.
                let pos = |id: &NodeId| self.rows.iter().position(|(r, _)| r == id);
                let from = self.anchor.as_ref().and_then(pos);
                match (from, pos(&id)) {
                    (Some(a), Some(b)) => {
                        let (lo, hi) = (a.min(b), a.max(b));
                        ed.selection = self.rows[lo..=hi].iter().map(|(r, _)| r.clone()).collect();
                    }
                    _ => {
                        ed.selection = vec![id.clone()];
                        self.anchor = Some(id);
                    }
                }
            }
        }
    }

    /// Whether an instance lists its derived children: opened by the user,
    /// or holding the selection.
    fn instance_open(&self, ed: &Editor, id: &NodeId) -> bool {
        let prefix = format!("{id}/");
        self.open_instances.contains(id)
            || ed.selection.iter().any(|s| s.as_str().starts_with(&prefix))
    }

    /// One layer and its children. `derived` rows are nodes shown inside an
    /// instance: greyed, selectable and hideable (an override), but not
    /// renamed, locked or moved.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut Ui,
        ed: &Editor,
        node: &Node,
        parent: &ParentRef,
        index: usize,
        depth: usize,
        derived: bool,
        actions: &mut Vec<Action>,
    ) {
        let instance = matches!(node.kind, NodeKind::Instance(_));
        let shown_children = if instance {
            ed.geometry.expanded(&node.id).and_then(|e| e.children())
        } else {
            node.children()
        };
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), ROW),
            Sense::click_and_drag(),
        );
        self.rows.push((node.id.clone(), rect));
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        let selected = ed.selection.contains(&node.id);
        if selected {
            painter.rect_filled(rect, CornerRadius::ZERO, SELECTION.gamma_multiply(0.35));
        } else if resp.hovered() {
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                visuals.widgets.hovered.weak_bg_fill,
            );
        }
        let text_color = if derived {
            visuals
                .weak_text_color()
                .gamma_multiply(if node.visible { 1.0 } else { 0.6 })
        } else if node.visible {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };

        let x0 = rect.min.x + 4.0 + depth as f32 * INDENT;
        let cy = rect.center().y;
        let arrow = Rect::from_min_size(Pos2::new(x0, rect.min.y), Vec2::new(12.0, ROW));
        let has_children = shown_children.is_some_and(|c| !c.is_empty());
        let collapsed = if instance {
            !self.instance_open(ed, &node.id)
        } else {
            self.collapsed.contains(&node.id)
        };
        if has_children {
            painter.text(
                arrow.center(),
                Align2::CENTER_CENTER,
                if collapsed { "▸" } else { "▾" },
                FontId::proportional(12.0),
                text_color,
            );
        }
        let is_master_root = ed.master_root() == Some(&node.id);
        painter.text(
            Pos2::new(arrow.max.x + 8.0, cy),
            Align2::CENTER_CENTER,
            if is_master_root {
                "❖"
            } else {
                kind_icon(&node.kind)
            },
            FontId::monospace(11.0),
            if instance || is_master_root {
                SELECTION
            } else {
                visuals.weak_text_color()
            },
        );
        let eye = Rect::from_center_size(Pos2::new(rect.max.x - 14.0, cy), Vec2::splat(16.0));
        let lock = Rect::from_center_size(Pos2::new(rect.max.x - 34.0, cy), Vec2::splat(16.0));
        let name_rect = Rect::from_min_max(
            Pos2::new(arrow.max.x + 18.0, rect.min.y + 1.0),
            Pos2::new(lock.min.x - 4.0, rect.max.y - 1.0),
        );

        // Name, or the rename editor.
        let renaming_this = self.renaming.as_ref().is_some_and(|r| r.id == node.id);
        if renaming_this {
            let r = self.renaming.as_mut().expect("checked");
            let out = ui.put(
                name_rect,
                TextEdit::singleline(&mut r.buffer).margin(egui::Margin::ZERO),
            );
            if !r.focused {
                out.request_focus();
                r.focused = true;
            } else if out.lost_focus() {
                let r = self.renaming.take().expect("checked");
                let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if !cancelled && !r.buffer.trim().is_empty() {
                    actions.push(Action::Rename(r.id, r.buffer.trim().to_string()));
                }
            }
        } else {
            let galley =
                painter.layout_no_wrap(node.name.clone(), FontId::proportional(13.0), text_color);
            let pos = Pos2::new(name_rect.min.x, cy - galley.size().y / 2.0);
            painter
                .with_clip_rect(name_rect)
                .galley(pos, galley, text_color);
        }

        // Visibility toggle: a filled dot when visible, a ring when hidden.
        let eye_hover = resp.hover_pos().is_some_and(|p| eye.contains(p));
        if node.visible {
            if eye_hover || resp.hovered() {
                painter.circle_filled(eye.center(), 4.0, text_color);
            }
        } else {
            painter.circle_stroke(eye.center(), 4.0, Stroke::new(1.2, text_color));
        }

        // Lock toggle: a padlock, shown when locked or hovered.
        if !derived && (node.locked || resp.hovered()) {
            padlock(&painter, lock.center(), node.locked, text_color);
        }

        if resp.clicked() {
            let p = resp.interact_pointer_pos().unwrap_or(rect.center());
            if eye.contains(p) {
                actions.push(Action::SetVisible(node.id.clone(), !node.visible));
            } else if lock.contains(p) && !derived {
                actions.push(Action::SetLocked(node.id.clone(), !node.locked));
            } else if has_children && arrow.contains(p) {
                actions.push(Action::ToggleCollapsed(node.id.clone()));
            } else {
                let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
                let mode = if command {
                    SelectMode::Toggle
                } else if shift {
                    SelectMode::Range
                } else {
                    SelectMode::Replace
                };
                actions.push(Action::Select {
                    id: node.id.clone(),
                    mode,
                });
            }
        }
        if resp.double_clicked() && !ed.read_only && !derived {
            self.renaming = Some(Renaming {
                id: node.id.clone(),
                buffer: node.name.clone(),
                focused: false,
            });
        }

        // Drag and drop.
        if resp.dragged() && !ed.read_only && !derived {
            resp.dnd_set_drag_payload(node.id.clone());
        }
        let zone = |p: Pos2| {
            let t = (p.y - rect.min.y) / rect.height();
            let container = matches!(node.kind, NodeKind::Frame(_) | NodeKind::Group(_));
            if container && (0.25..0.75).contains(&t) {
                Zone::Inside
            } else if t < 0.5 {
                Zone::Above
            } else {
                Zone::Below
            }
        };
        if let (Some(dragged), Some(p)) = (
            resp.dnd_hover_payload::<NodeId>(),
            ui.ctx().pointer_hover_pos(),
        ) {
            if *dragged != node.id && !derived {
                let stroke = Stroke::new(2.0, SELECTION);
                match zone(p) {
                    Zone::Above => {
                        painter.hline(rect.x_range(), rect.min.y + 1.0, stroke);
                    }
                    Zone::Below => {
                        painter.hline(rect.x_range(), rect.max.y - 1.0, stroke);
                    }
                    Zone::Inside => {
                        painter.rect_stroke(
                            rect.shrink(1.0),
                            CornerRadius::same(2),
                            stroke,
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }
        }
        if let (Some(dragged), Some(p)) = (
            resp.dnd_release_payload::<NodeId>(),
            ui.ctx().pointer_interact_pos(),
        ) {
            let dragged = (*dragged).clone();
            if dragged != node.id && !derived {
                let same_parent = |par: &ParentRef| match ed.doc.parent_of(&dragged) {
                    Some((dp, di)) if &dp == par => Some(di),
                    _ => None,
                };
                let (parent, index) = match zone(p) {
                    Zone::Inside => {
                        let into = ParentRef::Node(node.id.clone());
                        let len = node.children().map_or(0, |c| c.len());
                        let idx = match same_parent(&into) {
                            Some(_) => len - 1,
                            None => len,
                        };
                        (into, idx)
                    }
                    // Above in the panel is in front: later in the child list.
                    Zone::Above => (parent.clone(), drop_index(same_parent(parent), index, true)),
                    Zone::Below => (
                        parent.clone(),
                        drop_index(same_parent(parent), index, false),
                    ),
                };
                actions.push(Action::Move {
                    id: dragged,
                    parent,
                    index,
                });
            }
        }

        if let Some(children) = shown_children {
            if !collapsed {
                let me = ParentRef::Node(node.id.clone());
                let derived = derived || instance;
                for (i, c) in children.iter().enumerate().rev() {
                    self.row(ui, ed, c, &me, i, depth + 1, derived, actions);
                }
            }
        }
    }
}

/// A small padlock: a body with a shackle that is open when unlocked.
fn padlock(painter: &egui::Painter, c: Pos2, locked: bool, color: egui::Color32) {
    let body = Rect::from_center_size(c + Vec2::new(0.0, 2.0), Vec2::new(8.0, 6.0));
    painter.rect_filled(body, CornerRadius::same(1), color);
    let top = body.min.y;
    let stroke = Stroke::new(1.2, color);
    let (l, r) = (c.x - 2.5, c.x + 2.5);
    painter.line_segment([Pos2::new(l, top), Pos2::new(l, top - 3.0)], stroke);
    painter.line_segment([Pos2::new(l, top - 3.0), Pos2::new(r, top - 3.0)], stroke);
    if locked {
        painter.line_segment([Pos2::new(r, top - 3.0), Pos2::new(r, top)], stroke);
    }
}

fn is_instance_id(ed: &Editor, id: &NodeId) -> bool {
    let kind = match id.is_derived() {
        true => ed.shown_node(id).map(|n| n.kind),
        false => ed.node(id).map(|n| n.kind.clone()),
    };
    matches!(kind, Some(NodeKind::Instance(_)))
}

fn kind_icon(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Frame(_) => "#",
        NodeKind::Group(_) => "G",
        NodeKind::Rect(_) => "□",
        NodeKind::Ellipse(_) => "○",
        NodeKind::Text(_) => "T",
        NodeKind::Image(_) => "I",
        NodeKind::Vector(_) => "V",
        NodeKind::Instance(_) => "◇",
    }
}
