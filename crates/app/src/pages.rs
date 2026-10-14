//! Pages (§9): the editor commands for adding, renaming, deleting and
//! reordering pages, and the list above the layers tree.

use doc::{Command, Id, IdKind, Page, PageId};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, TextEdit, Ui, Vec2};

use crate::canvas::SELECTION;
use crate::editor::Editor;

impl Editor {
    fn page_command(&mut self, label: &str, cmd: Command) -> bool {
        let sel = self.selection.clone();
        self.commit(label, vec![cmd], sel).is_ok()
    }

    /// A default name such as "Page 3" that no page uses.
    fn next_page_name(&self) -> String {
        let mut n = self.doc.pages.len() + 1;
        while self.doc.pages.iter().any(|p| p.name == format!("Page {n}")) {
            n += 1;
        }
        format!("Page {n}")
    }

    /// Add an empty page after the current one and switch to it.
    pub fn add_page(&mut self) -> Option<PageId> {
        let page = Page::new(Id::new(IdKind::Page), self.next_page_name());
        let id = page.id.clone();
        let index = self.current_page_index() + 1;
        let cmd = Command::CreatePage {
            index: Some(index),
            page,
        };
        if !self.page_command("Add page", cmd) {
            return None;
        }
        self.switch_page(&id);
        Some(id)
    }

    pub fn rename_page(&mut self, id: &PageId, name: String) {
        if self.doc.page(id).is_none_or(|p| p.name == name) {
            return;
        }
        self.page_command(
            "Rename page",
            Command::UpdatePage {
                id: id.clone(),
                name: Some(name),
                background: None,
            },
        );
    }

    /// Delete a page with its layers. The last page cannot be deleted.
    pub fn delete_page(&mut self, id: &PageId) {
        if self.doc.pages.len() < 2 {
            self.status = Some("A document needs at least one page".into());
            return;
        }
        let Some(at) = self.doc.page_index(id) else {
            return;
        };
        let was_current = self.page().id == *id;
        if !self.page_command("Delete page", Command::DeletePage { id: id.clone() }) {
            return;
        }
        if was_current {
            // Show the page that took its place.
            let next = self.doc.pages[at.min(self.doc.pages.len() - 1)].id.clone();
            self.current_page = Some(next);
            self.selection.clear();
            self.scene_dirty = crate::editor::SceneDirty::All;
        }
    }

    /// Move a page to position `to` in the page list. A page has no
    /// ordering command, so this deletes and re-creates it in one
    /// transaction.
    pub fn move_page(&mut self, id: &PageId, to: usize) {
        let Some(at) = self.doc.page_index(id) else {
            return;
        };
        let to = to.min(self.doc.pages.len() - 1);
        if at == to {
            return;
        }
        let page = self.doc.pages[at].clone();
        let cmd = Command::Batch(vec![
            Command::DeletePage { id: id.clone() },
            Command::CreatePage {
                index: Some(to),
                page,
            },
        ]);
        self.page_command("Reorder page", cmd);
    }
}

const ROW: f32 = 22.0;
/// The small buttons at the right end of the current page's row, right to
/// left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PageButton {
    Delete,
    Down,
    Up,
}

impl PageButton {
    /// Where the button sits in a page row.
    pub fn rect(self, row: Rect) -> Rect {
        let slot = match self {
            PageButton::Delete => 0.0,
            PageButton::Down => 1.0,
            PageButton::Up => 2.0,
        };
        Rect::from_center_size(
            Pos2::new(row.max.x - 14.0 - slot * 20.0, row.center().y),
            Vec2::splat(18.0),
        )
    }
}

enum Action {
    Switch(PageId),
    Add,
    Rename(PageId, String),
    Delete(PageId),
    Move(PageId, usize),
}

struct Renaming {
    id: PageId,
    buffer: String,
    focused: bool,
}

#[derive(Default)]
pub struct PagesPanel {
    renaming: Option<Renaming>,
    rows: Vec<(PageId, Rect)>,
    add: Option<Rect>,
}

impl PagesPanel {
    /// Page rows drawn last frame, in screen points.
    pub fn rows(&self) -> &[(PageId, Rect)] {
        &self.rows
    }

    /// The "add page" button drawn last frame.
    pub fn add_button(&self) -> Option<Rect> {
        self.add
    }

    pub fn is_renaming(&self) -> bool {
        self.renaming.is_some()
    }

    pub fn ui(&mut self, ui: &mut Ui, ed: &mut Editor) {
        let mut actions = Vec::new();
        self.rows.clear();
        let (header, hresp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
        ui.painter_at(header).text(
            Pos2::new(header.min.x + 4.0, header.center().y),
            Align2::LEFT_CENTER,
            "Pages",
            FontId::proportional(13.0),
            ui.visuals().strong_text_color(),
        );
        let add = Rect::from_center_size(
            Pos2::new(header.max.x - 14.0, header.center().y),
            Vec2::splat(18.0),
        );
        self.add = Some(add);
        glyph(ui, add, "+");
        if hresp.clicked()
            && !ed.read_only
            && hresp
                .interact_pointer_pos()
                .is_some_and(|p| add.contains(p))
        {
            actions.push(Action::Add);
        }
        let current = ed.page().id.clone();
        let count = ed.doc.pages.len();
        egui::ScrollArea::vertical()
            .id_salt("pages")
            .max_height(ROW * 6.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, page) in ed.doc.pages.iter().enumerate() {
                    self.row(ui, ed, page, i, count, page.id == current, &mut actions);
                }
            });
        ui.separator();
        for a in actions {
            match a {
                Action::Switch(id) => ed.switch_page(&id),
                Action::Add => {
                    ed.add_page();
                }
                Action::Rename(id, name) => ed.rename_page(&id, name),
                Action::Delete(id) => ed.delete_page(&id),
                Action::Move(id, to) => ed.move_page(&id, to),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut Ui,
        ed: &Editor,
        page: &Page,
        index: usize,
        count: usize,
        current: bool,
        actions: &mut Vec<Action>,
    ) {
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
        self.rows.push((page.id.clone(), rect));
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        if current {
            painter.rect_filled(rect, CornerRadius::ZERO, SELECTION.gamma_multiply(0.35));
        } else if resp.hovered() {
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                visuals.widgets.hovered.weak_bg_fill,
            );
        }
        let buttons_w = if current { 3.0 * 20.0 + 4.0 } else { 4.0 };
        let name_rect = Rect::from_min_max(
            Pos2::new(rect.min.x + 12.0, rect.min.y + 1.0),
            Pos2::new(rect.max.x - buttons_w, rect.max.y - 1.0),
        );
        if self.renaming.as_ref().is_some_and(|r| r.id == page.id) {
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
            let color = visuals.text_color();
            let galley =
                painter.layout_no_wrap(page.name.clone(), FontId::proportional(13.0), color);
            let pos = Pos2::new(name_rect.min.x, rect.center().y - galley.size().y / 2.0);
            painter.with_clip_rect(name_rect).galley(pos, galley, color);
        }
        if current && !ed.read_only {
            let buttons = [
                (PageButton::Up, "▲", index > 0),
                (PageButton::Down, "▼", index + 1 < count),
                (PageButton::Delete, "×", count > 1),
            ];
            for (b, text, enabled) in buttons {
                if enabled {
                    glyph(ui, b.rect(rect), text);
                }
            }
            if resp.clicked() {
                let p = resp.interact_pointer_pos().unwrap_or(rect.center());
                for (b, _, enabled) in buttons {
                    if enabled && b.rect(rect).contains(p) {
                        actions.push(match b {
                            PageButton::Up => Action::Move(page.id.clone(), index - 1),
                            PageButton::Down => Action::Move(page.id.clone(), index + 1),
                            PageButton::Delete => Action::Delete(page.id.clone()),
                        });
                        return;
                    }
                }
            }
        }
        if resp.clicked() && !current {
            actions.push(Action::Switch(page.id.clone()));
        }
        if resp.double_clicked() && !ed.read_only {
            let p = resp.interact_pointer_pos().unwrap_or(rect.center());
            if name_rect.contains(p) {
                self.renaming = Some(Renaming {
                    id: page.id.clone(),
                    buffer: page.name.clone(),
                    focused: false,
                });
            }
        }
    }
}

fn glyph(ui: &Ui, rect: Rect, text: &str) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
}
