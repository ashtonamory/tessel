//! Reusable widgets for tokens and styles (§3.6, §9): token pickers for
//! colours, spacing, radii and fonts, a style picker, and the click-to-edit
//! text field the tokens and styles panels use.
//!
//! A picker draws a swatch and a combo box and returns what the user picked;
//! it never edits the document. Widgets record where they were drawn in a
//! `Fields` list, keyed by name, so the headless tests can click them.

use doc::{Color, ColorRef, Document, RadiusRef, SpaceRef, StyleId, StyleKind, TokenId};
use egui::text::{CCursor, CCursorRange};
use egui::{Align2, Color32, CursorIcon, FontId, Key, Rect, Sense, TextEdit, Ui, Vec2};

use crate::inspector::{parse_hex, resolve_ref, to_hex};

/// Where widgets were drawn last frame, by key, in screen points.
pub type Fields = Vec<(String, Rect)>;

/// What each text field showed last frame, by key.
pub type Texts = Vec<(String, String)>;

pub fn find_field(fields: &Fields, key: &str) -> Option<Rect> {
    fields.iter().find(|(k, _)| k == key).map(|(_, r)| *r)
}

pub fn find_text<'a>(texts: &'a Texts, key: &str) -> Option<&'a str> {
    texts
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, t)| t.as_str())
}

/// The colour of an `#rrggbb[aa]` string, if it is one.
pub fn color32(hex: &str) -> Option<Color32> {
    let [r, g, b, a] = parse_hex(hex)?;
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
}

/// A small filled square, outlined so light colours show on a light panel.
pub fn swatch(ui: &mut Ui, color: Option<Color32>) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        match color {
            Some(c) => ui.painter().rect_filled(rect, 2.0, c),
            None => ui.painter().rect_filled(rect, 2.0, Color32::TRANSPARENT),
        };
        ui.painter()
            .rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    }
    rect
}

/// One choice in a token picker.
struct Entry {
    name: String,
    swatch: Option<Color32>,
    /// The value, shown after the name ("#2563eb", "16").
    detail: String,
}

/// A combo box listing `entries`, with the swatch of the current one beside
/// it. `Some(Some(name))` is a token picked; `Some(None)` is "unlink".
fn token_combo(
    ui: &mut Ui,
    fields: &mut Fields,
    key: &str,
    current: Option<&str>,
    entries: &[Entry],
) -> Option<Option<String>> {
    let mut picked = None;
    let current_swatch = current
        .and_then(|c| entries.iter().find(|e| e.name == c))
        .and_then(|e| e.swatch);
    if entries.iter().any(|e| e.swatch.is_some()) {
        swatch(ui, current_swatch);
    }
    let resp = egui::ComboBox::from_id_salt(key)
        .width(120.0)
        .selected_text(current.unwrap_or("No token"))
        .show_ui(ui, |ui| {
            if entries.is_empty() {
                ui.weak("No tokens defined");
            }
            if current.is_some() {
                let r = ui.selectable_label(false, "Unlink token");
                fields.push((format!("{key}.none"), r.rect));
                if r.clicked() {
                    picked = Some(None);
                }
            }
            for e in entries {
                ui.horizontal(|ui| {
                    if e.swatch.is_some() {
                        swatch(ui, e.swatch);
                    }
                    let text = format!("{}  {}", e.name, e.detail);
                    let r = ui.selectable_label(current == Some(e.name.as_str()), text);
                    fields.push((format!("{key}.item.{}", e.name), r.rect));
                    if r.clicked() {
                        picked = Some(Some(e.name.clone()));
                    }
                });
            }
        });
    fields.push((key.to_string(), resp.response.rect));
    picked
}

fn color_entries(doc: &Document) -> Vec<Entry> {
    doc.tokens
        .colors
        .iter()
        .map(|(name, c)| Entry {
            name: name.0.clone(),
            swatch: color32(&c.0),
            detail: c.0.clone(),
        })
        .collect()
}

fn number_entries<'a>(it: impl Iterator<Item = (&'a TokenId, &'a f32)>) -> Vec<Entry> {
    it.map(|(name, v)| Entry {
        name: name.0.clone(),
        swatch: None,
        detail: crate::inspector::fmt_num(*v, 2),
    })
    .collect()
}

/// Token picker for a colour field. Picking a token gives a reference to it;
/// unlinking gives the colour the token resolved to, as a literal.
pub fn color_token_picker(
    ui: &mut Ui,
    doc: &Document,
    fields: &mut Fields,
    key: &str,
    current: &ColorRef,
) -> Option<ColorRef> {
    let name = match current {
        ColorRef::Token(t) => Some(t.0.as_str()),
        ColorRef::Literal(_) => None,
    };
    let picked = token_combo(ui, fields, key, name, &color_entries(doc))?;
    Some(match picked {
        Some(n) => ColorRef::Token(TokenId(n)),
        None => ColorRef::Literal(Color(
            resolve_ref(doc, current)
                .map(to_hex)
                .unwrap_or_else(|| "#000000".to_string()),
        )),
    })
}

/// Token picker for a spacing field (gap, padding). Picking a token gives a
/// reference to it; unlinking gives the px value it resolved to.
pub fn space_token_picker(
    ui: &mut Ui,
    doc: &Document,
    fields: &mut Fields,
    key: &str,
    current: &SpaceRef,
) -> Option<SpaceRef> {
    let name = match current {
        SpaceRef::Token(t) => Some(t.0.as_str()),
        SpaceRef::Px(_) => None,
    };
    let entries = number_entries(doc.tokens.spacing.iter());
    let picked = token_combo(ui, fields, key, name, &entries)?;
    Some(match picked {
        Some(n) => SpaceRef::Token(TokenId(n)),
        None => SpaceRef::Px(doc::resolve::resolve_space(doc, current).unwrap_or(0.0)),
    })
}

/// Token picker for a corner radius. Picking a token gives a reference to
/// it; unlinking gives the px value it resolved to.
pub fn radius_token_picker(
    ui: &mut Ui,
    doc: &Document,
    fields: &mut Fields,
    key: &str,
    current: &RadiusRef,
) -> Option<RadiusRef> {
    let name = match current {
        RadiusRef::Token(t) => Some(t.0.as_str()),
        RadiusRef::Px(_) => None,
    };
    let entries = number_entries(doc.tokens.radii.iter());
    let picked = token_combo(ui, fields, key, name, &entries)?;
    Some(match picked {
        Some(n) => RadiusRef::Token(TokenId(n)),
        None => RadiusRef::Px(doc::resolve::resolve_radius(doc, current).unwrap_or(0.0)),
    })
}

/// Token picker for a font family. Picking a token gives its `{reference}`;
/// unlinking gives the family the token resolved to.
pub fn font_token_picker(
    ui: &mut Ui,
    doc: &Document,
    fields: &mut Fields,
    key: &str,
    family: &str,
) -> Option<String> {
    let name = TokenId::from_reference(family);
    let entries: Vec<Entry> = doc
        .tokens
        .fonts
        .iter()
        .map(|(n, f)| Entry {
            name: n.0.clone(),
            swatch: None,
            detail: f.clone(),
        })
        .collect();
    let picked = token_combo(
        ui,
        fields,
        key,
        name.as_ref().map(|t| t.0.as_str()),
        &entries,
    )?;
    Some(match picked {
        Some(n) => TokenId(n).to_reference(),
        None => doc::resolve::resolve_font(doc, family).into_owned(),
    })
}

// --- Styles ----------------------------------------------------------------

/// The four kinds of style (§3.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleClass {
    Fill,
    Stroke,
    Text,
    Effect,
}

impl StyleClass {
    pub const ALL: [StyleClass; 4] = [
        StyleClass::Fill,
        StyleClass::Stroke,
        StyleClass::Text,
        StyleClass::Effect,
    ];

    pub fn of(kind: &StyleKind) -> StyleClass {
        match kind {
            StyleKind::Fill(_) => StyleClass::Fill,
            StyleKind::Stroke(_) => StyleClass::Stroke,
            StyleKind::Text(_) => StyleClass::Text,
            StyleKind::Effect(_) => StyleClass::Effect,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StyleClass::Fill => "Fill",
            StyleClass::Stroke => "Stroke",
            StyleClass::Text => "Text",
            StyleClass::Effect => "Effect",
        }
    }

    /// Lower case, for widget keys.
    pub fn key(self) -> &'static str {
        match self {
            StyleClass::Fill => "fill",
            StyleClass::Stroke => "stroke",
            StyleClass::Text => "text",
            StyleClass::Effect => "effect",
        }
    }
}

/// Split a style name at its last '/': ("Surface", "Card") for
/// "Surface/Card", ("", "Card") for "Card".
pub fn split_group(name: &str) -> (&str, &str) {
    match name.rfind('/') {
        Some(i) => (name[..i].trim(), name[i + 1..].trim()),
        None => ("", name.trim()),
    }
}

/// Style picker for one class. `Some(Some(id))` applies a style,
/// `Some(None)` detaches the current one. `mixed` is for a selection that
/// does not share one style.
pub fn style_picker(
    ui: &mut Ui,
    doc: &Document,
    fields: &mut Fields,
    key: &str,
    class: StyleClass,
    current: Option<&StyleId>,
    mixed: bool,
) -> Option<Option<StyleId>> {
    let mut styles: Vec<_> = doc
        .styles
        .iter()
        .filter(|s| StyleClass::of(&s.kind) == class)
        .collect();
    styles.sort_by(|a, b| a.name.cmp(&b.name));
    let shown = match current {
        _ if mixed => "Mixed".to_string(),
        Some(id) => doc::resolve::style(doc, id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Missing style".to_string()),
        None => "No style".to_string(),
    };
    let mut picked = None;
    let resp = egui::ComboBox::from_id_salt(key)
        .width(150.0)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if styles.is_empty() {
                ui.weak(format!("No {} styles yet", class.key()));
            }
            if current.is_some() || mixed {
                let r = ui.selectable_label(false, "Detach style");
                fields.push((format!("{key}.none"), r.rect));
                if r.clicked() {
                    picked = Some(None);
                }
            }
            let mut group = None;
            for s in styles {
                let (g, leaf) = split_group(&s.name);
                if group != Some(g) {
                    if !g.is_empty() {
                        ui.weak(g);
                    }
                    group = Some(g);
                }
                let r = ui.selectable_label(current == Some(&s.id), leaf);
                fields.push((format!("{key}.item.{}", s.name), r.rect));
                if r.clicked() {
                    picked = Some(Some(s.id.clone()));
                }
            }
        });
    fields.push((key.to_string(), resp.response.rect));
    picked
}

// --- Click-to-edit text ----------------------------------------------------

struct Inline {
    key: String,
    buffer: String,
    focused: bool,
}

/// A text field that reads as a label until clicked: Enter or clicking away
/// commits, Esc cancels. Only one field of an `InlineEdit` is edited at a
/// time.
#[derive(Default)]
pub struct InlineEdit {
    editing: Option<Inline>,
}

impl InlineEdit {
    pub fn is_active(&self) -> bool {
        self.editing.is_some()
    }

    pub fn cancel(&mut self) {
        self.editing = None;
    }

    /// Draw the field and return the text when an edit is committed.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        fields: &mut Fields,
        texts: &mut Texts,
        key: &str,
        text: &str,
        width: f32,
    ) -> Option<String> {
        let size = Vec2::new(width, ui.spacing().interact_size.y);
        if let Some(e) = self.editing.as_mut().filter(|e| e.key == key) {
            let id = ui.id().with(("inline-edit", key));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let resp = ui.put(
                rect,
                TextEdit::singleline(&mut e.buffer)
                    .id(id)
                    .margin(egui::Margin::symmetric(4, 2)),
            );
            fields.push((key.to_string(), rect));
            if !e.focused {
                e.focused = true;
                resp.request_focus();
                if let Some(mut state) = TextEdit::load_state(ui.ctx(), id) {
                    let len = e.buffer.chars().count();
                    state.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(0),
                        CCursor::new(len),
                    )));
                    state.store(ui.ctx(), id);
                }
            } else if resp.lost_focus() || !resp.has_focus() {
                let e = self.editing.take().expect("checked");
                if !ui.input(|i| i.key_pressed(Key::Escape)) {
                    return Some(e.buffer);
                }
            }
            return None;
        }

        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        fields.push((key.to_string(), rect));
        texts.push((key.to_string(), text.to_string()));
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&resp);
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, ui.visuals().extreme_bg_color);
            ui.painter().rect_stroke(
                rect,
                visuals.corner_radius,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().with_clip_rect(rect.shrink(2.0)).text(
                rect.left_center() + Vec2::new(4.0, 0.0),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(13.0),
                ui.visuals().text_color(),
            );
        }
        let resp = resp.on_hover_cursor(CursorIcon::Text);
        if resp.clicked() {
            // A field that was being edited elsewhere is dropped; its panel
            // commits on blur before this click lands.
            self.editing = Some(Inline {
                key: key.to_string(),
                buffer: text.to_string(),
                focused: false,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_split_at_the_last_slash() {
        assert_eq!(split_group("Surface/Card"), ("Surface", "Card"));
        assert_eq!(split_group("Heading/Large/H1"), ("Heading/Large", "H1"));
        assert_eq!(split_group("Card"), ("", "Card"));
    }

    #[test]
    fn colors_parse_to_egui() {
        assert_eq!(color32("#ff0000"), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(color32("nope"), None);
    }
}
