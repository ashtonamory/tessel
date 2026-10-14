//! The styles panel (§3.6, §9): fill, stroke, text and effect styles,
//! grouped by the '/' in their names. A style can be created from the
//! selected layer's current values, renamed, edited, and deleted; deleting
//! one that layers use asks first and detaches them (their values are kept).
//! Every change is one command through the editor, so one undo step.

use doc::resolve::{
    detach_style_patch, effective_fills, effective_shadows, effective_stroke_width,
    effective_strokes, effective_text_style,
};
use doc::{
    ColorRef, Command, Document, Extra, Id, IdKind, Node, NodeKind, NodePatch, Paint, Style,
    StyleId, StyleKind, TokenId,
};
use egui::{Rect, Ui};

use crate::editor::Editor;
use crate::inspector::{fmt_num, parse_hex, to_hex};
use crate::pickers::{self, split_group, Fields, InlineEdit, StyleClass, Texts};

/// How many layers (in pages and component masters) use style `id`.
pub fn style_uses(doc: &Document, id: &StyleId) -> usize {
    let mut n = 0;
    let mut count = |node: &Node| n += detach_style_patch(doc, node, Some(id)).is_some() as usize;
    doc.walk_nodes(&mut count);
    for c in &doc.components {
        c.root.walk(&mut count);
    }
    n
}

/// Whether `node` can use a style of `class`.
pub fn accepts(node: &Node, class: StyleClass) -> bool {
    match class {
        StyleClass::Text => matches!(node.kind, NodeKind::Text(_)),
        _ => node.shape().is_some(),
    }
}

/// The patch that makes `node` use style `id` (`None`: no style) of `class`.
pub fn use_style_patch(class: StyleClass, id: Option<StyleId>) -> NodePatch {
    let mut p = NodePatch::default();
    match class {
        StyleClass::Fill => p.fill_style = Some(id),
        StyleClass::Stroke => p.stroke_style = Some(id),
        StyleClass::Text => p.text_style = Some(id),
        StyleClass::Effect => p.effect_style = Some(id),
    }
    p
}

/// The style of `class` that `node` uses.
pub fn style_of(node: &Node, class: StyleClass) -> Option<StyleId> {
    match (&node.kind, class) {
        (NodeKind::Text(t), StyleClass::Text) => t.text_style.clone(),
        (_, StyleClass::Text) => None,
        (_, c) => node.shape().and_then(|s| match c {
            StyleClass::Fill => s.fill_style.clone(),
            StyleClass::Stroke => s.stroke_style.clone(),
            _ => s.effect_style.clone(),
        }),
    }
}

/// The values of `node` as a style of `class`, as drawn right now.
fn kind_from(doc: &Document, node: &Node, class: StyleClass) -> Option<StyleKind> {
    if !accepts(node, class) {
        return None;
    }
    Some(match class {
        StyleClass::Fill => StyleKind::Fill(doc::FillStyle {
            fills: effective_fills(doc, node).to_vec(),
            extra: Extra::new(),
        }),
        StyleClass::Stroke => StyleKind::Stroke(doc::StrokeStyle {
            strokes: effective_strokes(doc, node).to_vec(),
            width: effective_stroke_width(doc, node),
            extra: Extra::new(),
        }),
        StyleClass::Text => StyleKind::Text(doc::TextStyleDef {
            style: effective_text_style(doc, node)?.clone(),
            extra: Extra::new(),
        }),
        StyleClass::Effect => StyleKind::Effect(doc::EffectStyle {
            shadows: effective_shadows(doc, node).to_vec(),
            extra: Extra::new(),
        }),
    })
}

fn fresh_name(doc: &Document, class: StyleClass) -> String {
    let base = format!("New {} style", class.key());
    let taken = |n: &str| doc.styles.iter().any(|s| s.name == n);
    let mut name = base.clone();
    let mut i = 2;
    while taken(&name) {
        name = format!("{base} {i}");
        i += 1;
    }
    name
}

/// Create a style of `class` from the first selected layer's values, and
/// make that layer use it, as one undo step.
pub fn create_style(ed: &mut Editor, class: StyleClass) -> Result<StyleId, String> {
    let node = ed
        .selection
        .first()
        .and_then(|id| ed.node(id))
        .ok_or("Select a layer to make a style from")?;
    let kind = kind_from(&ed.doc, node, class)
        .ok_or_else(|| format!("This layer can't have a {} style", class.key()))?;
    let id: StyleId = Id::new(IdKind::Style);
    let style = Style {
        id: id.clone(),
        name: fresh_name(&ed.doc, class),
        kind,
    };
    let cmds = vec![
        Command::UpsertStyle { style },
        Command::SetProps {
            id: node.id.clone(),
            patch: use_style_patch(class, Some(id.clone())),
        },
    ];
    let sel = ed.selection.clone();
    ed.commit("Create style", cmds, sel)
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Delete style `id`; layers that use it keep its values.
pub fn delete_style(ed: &mut Editor, id: &StyleId) -> Result<(), String> {
    let sel = ed.selection.clone();
    ed.commit(
        "Delete style",
        vec![Command::DeleteStyle {
            id: id.clone(),
            detach: true,
        }],
        sel,
    )
    .map_err(|e| e.to_string())
}

/// Replace style `new` (matched by id) as one undo step labelled `label`.
fn update_style(ed: &mut Editor, label: &str, new: Style) -> Result<(), String> {
    let sel = ed.selection.clone();
    ed.commit(label, vec![Command::UpsertStyle { style: new }], sel)
        .map_err(|e| e.to_string())
}

pub fn rename_style(ed: &mut Editor, id: &StyleId, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A style needs a name".into());
    }
    let mut style = doc::resolve::style(&ed.doc, id)
        .ok_or("No such style")?
        .clone();
    if style.name == name {
        return Ok(());
    }
    style.name = name.to_string();
    update_style(ed, "Rename style", style)
}

/// A colour typed into a style: a `{token}` that exists, or a hex colour.
fn parse_color(doc: &Document, text: &str) -> Result<ColorRef, String> {
    let text = text.trim();
    if let Some(t) = TokenId::from_reference(text) {
        return if doc.tokens.colors.contains_key(&t) {
            Ok(ColorRef::Token(t))
        } else {
            Err(format!("No color token named {t}"))
        };
    }
    let c = parse_hex(text).ok_or("Colors are written #rrggbb, #rrggbbaa or {color.name}")?;
    Ok(ColorRef::literal(&to_hex(c)))
}

fn parse_number(text: &str, min: f32) -> Result<f32, String> {
    match text.trim().parse::<f32>() {
        Ok(v) if v.is_finite() && v >= min => Ok(v),
        _ => Err(format!("Enter a number of {min} or more")),
    }
}

/// The first solid paint's colour, as shown in a style row.
fn first_color(paints: &[Paint]) -> String {
    match paints.first() {
        Some(Paint::Solid(s)) => match &s.color {
            ColorRef::Literal(c) => c.0.clone(),
            ColorRef::Token(t) => t.to_reference(),
        },
        Some(Paint::Image(_)) => "image".into(),
        Some(_) => "gradient".into(),
        None => "none".into(),
    }
}

/// Set the first paint to a solid `color`, or add one.
fn set_first_color(paints: &mut Vec<Paint>, color: ColorRef) {
    let paint = Paint::solid(color);
    match paints.first_mut() {
        Some(p) => *p = paint,
        None => paints.push(paint),
    }
}

/// One editable value of a style row: its key suffix, label, current text,
/// width, and how typed text changes the style.
struct Cell {
    key: &'static str,
    label: &'static str,
    text: String,
    width: f32,
    apply: fn(&Document, &mut StyleKind, &str) -> Result<(), String>,
}

fn cells(style: &Style) -> Vec<Cell> {
    match &style.kind {
        StyleKind::Fill(f) => vec![Cell {
            key: "color",
            label: "",
            text: first_color(&f.fills),
            width: 90.0,
            apply: |doc, k, t| {
                let c = parse_color(doc, t)?;
                if let StyleKind::Fill(f) = k {
                    set_first_color(&mut f.fills, c);
                }
                Ok(())
            },
        }],
        StyleKind::Stroke(s) => vec![
            Cell {
                key: "color",
                label: "",
                text: first_color(&s.strokes),
                width: 90.0,
                apply: |doc, k, t| {
                    let c = parse_color(doc, t)?;
                    if let StyleKind::Stroke(s) = k {
                        set_first_color(&mut s.strokes, c);
                    }
                    Ok(())
                },
            },
            Cell {
                key: "width",
                label: "W",
                text: fmt_num(s.width, 2),
                width: 40.0,
                apply: |_, k, t| {
                    if let StyleKind::Stroke(s) = k {
                        s.width = parse_number(t, 0.0)?;
                    }
                    Ok(())
                },
            },
        ],
        StyleKind::Text(t) => vec![
            Cell {
                key: "family",
                label: "",
                text: t.style.font_family.clone(),
                width: 90.0,
                apply: |_, k, t| {
                    let t = t.trim();
                    if t.is_empty() {
                        return Err("Enter a font family".into());
                    }
                    if let StyleKind::Text(d) = k {
                        d.style.font_family = t.to_string();
                    }
                    Ok(())
                },
            },
            Cell {
                key: "size",
                label: "Size",
                text: fmt_num(t.style.font_size, 2),
                width: 40.0,
                apply: |_, k, t| {
                    if let StyleKind::Text(d) = k {
                        d.style.font_size = parse_number(t, 1.0)?;
                    }
                    Ok(())
                },
            },
            Cell {
                key: "weight",
                label: "Wt",
                text: t.style.font_weight.to_string(),
                width: 40.0,
                apply: |_, k, t| {
                    let w = parse_number(t, 100.0)?.min(900.0);
                    if let StyleKind::Text(d) = k {
                        d.style.font_weight = ((w / 100.0).round() * 100.0) as u16;
                    }
                    Ok(())
                },
            },
        ],
        StyleKind::Effect(e) => {
            let s = e.shadows.first();
            vec![
                Cell {
                    key: "y",
                    label: "Y",
                    text: s.map_or("-".into(), |s| fmt_num(s.y, 2)),
                    width: 36.0,
                    apply: |_, k, t| {
                        let v = parse_number(t, f32::MIN)?;
                        if let StyleKind::Effect(e) = k {
                            if let Some(s) = e.shadows.first_mut() {
                                s.y = v;
                            }
                        }
                        Ok(())
                    },
                },
                Cell {
                    key: "blur",
                    label: "B",
                    text: s.map_or("-".into(), |s| fmt_num(s.blur, 2)),
                    width: 36.0,
                    apply: |_, k, t| {
                        let v = parse_number(t, 0.0)?;
                        if let StyleKind::Effect(e) = k {
                            if let Some(s) = e.shadows.first_mut() {
                                s.blur = v;
                            }
                        }
                        Ok(())
                    },
                },
                Cell {
                    key: "color",
                    label: "",
                    text: s.map_or("-".into(), |s| match &s.color {
                        ColorRef::Literal(c) => c.0.clone(),
                        ColorRef::Token(t) => t.to_reference(),
                    }),
                    width: 76.0,
                    apply: |doc, k, t| {
                        let c = parse_color(doc, t)?;
                        if let StyleKind::Effect(e) = k {
                            if let Some(s) = e.shadows.first_mut() {
                                s.color = c;
                            }
                        }
                        Ok(())
                    },
                },
            ]
        }
    }
}

enum Action {
    Create(StyleClass),
    Rename(StyleId, String),
    Edit(StyleId, usize, String),
    /// Delete: straight away if unused, else after confirming.
    Delete(StyleId),
    ConfirmDelete(StyleId),
    CancelDelete,
}

#[derive(Default)]
pub struct StylesPanel {
    edit: InlineEdit,
    fields: Fields,
    texts: Texts,
    /// A style whose deletion is waiting for a yes, with the number of
    /// layers it would detach from.
    confirm: Option<(StyleId, usize)>,
    notice: Option<String>,
}

impl StylesPanel {
    /// Where widget `key` was drawn last frame, in screen points. Keys are
    /// `new.<class>`, `<style id>.name`, `<style id>.<cell>`,
    /// `<style id>.delete`, `confirm.ok` and `confirm.cancel`.
    pub fn field_rect(&self, key: &str) -> Option<Rect> {
        pickers::find_field(&self.fields, key)
    }

    pub fn field_text(&self, key: &str) -> Option<&str> {
        pickers::find_text(&self.texts, key)
    }

    pub fn is_editing(&self) -> bool {
        self.edit.is_active()
    }

    /// The style whose delete is waiting to be confirmed, and how many
    /// layers it would detach from.
    pub fn pending_delete(&self) -> Option<(&StyleId, usize)> {
        self.confirm.as_ref().map(|(id, n)| (id, *n))
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn ui(&mut self, ui: &mut Ui, ed: &mut Editor) {
        self.fields.clear();
        self.texts.clear();
        if let Some((id, _)) = &self.confirm {
            if doc::resolve::style(&ed.doc, id).is_none() {
                self.confirm = None;
            }
        }
        let mut actions = Vec::new();
        ui.add_space(4.0);
        if let Some(n) = &self.notice {
            ui.colored_label(ui.visuals().error_fg_color, n);
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if ed.read_only {
                    ui.disable();
                }
                for class in StyleClass::ALL {
                    self.section(ui, ed, class, &mut actions);
                }
            });
        for a in actions {
            self.run(ed, a);
        }
    }

    fn section(&mut self, ui: &mut Ui, ed: &Editor, class: StyleClass, actions: &mut Vec<Action>) {
        let can_make = ed
            .selection
            .first()
            .and_then(|id| ed.node(id))
            .is_some_and(|n| accepts(n, class));
        ui.horizontal(|ui| {
            ui.strong(format!("{} styles", class.label()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = ui
                    .add_enabled(can_make, egui::Button::new("+"))
                    .on_hover_text("New style from the selected layer");
                self.fields
                    .push((format!("new.{}", class.key()), resp.rect));
                if resp.clicked() {
                    actions.push(Action::Create(class));
                }
            });
        });
        let mut styles: Vec<&Style> = ed
            .doc
            .styles
            .iter()
            .filter(|s| StyleClass::of(&s.kind) == class)
            .collect();
        styles.sort_by(|a, b| a.name.cmp(&b.name));
        if styles.is_empty() {
            ui.weak("None yet");
        }
        let mut group = "";
        for s in styles {
            let (g, _) = split_group(&s.name);
            if g != group {
                group = g;
                if !g.is_empty() {
                    ui.weak(g);
                }
            }
            self.row(ui, s, !g.is_empty(), actions);
        }
        ui.add_space(6.0);
    }

    fn row(&mut self, ui: &mut Ui, s: &Style, grouped: bool, actions: &mut Vec<Action>) {
        let id = s.id.to_string();
        ui.horizontal(|ui| {
            if grouped {
                ui.add_space(10.0);
            }
            if let Some(text) = self.edit.show(
                ui,
                &mut self.fields,
                &mut self.texts,
                &format!("{id}.name"),
                &s.name,
                120.0,
            ) {
                actions.push(Action::Rename(s.id.clone(), text));
            }
            for (i, cell) in cells(s).into_iter().enumerate() {
                if !cell.label.is_empty() {
                    ui.label(cell.label);
                }
                if let Some(text) = self.edit.show(
                    ui,
                    &mut self.fields,
                    &mut self.texts,
                    &format!("{id}.{}", cell.key),
                    &cell.text,
                    cell.width,
                ) {
                    actions.push(Action::Edit(s.id.clone(), i, text));
                }
            }
            let resp = ui.add(egui::Button::new("✕").small());
            self.fields.push((format!("{id}.delete"), resp.rect));
            if resp.clicked() {
                actions.push(Action::Delete(s.id.clone()));
            }
        });
        if let Some((cid, n)) = self.confirm.as_ref().filter(|(c, _)| *c == s.id) {
            let plural = if *n == 1 { "" } else { "s" };
            ui.horizontal(|ui| {
                ui.label(format!("Detach from {n} layer{plural} and delete?"));
            });
            ui.horizontal(|ui| {
                let ok = ui.button("Detach and delete");
                self.fields.push(("confirm.ok".into(), ok.rect));
                if ok.clicked() {
                    actions.push(Action::ConfirmDelete(cid.clone()));
                }
                let cancel = ui.button("Cancel");
                self.fields.push(("confirm.cancel".into(), cancel.rect));
                if cancel.clicked() {
                    actions.push(Action::CancelDelete);
                }
            });
        }
    }

    fn run(&mut self, ed: &mut Editor, action: Action) {
        self.notice = None;
        let result = match action {
            Action::Create(class) => create_style(ed, class).map(|_| ()),
            Action::Rename(id, name) => rename_style(ed, &id, &name),
            Action::Edit(id, cell, text) => self.edit_cell(ed, &id, cell, &text),
            Action::Delete(id) => {
                let uses = style_uses(&ed.doc, &id);
                if uses == 0 {
                    self.confirm = None;
                    delete_style(ed, &id)
                } else {
                    self.confirm = Some((id, uses));
                    Ok(())
                }
            }
            Action::ConfirmDelete(id) => {
                self.confirm = None;
                delete_style(ed, &id)
            }
            Action::CancelDelete => {
                self.confirm = None;
                Ok(())
            }
        };
        if let Err(msg) = result {
            ed.status = Some(msg.clone());
            self.notice = Some(msg);
        }
    }

    fn edit_cell(
        &mut self,
        ed: &mut Editor,
        id: &StyleId,
        cell: usize,
        text: &str,
    ) -> Result<(), String> {
        let mut style = doc::resolve::style(&ed.doc, id)
            .ok_or("No such style")?
            .clone();
        let apply = cells(&style).get(cell).ok_or("No such field")?.apply;
        apply(&ed.doc, &mut style.kind, text)?;
        update_style(ed, "Edit style", style)
    }
}
