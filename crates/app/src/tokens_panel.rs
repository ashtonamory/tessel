//! The tokens panel (§3.6, §9): colours, spacing, radii and fonts of the
//! document, with add, edit, rename and delete. Every change is one
//! `UpsertTokens` or `RenameToken` command through the editor, so one undo
//! step; a rename rewrites every reference, and a token that is still
//! referenced cannot be deleted.

use std::collections::BTreeMap;

use doc::{validate, Color, Command, CommandError, Document, NodeId, TokenId, Tokens, TokensPatch};
use egui::color_picker::{color_edit_button_hsva, Alpha};
use egui::ecolor::Hsva;
use egui::{Rect, Ui};

use crate::editor::Editor;
use crate::inspector::{fmt_num, parse_hex, to_hex};
use crate::pickers::{self, Fields, InlineEdit, Texts};

/// The four token groups, in the order the panel lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Color,
    Space,
    Radius,
    Font,
}

impl TokenKind {
    pub const ALL: [TokenKind; 4] = [
        TokenKind::Color,
        TokenKind::Space,
        TokenKind::Radius,
        TokenKind::Font,
    ];

    pub fn title(self) -> &'static str {
        match self {
            TokenKind::Color => "Colors",
            TokenKind::Space => "Spacing",
            TokenKind::Radius => "Radii",
            TokenKind::Font => "Fonts",
        }
    }

    /// The dotted prefix of new names, and the widget key.
    pub fn prefix(self) -> &'static str {
        match self {
            TokenKind::Color => "color",
            TokenKind::Space => "space",
            TokenKind::Radius => "radius",
            TokenKind::Font => "font",
        }
    }

    /// A patch that sets token `name` to its default value.
    fn default_patch(self, name: TokenId) -> TokensPatch {
        let mut p = TokensPatch::default();
        match self {
            TokenKind::Color => {
                p.colors.insert(name, Some(Color::new("#808080")));
            }
            TokenKind::Space => {
                p.spacing.insert(name, Some(8.0));
            }
            TokenKind::Radius => {
                p.radii.insert(name, Some(4.0));
            }
            TokenKind::Font => {
                p.fonts.insert(name, Some("Inter".to_string()));
            }
        }
        p
    }
}

/// Every token name in the document.
fn all_names(t: &Tokens) -> impl Iterator<Item = &TokenId> {
    t.colors
        .keys()
        .chain(t.spacing.keys())
        .chain(t.radii.keys())
        .chain(t.fonts.keys())
}

/// A valid new name for a token: dotted words (`color.primary`, `space.4`)
/// of letters, digits, `-` and `_`, not used by another token.
pub fn check_name(tokens: &Tokens, name: &str) -> Result<TokenId, String> {
    let name = name.trim();
    let parts: Vec<&str> = name.split('.').collect();
    let word = |p: &&str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    };
    if parts.len() < 2 || !parts.iter().all(word) {
        return Err(format!(
            "“{name}” is not a token name: use dotted words like color.primary"
        ));
    }
    let id = TokenId(name.to_string());
    if all_names(tokens).any(|n| *n == id) {
        return Err(format!("A token named {name} already exists"));
    }
    Ok(id)
}

/// The first `prefix.new`, `prefix.new-2`, … that no token uses.
fn fresh_name(tokens: &Tokens, prefix: &str) -> TokenId {
    let taken = |n: &str| all_names(tokens).any(|t| t.0 == n);
    let mut name = format!("{prefix}.new");
    let mut i = 2;
    while taken(&name) {
        name = format!("{prefix}.new-{i}");
        i += 1;
    }
    TokenId(name)
}

/// How many places still refer to token `name`: what deleting it would
/// leave dangling.
pub fn token_uses(doc: &Document, name: &TokenId) -> usize {
    let mut patch = TokensPatch::default();
    patch.colors.insert(name.clone(), None);
    patch.spacing.insert(name.clone(), None);
    patch.radii.insert(name.clone(), None);
    patch.fonts.insert(name.clone(), None);
    let before = validate(doc);
    let mut trial = doc.clone();
    match (Command::UpsertTokens { patch }).apply(&mut trial) {
        Err(CommandError::Invariant(ps)) => ps.iter().filter(|p| !before.contains(p)).count(),
        _ => 0,
    }
}

/// Add a token of `kind` with a default value and a fresh name.
pub fn add_token(ed: &mut Editor, kind: TokenKind) -> Option<TokenId> {
    let name = fresh_name(&ed.doc.tokens, kind.prefix());
    let patch = kind.default_patch(name.clone());
    let sel = ed.selection.clone();
    ed.commit("Add token", vec![Command::UpsertTokens { patch }], sel)
        .ok()
        .map(|_| name)
}

/// Rename `from` to `to`, rewriting every reference.
pub fn rename_token(ed: &mut Editor, from: &TokenId, to: &str) -> Result<(), String> {
    if from.0 == to.trim() {
        return Ok(());
    }
    let to = check_name(&ed.doc.tokens, to)?;
    let sel = ed.selection.clone();
    ed.commit(
        "Rename token",
        vec![Command::RenameToken {
            from: from.clone(),
            to,
        }],
        sel,
    )
    .map_err(|e| e.to_string())
}

/// Delete `name`, unless something still refers to it.
pub fn delete_token(ed: &mut Editor, name: &TokenId) -> Result<(), String> {
    let uses = token_uses(&ed.doc, name);
    if uses > 0 {
        let plural = if uses == 1 { "" } else { "s" };
        return Err(format!(
            "Can't delete {name}: {uses} reference{plural} still use it. Unlink them first."
        ));
    }
    let mut patch = TokensPatch::default();
    patch.colors.insert(name.clone(), None);
    patch.spacing.insert(name.clone(), None);
    patch.radii.insert(name.clone(), None);
    patch.fonts.insert(name.clone(), None);
    let sel = ed.selection.clone();
    ed.commit("Delete token", vec![Command::UpsertTokens { patch }], sel)
        .map_err(|e| e.to_string())
}

/// A patch that sets token `name` of `kind` from typed `text`.
fn value_patch(kind: TokenKind, name: &TokenId, text: &str) -> Result<TokensPatch, String> {
    let mut p = TokensPatch::default();
    let text = text.trim();
    match kind {
        TokenKind::Color => {
            let c = parse_hex(text).ok_or("Colors are written #rrggbb or #rrggbbaa")?;
            p.colors.insert(name.clone(), Some(Color::new(to_hex(c))));
        }
        TokenKind::Space | TokenKind::Radius => {
            let v: f32 = text.parse().map_err(|_| "Enter a number")?;
            if !v.is_finite() || v < 0.0 {
                return Err("Enter a number of 0 or more".into());
            }
            let map = if kind == TokenKind::Space {
                &mut p.spacing
            } else {
                &mut p.radii
            };
            map.insert(name.clone(), Some(v));
        }
        TokenKind::Font => {
            if text.is_empty() {
                return Err("Enter a font family".into());
            }
            p.fonts.insert(name.clone(), Some(text.to_string()));
        }
    }
    Ok(p)
}

/// What the panel shows of one token.
struct Row {
    name: TokenId,
    value: String,
}

fn rows(doc: &Document, kind: TokenKind) -> Vec<Row> {
    let t = &doc.tokens;
    fn numbers(m: &BTreeMap<TokenId, f32>) -> Vec<Row> {
        m.iter()
            .map(|(n, v)| Row {
                name: n.clone(),
                value: fmt_num(*v, 2),
            })
            .collect()
    }
    match kind {
        TokenKind::Color => t
            .colors
            .iter()
            .map(|(n, c)| Row {
                name: n.clone(),
                value: c.0.clone(),
            })
            .collect(),
        TokenKind::Space => numbers(&t.spacing),
        TokenKind::Radius => numbers(&t.radii),
        TokenKind::Font => t
            .fonts
            .iter()
            .map(|(n, f)| Row {
                name: n.clone(),
                value: f.clone(),
            })
            .collect(),
    }
}

enum Action {
    Add(TokenKind),
    SetValue(TokenKind, TokenId, String),
    /// A colour being dragged in the picker.
    Pick(TokenId, [u8; 4]),
    Rename(TokenId, String),
    Delete(TokenId),
}

#[derive(Default)]
pub struct TokensPanel {
    edit: InlineEdit,
    /// Colour being picked, kept between frames so hue survives grey.
    hsva: Option<(String, Hsva)>,
    /// A picker drag is an editor gesture.
    live: bool,
    fields: Fields,
    texts: Texts,
    /// The last refusal or error, shown at the top of the panel.
    notice: Option<String>,
}

impl TokensPanel {
    /// Where widget `key` was drawn last frame, in screen points. Keys are
    /// `add.<kind>`, `<name>.name`, `<name>.value`, `<name>.color` and
    /// `<name>.delete`.
    pub fn field_rect(&self, key: &str) -> Option<Rect> {
        pickers::find_field(&self.fields, key)
    }

    /// The text field `key` showed last frame.
    pub fn field_text(&self, key: &str) -> Option<&str> {
        pickers::find_text(&self.texts, key)
    }

    pub fn is_editing(&self) -> bool {
        self.edit.is_active()
    }

    /// The last refusal, e.g. deleting a token that is still used.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn ui(&mut self, ui: &mut Ui, ed: &mut Editor) {
        if self.live && !ui.ctx().input(|i| i.pointer.any_down()) {
            self.live = false;
            self.hsva = None;
            ed.end_gesture();
        }
        self.fields.clear();
        self.texts.clear();
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
                for kind in TokenKind::ALL {
                    self.section(ui, &ed.doc, kind, &mut actions);
                }
            });
        for a in actions {
            self.run(ed, a);
        }
    }

    fn section(&mut self, ui: &mut Ui, doc: &Document, kind: TokenKind, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.strong(kind.title());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = ui.button("+").on_hover_text("Add token");
                self.fields
                    .push((format!("add.{}", kind.prefix()), resp.rect));
                if resp.clicked() {
                    actions.push(Action::Add(kind));
                }
            });
        });
        let rows = rows(doc, kind);
        if rows.is_empty() {
            ui.weak("None yet");
        }
        for row in rows {
            ui.horizontal(|ui| {
                if kind == TokenKind::Color {
                    self.color_button(ui, &row, actions);
                }
                let k = &row.name.0;
                if let Some(text) = self.edit.show(
                    ui,
                    &mut self.fields,
                    &mut self.texts,
                    &format!("{k}.name"),
                    k,
                    130.0,
                ) {
                    actions.push(Action::Rename(row.name.clone(), text));
                }
                if let Some(text) = self.edit.show(
                    ui,
                    &mut self.fields,
                    &mut self.texts,
                    &format!("{k}.value"),
                    &row.value,
                    74.0,
                ) {
                    actions.push(Action::SetValue(kind, row.name.clone(), text));
                }
                let resp = ui.add(egui::Button::new("✕").small());
                self.fields.push((format!("{k}.delete"), resp.rect));
                if resp.clicked() {
                    actions.push(Action::Delete(row.name.clone()));
                }
            });
        }
        ui.add_space(6.0);
    }

    fn color_button(&mut self, ui: &mut Ui, row: &Row, actions: &mut Vec<Action>) {
        let key = format!("{}.color", row.name.0);
        let rgba = parse_hex(&row.value).unwrap_or([0, 0, 0, 255]);
        let mut hsva = match &self.hsva {
            Some((k, h)) if *k == key => *h,
            _ => Hsva::from_srgba_unmultiplied(rgba),
        };
        let resp = color_edit_button_hsva(ui, &mut hsva, Alpha::OnlyBlend);
        self.fields.push((key.clone(), resp.rect));
        if resp.changed() {
            self.hsva = Some((key, hsva));
            actions.push(Action::Pick(row.name.clone(), hsva.to_srgba_unmultiplied()));
        }
    }

    fn run(&mut self, ed: &mut Editor, action: Action) {
        self.notice = None;
        let result = match action {
            Action::Add(kind) => {
                add_token(ed, kind);
                Ok(())
            }
            Action::SetValue(kind, name, text) => value_patch(kind, &name, &text).and_then(|p| {
                let sel: Vec<NodeId> = ed.selection.clone();
                ed.commit(
                    "Change token",
                    vec![Command::UpsertTokens { patch: p }],
                    sel,
                )
                .map_err(|e| e.to_string())
            }),
            Action::Pick(name, rgba) => {
                let mut patch = TokensPatch::default();
                patch.colors.insert(name, Some(Color::new(to_hex(rgba))));
                self.live_update(ed, patch)
            }
            Action::Rename(from, to) => rename_token(ed, &from, &to),
            Action::Delete(name) => delete_token(ed, &name),
        };
        if let Err(msg) = result {
            ed.status = Some(msg.clone());
            self.notice = Some(msg);
        }
    }

    /// Apply a token change live, as one gesture until the pointer is
    /// released.
    fn live_update(&mut self, ed: &mut Editor, patch: TokensPatch) -> Result<(), String> {
        if !self.live || !ed.in_gesture() {
            ed.begin_gesture("Change token")
                .map_err(|e| e.to_string())?;
            self.live = true;
        }
        ed.gesture_apply(Command::UpsertTokens { patch })
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        let mut t = Tokens::default();
        t.colors
            .insert(TokenId("color.primary".into()), Color::new("#2563eb"));
        t
    }

    #[test]
    fn names_are_dotted_words() {
        let t = tokens();
        assert!(check_name(&t, "color.accent").is_ok());
        assert!(check_name(&t, "space.4").is_ok());
        assert!(check_name(&t, "color.brand-dark.2").is_ok());
        for bad in [
            "", "color", "color.", ".x", "a..b", "a b.c", "{a.b}", "a.b c",
        ] {
            assert!(check_name(&t, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_must_be_unused() {
        assert!(check_name(&tokens(), "color.primary").is_err());
    }

    #[test]
    fn fresh_names_skip_taken_ones() {
        let mut t = tokens();
        assert_eq!(fresh_name(&t, "color").0, "color.new");
        t.colors
            .insert(TokenId("color.new".into()), Color::new("#000000"));
        assert_eq!(fresh_name(&t, "color").0, "color.new-2");
    }

    #[test]
    fn values_are_checked() {
        let n = TokenId("space.4".into());
        assert!(value_patch(TokenKind::Space, &n, "16").is_ok());
        assert!(value_patch(TokenKind::Space, &n, "-1").is_err());
        assert!(value_patch(TokenKind::Space, &n, "x").is_err());
        assert!(value_patch(TokenKind::Color, &n, "#12").is_err());
        assert!(value_patch(TokenKind::Font, &n, " ").is_err());
    }
}
