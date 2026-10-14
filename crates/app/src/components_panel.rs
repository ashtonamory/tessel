//! The Components tab of the left panel (§9 "components list"): the
//! document's components grouped by the `/` in their names. Click a
//! component to place an instance in the middle of the canvas, or drag it
//! onto the canvas; the row buttons rename it, open its master on the
//! Components page, and delete it (asking first when instances would be
//! detached). Also the bar above the canvas while a master is edited.

use std::collections::{BTreeMap, HashSet};

use doc::expand::component;
use doc::ComponentId;
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, TextEdit, Ui, Vec2};
use kurbo::Point;

use crate::canvas::{Canvas, View, SELECTION};
use crate::components::short_name;
use crate::editor::Editor;

const ROW: f32 = 22.0;
const INDENT: f32 = 14.0;

/// What is dragged from the components list onto the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentDrag(pub ComponentId);

/// The small buttons at the right end of a component row, right to left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ComponentButton {
    Delete,
    Edit,
    Rename,
}

impl ComponentButton {
    pub const ALL: [ComponentButton; 3] = [
        ComponentButton::Rename,
        ComponentButton::Edit,
        ComponentButton::Delete,
    ];

    /// Where the button sits in a component row.
    pub fn rect(self, row: Rect) -> Rect {
        let slot = match self {
            ComponentButton::Delete => 0.0,
            ComponentButton::Edit => 1.0,
            ComponentButton::Rename => 2.0,
        };
        Rect::from_center_size(
            Pos2::new(row.max.x - 14.0 - slot * 20.0, row.center().y),
            Vec2::splat(18.0),
        )
    }

    fn glyph(self) -> &'static str {
        match self {
            ComponentButton::Delete => "×",
            ComponentButton::Edit => "◆",
            ComponentButton::Rename => "✎",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            ComponentButton::Delete => "Delete component",
            ComponentButton::Edit => "Edit component",
            ComponentButton::Rename => "Rename",
        }
    }
}

enum Action {
    Insert(ComponentId),
    Edit(ComponentId),
    Rename(ComponentId, String),
    Delete(ComponentId),
}

struct Renaming {
    id: ComponentId,
    buffer: String,
    focused: bool,
}

/// Components grouped by name: `Button/Primary` sits in group `Button`.
#[derive(Default)]
struct Group {
    groups: BTreeMap<String, Group>,
    /// `(id, full name)`, in document order.
    items: Vec<(ComponentId, String)>,
}

impl Group {
    fn of(ed: &Editor) -> Group {
        let mut root = Group::default();
        for c in &ed.document().components {
            let segments: Vec<&str> = c.name.split('/').map(str::trim).collect();
            let mut g = &mut root;
            for s in &segments[..segments.len() - 1] {
                g = g.groups.entry(s.to_string()).or_default();
            }
            g.items.push((c.id.clone(), c.name.clone()));
        }
        root
    }
}

#[derive(Default)]
pub struct ComponentsPanel {
    renaming: Option<Renaming>,
    /// Groups the user closed, by path ("Button/Icon").
    collapsed: HashSet<String>,
    rows: Vec<(ComponentId, Rect)>,
    group_rows: Vec<(String, Rect)>,
    /// A used component waiting for "Detach and delete".
    confirm_delete: Option<ComponentId>,
    /// The component being edited last frame, and the page view to go
    /// back to.
    shown: Option<ComponentId>,
    page_view: Option<View>,
}

impl ComponentsPanel {
    /// Component rows drawn last frame, in screen points.
    pub fn rows(&self) -> &[(ComponentId, Rect)] {
        &self.rows
    }

    /// Group header rows drawn last frame, by path.
    pub fn group_rows(&self) -> &[(String, Rect)] {
        &self.group_rows
    }

    pub fn is_renaming(&self) -> bool {
        self.renaming.is_some()
    }

    /// The component waiting for the delete confirmation.
    pub fn pending_delete(&self) -> Option<&ComponentId> {
        self.confirm_delete.as_ref()
    }

    /// Answer the delete confirmation: detach its instances and delete
    /// (`true`), or keep it.
    pub fn answer_delete(&mut self, ed: &mut Editor, delete: bool) {
        if let Some(id) = self.confirm_delete.take() {
            if delete {
                ed.delete_component(&id, true);
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, ed: &mut Editor, view: kurbo::Rect) {
        let mut actions = Vec::new();
        self.rows.clear();
        self.group_rows.clear();
        let tree = Group::of(ed);
        egui::ScrollArea::vertical()
            .id_salt("components")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if ed.document().components.is_empty() {
                    ui.weak("No components yet. Select a frame or layers and press Ctrl+Alt+K.");
                }
                self.group(ui, ed, &tree, "", 0, &mut actions);
            });
        for a in actions {
            match a {
                Action::Insert(id) => {
                    ed.insert_instance(&id, None, view);
                }
                Action::Edit(id) => {
                    ed.edit_component(&id);
                }
                Action::Rename(id, name) => ed.rename_component(&id, name),
                Action::Delete(id) => self.delete(ed, id),
            }
        }
    }

    /// Delete at once when nothing uses the component; else ask.
    fn delete(&mut self, ed: &mut Editor, id: ComponentId) {
        if ed.component_users(&id) == 0 {
            ed.delete_component(&id, false);
        } else {
            self.confirm_delete = Some(id);
        }
    }

    fn group(
        &mut self,
        ui: &mut Ui,
        ed: &Editor,
        g: &Group,
        path: &str,
        depth: usize,
        actions: &mut Vec<Action>,
    ) {
        for (name, sub) in &g.groups {
            let sub_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
            self.group_rows.push((sub_path.clone(), rect));
            let open = !self.collapsed.contains(&sub_path);
            let painter = ui.painter_at(rect);
            let color = ui.visuals().strong_text_color();
            let x = rect.min.x + 4.0 + depth as f32 * INDENT;
            painter.text(
                Pos2::new(x + 6.0, rect.center().y),
                Align2::CENTER_CENTER,
                if open { "▾" } else { "▸" },
                FontId::proportional(12.0),
                color,
            );
            painter.text(
                Pos2::new(x + 16.0, rect.center().y),
                Align2::LEFT_CENTER,
                name,
                FontId::proportional(13.0),
                color,
            );
            if resp.clicked() && !self.collapsed.remove(&sub_path) {
                self.collapsed.insert(sub_path.clone());
            }
            if open {
                self.group(ui, ed, sub, &sub_path, depth + 1, actions);
            }
        }
        for (id, name) in &g.items {
            self.row(ui, ed, id, name, depth, actions);
        }
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        ed: &Editor,
        id: &ComponentId,
        name: &str,
        depth: usize,
        actions: &mut Vec<Action>,
    ) {
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), ROW),
            Sense::click_and_drag(),
        );
        self.rows.push((id.clone(), rect));
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        if ed.editing_component() == Some(id) {
            painter.rect_filled(rect, CornerRadius::ZERO, SELECTION.gamma_multiply(0.35));
        } else if resp.hovered() {
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                visuals.widgets.hovered.weak_bg_fill,
            );
        }
        let x = rect.min.x + 4.0 + depth as f32 * INDENT;
        painter.text(
            Pos2::new(x + 6.0, rect.center().y),
            Align2::CENTER_CENTER,
            "❖",
            FontId::proportional(11.0),
            SELECTION,
        );
        let name_rect = Rect::from_min_max(
            Pos2::new(x + 16.0, rect.min.y + 1.0),
            Pos2::new(
                ComponentButton::Rename.rect(rect).min.x - 4.0,
                rect.max.y - 1.0,
            ),
        );
        if self.renaming.as_ref().is_some_and(|r| &r.id == id) {
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
                let name: String = r
                    .buffer
                    .split('/')
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("/");
                if !cancelled && !short_name(&name).is_empty() {
                    actions.push(Action::Rename(r.id, name));
                }
            }
        } else {
            let color = visuals.text_color();
            let galley = painter.layout_no_wrap(
                short_name(name).to_string(),
                FontId::proportional(13.0),
                color,
            );
            let pos = Pos2::new(name_rect.min.x, rect.center().y - galley.size().y / 2.0);
            painter.with_clip_rect(name_rect).galley(pos, galley, color);
        }
        let editable = !ed.read_only;
        if editable && (resp.hovered() || ed.editing_component() == Some(id)) {
            for b in ComponentButton::ALL {
                painter.text(
                    b.rect(rect).center(),
                    Align2::CENTER_CENTER,
                    b.glyph(),
                    FontId::proportional(12.0),
                    visuals.text_color(),
                );
            }
        }
        let hint = match resp.hover_pos() {
            Some(p) if editable => ComponentButton::ALL
                .into_iter()
                .find(|b| b.rect(rect).contains(p))
                .map_or(
                    "Click to place an instance, or drag it onto the canvas",
                    |b| b.hint(),
                ),
            _ => name,
        };
        let resp = resp.on_hover_text(hint);
        if resp.clicked() && editable {
            let p = resp.interact_pointer_pos().unwrap_or(rect.center());
            match ComponentButton::ALL
                .into_iter()
                .find(|b| b.rect(rect).contains(p))
            {
                Some(ComponentButton::Rename) => {
                    self.renaming = Some(Renaming {
                        id: id.clone(),
                        buffer: name.to_string(),
                        focused: false,
                    })
                }
                Some(ComponentButton::Edit) => actions.push(Action::Edit(id.clone())),
                Some(ComponentButton::Delete) => actions.push(Action::Delete(id.clone())),
                None => actions.push(Action::Insert(id.clone())),
            }
        }
        if resp.dragged() && editable {
            resp.dnd_set_drag_payload(ComponentDrag(id.clone()));
        }
    }

    /// The bar above the canvas while a master is edited: what is being
    /// edited, and the way back.
    pub fn master_bar(&self, ui: &mut Ui, ed: &mut Editor) {
        let Some(id) = ed.editing_component().cloned() else {
            return;
        };
        let name = component(ed.document(), &id).map_or(String::new(), |c| c.name.clone());
        let back = ed.document_page().name.clone();
        let mut leave = false;
        egui::Frame::new()
            .fill(SELECTION.gamma_multiply(0.25))
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("❖").color(SELECTION));
                    ui.strong(format!("Editing component {name}"));
                    ui.weak("Changes apply to every instance.");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        leave = ui
                            .button(format!("Done: back to {back}"))
                            .on_hover_text("Leave the component (Esc with nothing selected)")
                            .clicked();
                    });
                });
            });
        if leave {
            ed.leave_component();
        }
    }

    /// Fit the master on entering its page, and restore the page view on
    /// leaving. Called every frame before the canvas.
    pub fn sync_view(&mut self, canvas: &mut Canvas, ed: &Editor) {
        let now = ed.editing_component().cloned();
        if now == self.shown {
            return;
        }
        match (&self.shown, &now) {
            (None, Some(_)) => {
                self.page_view = Some(canvas.view);
                canvas.fit_pending = true;
            }
            (Some(_), Some(_)) => canvas.fit_pending = true,
            (Some(_), None) => {
                if let Some(v) = self.page_view.take() {
                    canvas.view = v;
                }
            }
            (None, None) => {}
        }
        self.shown = now;
    }

    /// The delete confirmation, when a used component is being deleted.
    pub fn dialogs(&mut self, ctx: &egui::Context, ed: &mut Editor) {
        let Some(id) = self.confirm_delete.clone() else {
            return;
        };
        let name = component(ed.document(), &id).map_or(String::new(), |c| c.name.clone());
        let users = ed.component_users(&id);
        let mut choice = None;
        egui::Modal::new(egui::Id::new("delete-component")).show(ctx, |ui| {
            ui.heading("Delete component?");
            ui.label(format!(
                "{name} is used {users} time(s). Its instances will be detached into plain \
                 frames, which keep how they look."
            ));
            ui.horizontal(|ui| {
                if ui.button("Detach and delete").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        if let Some(delete) = choice {
            self.answer_delete(ed, delete);
        }
    }
}

/// Place an instance where a component was dropped on the canvas.
pub fn drop_on_canvas(ed: &mut Editor, drag: &ComponentDrag, at: Point, view: kurbo::Rect) {
    ed.insert_instance(&drag.0, Some(at), view);
}
