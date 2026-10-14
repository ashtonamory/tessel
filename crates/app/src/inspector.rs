//! The inspector (§9, right panel): position, size, rotation, opacity,
//! corner radius, fills, strokes, shadows, text and frame properties of the
//! selection.
//!
//! A field shows the value all selected nodes share, or "Mixed"; an edit
//! applies to every selected node that has the property. Every change is a
//! `SetGeometry` or `SetProps` command through the editor. Typing commits on
//! Enter or blur and Esc cancels; dragging a number or a colour picker is one
//! gesture, so one undo step.

use std::rc::Rc;

use doc::{
    Color, ColorRef, Command, CornerRadius, Document, Extra, LineHeight, LinearGradient, Node,
    NodeId, NodeKind, NodePatch, Paint, RadialGradient, Shadow, Shape, Stop, StrokeAlign, StyleId,
    TextAlign, TextSizing,
};
use egui::color_picker::{color_edit_button_hsva, Alpha};
use egui::ecolor::Hsva;
use egui::text::{CCursor, CCursorRange};
use egui::{Align2, CursorIcon, FontId, Key, Rect, Sense, TextEdit, Ui, Vec2};

use crate::editor::Editor;
use crate::pickers::{self, StyleClass};
use crate::styles_panel;

#[path = "inspector_layout.rs"]
mod layout_section;

const FIELD_W: f32 = 58.0;
const HEX_W: f32 = 74.0;

/// A change to one node, applied to a copy of it.
type Edit = Rc<dyn Fn(&mut Node)>;
/// Turns typed text into an edit, or `None` if the text is not valid.
type Parse = Rc<dyn Fn(&str) -> Option<Edit>>;
/// Turns the total drag distance (points) into an edit.
type Drag = Rc<dyn Fn(f32) -> Edit>;

/// A property of a node: read it (`None` when the node does not have it or
/// it cannot be shown as one value) and write it.
struct Lens<T> {
    get: Getter<T>,
    set: Setter<T>,
}

type Getter<T> = Rc<dyn Fn(&Node) -> Option<T>>;
type Setter<T> = Rc<dyn Fn(&mut Node, T)>;

impl<T> Clone for Lens<T> {
    fn clone(&self) -> Self {
        Lens {
            get: self.get.clone(),
            set: self.set.clone(),
        }
    }
}

impl<T: Clone + 'static> Lens<T> {
    fn new(
        get: impl Fn(&Node) -> Option<T> + 'static,
        set: impl Fn(&mut Node, T) + 'static,
    ) -> Lens<T> {
        Lens {
            get: Rc::new(get),
            set: Rc::new(set),
        }
    }

    /// A part of this property.
    fn map<U: Clone + 'static>(
        &self,
        get: impl Fn(&T) -> Option<U> + 'static,
        set: impl Fn(&mut T, U) + 'static,
    ) -> Lens<U> {
        let (g1, g2, s) = (self.get.clone(), self.get.clone(), self.set.clone());
        Lens::new(
            move |n| g1(n).and_then(|t| get(&t)),
            move |n, u| {
                if let Some(mut t) = g2(n) {
                    set(&mut t, u);
                    s(n, t);
                }
            },
        )
    }

    /// An edit that sets the property to `v`.
    fn to(&self, v: T) -> Edit {
        let set = self.set.clone();
        Rc::new(move |n| set(n, v.clone()))
    }

    /// An edit that changes the property with `f`, where the node has it.
    fn modify(&self, f: impl Fn(&mut T) + 'static) -> Edit {
        let (get, set) = (self.get.clone(), self.set.clone());
        Rc::new(move |n| {
            if let Some(mut v) = get(n) {
                f(&mut v);
                set(n, v);
            }
        })
    }
}

/// The value of a property across the selection.
#[derive(Clone, Debug, PartialEq)]
enum Shared<T> {
    Absent,
    Same(T),
    Mixed,
}

fn shared<T: PartialEq>(nodes: &[&Node], lens: &Lens<T>) -> Shared<T> {
    let mut out = Shared::Absent;
    for n in nodes {
        match ((lens.get)(n), &out) {
            (None, _) => return Shared::Mixed,
            (Some(v), Shared::Absent) => out = Shared::Same(v),
            (Some(v), Shared::Same(s)) if v == *s => {}
            _ => return Shared::Mixed,
        }
    }
    out
}

/// How a number is shown, dragged and limited.
#[derive(Clone, Copy)]
struct Num {
    /// Display units per point of drag.
    speed: f32,
    /// Drag results are multiples of this (display units).
    step: f32,
    min: f32,
    max: f32,
    /// Display value = stored value × scale (e.g. 100 for a percentage).
    scale: f32,
    suffix: &'static str,
    /// Decimals shown (at most).
    decimals: usize,
}

const PX: Num = Num {
    speed: 1.0,
    step: 1.0,
    min: -1e6,
    max: 1e6,
    scale: 1.0,
    suffix: "",
    decimals: 2,
};
const SIZE: Num = Num { min: 0.0, ..PX };
const DEG: Num = Num {
    min: -360.0,
    max: 360.0,
    suffix: "°",
    ..PX
};
const PERCENT: Num = Num {
    speed: 0.5,
    step: 1.0,
    min: 0.0,
    max: 1.0,
    scale: 100.0,
    suffix: "%",
    decimals: 1,
};
/// An 8-bit colour alpha as a whole percentage.
const ALPHA: Num = Num {
    decimals: 0,
    ..PERCENT
};
/// Gradient coordinates, in the node's unit box.
const UNIT: Num = Num {
    speed: 0.01,
    step: 0.01,
    min: -10.0,
    max: 10.0,
    scale: 1.0,
    suffix: "",
    decimals: 2,
};

struct Change {
    key: String,
    label: String,
    live: bool,
    targets: Vec<NodeId>,
    edit: Edit,
}

/// A field being typed into.
struct Editing {
    key: String,
    label: String,
    buffer: String,
    /// The nodes to change, fixed when typing started.
    targets: Vec<NodeId>,
    focused: bool,
    parse: Parse,
    /// Drawn this frame; a field that disappears commits.
    seen: bool,
}

/// A drag in progress: one editor gesture.
struct Live {
    key: String,
    /// The selected nodes when the drag started; each frame's edit applies
    /// to these.
    originals: Vec<Node>,
}

#[derive(Default)]
pub struct Inspector {
    editing: Option<Editing>,
    live: Option<Live>,
    drag_total: f32,
    /// Colour being picked, kept between frames so hue survives grey.
    hsva: Option<(String, Hsva)>,
    each_corner: bool,
    targets: Vec<NodeId>,
    changes: Vec<Change>,
    fields: Vec<(String, Rect)>,
    /// What each text field showed last frame.
    texts: Vec<(String, String)>,
}

impl Inspector {
    /// Where field `key` was drawn last frame, in screen points.
    pub fn field_rect(&self, key: &str) -> Option<Rect> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, r)| *r)
    }

    /// The text field `key` showed last frame: a value or "Mixed".
    pub fn field_text(&self, key: &str) -> Option<&str> {
        self.texts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, t)| t.as_str())
    }

    /// Keys of the fields drawn last frame.
    pub fn field_keys(&self) -> Vec<&str> {
        self.fields.iter().map(|(k, _)| k.as_str()).collect()
    }

    /// Typing into a field; keyboard shortcuts must leave the keys alone.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Changes collected by the last `ui` and not yet applied.
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
            || self.editing.as_ref().is_some_and(|e| !e.seen)
            || self.live.is_some()
    }

    /// Draw the panel for the editor's selection. Edits are collected and
    /// applied by `apply`.
    pub fn ui(&mut self, ui: &mut Ui, ed: &Editor) {
        self.fields.clear();
        self.texts.clear();
        if let Some(e) = &mut self.editing {
            e.seen = false;
        }
        if self.targets != ed.selection {
            self.each_corner = false;
            self.targets = ed.selection.clone();
        }
        let nodes: Vec<&Node> = ed.selection.iter().filter_map(|id| ed.node(id)).collect();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                if ed.selection.is_empty() {
                    ui.weak("Select a layer to see its properties.");
                    return;
                }
                // Nodes inside instances have only the component section.
                if nodes.is_empty() {
                    return;
                }
                if ed.read_only {
                    ui.disable();
                }
                self.sections(ui, &ed.doc, &nodes);
            });
    }

    /// Apply the edits collected by `ui`, and end a drag once the pointer
    /// is released.
    pub fn apply(&mut self, ctx: &egui::Context, ed: &mut Editor) {
        if let Some(e) = self.editing.take_if(|e| !e.seen) {
            self.commit_typed(e);
        }
        for ch in std::mem::take(&mut self.changes) {
            self.apply_change(ed, ch);
        }
        layout_section::apply_actions(ctx, ed);
        if self.live.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            self.live = None;
            self.hsva = None;
            ed.end_gesture();
        }
    }

    fn apply_change(&mut self, ed: &mut Editor, ch: Change) {
        if !ch.live {
            self.live = None;
            let cmds: Vec<Command> = ch
                .targets
                .iter()
                .filter_map(|id| {
                    let base = ed.node(id)?.clone();
                    let mut new = base.clone();
                    (ch.edit)(&mut new);
                    node_command(ed, &base, new)
                })
                .collect();
            if !cmds.is_empty() {
                let sel = ed.selection.clone();
                let _ = ed.commit(&ch.label, vec![Command::Batch(cmds)], sel);
            }
            return;
        }
        let same = self.live.as_ref().is_some_and(|l| l.key == ch.key);
        if !same || !ed.in_gesture() {
            if ed.begin_gesture(&ch.label).is_err() {
                self.live = None;
                return;
            }
            let originals = ch
                .targets
                .iter()
                .filter_map(|id| ed.node(id).cloned())
                .collect();
            self.live = Some(Live {
                key: ch.key.clone(),
                originals,
            });
        }
        let Some(live) = self.live.take() else {
            return;
        };
        for base in &live.originals {
            let mut new = base.clone();
            (ch.edit)(&mut new);
            if let Some(cmd) = node_command(ed, base, new) {
                if let Err(e) = ed.gesture_apply(cmd) {
                    ed.status = Some(format!("{}: {e}", ch.label));
                }
            }
        }
        self.live = Some(live);
    }

    fn commit_typed(&mut self, e: Editing) {
        match (e.parse)(&e.buffer) {
            Some(edit) => self.changes.push(Change {
                key: e.key,
                label: e.label,
                live: false,
                targets: e.targets,
                edit,
            }),
            None => tracing::debug!("{}: ignoring {:?}", e.key, e.buffer),
        }
    }

    fn push(&mut self, key: &str, label: &str, live: bool, edit: Edit) {
        self.changes.push(Change {
            key: key.to_string(),
            label: label.to_string(),
            live,
            targets: self.targets.clone(),
            edit,
        });
    }

    // --- Sections -----------------------------------------------------------

    fn sections(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        if let [n] = nodes {
            ui.horizontal(|ui| {
                ui.strong(&n.name);
                ui.weak(n.kind.tag());
            });
        } else {
            ui.strong(format!("{} layers", nodes.len()));
        }
        ui.separator();

        self.transform(ui, nodes);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Opacity");
            self.number(
                ui,
                nodes,
                "opacity",
                "Change opacity",
                Lens::new(|n| Some(n.opacity), |n, v| n.opacity = v),
                PERCENT,
            );
        });

        self.layout_sections(ui, doc, nodes);
        let all = |f: fn(&Node) -> bool| nodes.iter().all(|n| f(n));
        if all(|n| corner_radius(n).is_some()) {
            self.corner_radius(ui, doc, nodes);
        }
        if all(|n| matches!(n.kind, NodeKind::Frame(_))) {
            self.clip(ui, nodes);
        }
        if all(|n| matches!(n.kind, NodeKind::Text(_))) {
            ui.separator();
            self.text(ui, doc, nodes);
        }
        if all(|n| fills(n).is_some()) {
            ui.separator();
            let list = Lens::new(
                |n| fills(n).cloned(),
                |n, v| {
                    if let Some(f) = fills_mut(n) {
                        *f = v;
                    }
                },
            );
            let default = Paint::solid(ColorRef::literal(match nodes[0].kind {
                NodeKind::Text(_) => "#000000",
                _ => "#d9d9d9",
            }));
            let styled = all(|n| n.shape().is_some())
                && self.style_row(ui, doc, nodes, "fill.style", StyleClass::Fill);
            if !styled {
                self.paints(ui, doc, nodes, "fill", "Fill", list, default);
            }
        }
        if all(|n| n.shape().is_some()) {
            ui.separator();
            self.strokes(ui, doc, nodes);
            ui.separator();
            self.shadows(ui, doc, nodes);
        }
    }

    fn transform(&mut self, ui: &mut Ui, nodes: &[&Node]) {
        ui.horizontal(|ui| {
            ui.label("X");
            self.number(
                ui,
                nodes,
                "x",
                "Change X",
                Lens::new(|n| Some(n.x), |n, v| n.x = v),
                PX,
            );
            ui.label("Y");
            self.number(
                ui,
                nodes,
                "y",
                "Change Y",
                Lens::new(|n| Some(n.y), |n, v| n.y = v),
                PX,
            );
        });
        ui.horizontal(|ui| {
            ui.label("W");
            self.number(
                ui,
                nodes,
                "width",
                "Change width",
                Lens::new(|n| Some(n.width), |n, v| n.width = v),
                SIZE,
            );
            ui.label("H");
            self.number(
                ui,
                nodes,
                "height",
                "Change height",
                Lens::new(|n| Some(n.height), |n, v| n.height = v),
                SIZE,
            );
        });
        ui.horizontal(|ui| {
            ui.label("Rotation");
            self.number(
                ui,
                nodes,
                "rotation",
                "Change rotation",
                Lens::new(|n| Some(n.rotation), |n, v| n.rotation = v),
                DEG,
            );
        });
    }

    fn corner_radius(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        let radius = Lens::new(
            |n| corner_radius(n).cloned(),
            |n, v| {
                if let Some(r) = corner_radius_mut(n) {
                    *r = v;
                }
            },
        );
        let each = self.each_corner
            || nodes
                .iter()
                .any(|n| matches!(corner_radius(n), Some(CornerRadius::Each(_))));
        ui.horizontal(|ui| {
            ui.label("Radius");
            if !each {
                let uniform = radius.map(
                    |r| match r {
                        CornerRadius::All(v) => Some(v.clone()),
                        CornerRadius::Each(a) if a.iter().all(|v| *v == a[0]) => Some(a[0].clone()),
                        CornerRadius::Each(_) => None,
                    },
                    |r, v| *r = CornerRadius::All(v),
                );
                self.radius_w(ui, doc, nodes, "radius", uniform, FIELD_W, true);
            }
            let resp = ui
                .selectable_label(each, "⛶")
                .on_hover_text("Independent corners");
            self.fields.push(("radius.each".into(), resp.rect));
            if resp.clicked() {
                if each {
                    self.each_corner = false;
                    // Back to one radius: the top-left corner's.
                    let to_all = radius.modify(|r| {
                        let first = match r {
                            CornerRadius::Each(a) => Some(a[0].clone()),
                            CornerRadius::All(_) => None,
                        };
                        if let Some(first) = first {
                            *r = CornerRadius::All(first);
                        }
                    });
                    self.push("radius", "Change corner radius", false, to_all);
                } else {
                    self.each_corner = true;
                }
            }
        });
        if each {
            let mut corners = Vec::new();
            ui.horizontal(|ui| {
                for (i, label) in ["↖", "↗", "↘", "↙"].into_iter().enumerate() {
                    ui.label(label);
                    let corner = radius.map(
                        move |r| Some(r.corners()[i].clone()),
                        move |r, v| {
                            let mut a = r.corners();
                            a[i] = v;
                            *r = CornerRadius::Each(a);
                        },
                    );
                    self.radius_w(
                        ui,
                        doc,
                        nodes,
                        &format!("radius.{i}"),
                        corner.clone(),
                        40.0,
                        false,
                    );
                    corners.push(corner);
                }
            });
            // Token pickers one row per corner: a picker is too wide to share a row.
            for (i, (label, corner)) in ["↖", "↗", "↘", "↙"].into_iter().zip(corners).enumerate()
            {
                ui.horizontal(|ui| {
                    ui.label(label);
                    self.radius_picker(ui, doc, nodes, &format!("radius.{i}"), corner);
                });
            }
        }
    }

    fn clip(&mut self, ui: &mut Ui, nodes: &[&Node]) {
        let clip = Lens::new(
            |n| match &n.kind {
                NodeKind::Frame(f) => Some(f.clip),
                _ => None,
            },
            |n, v| {
                if let NodeKind::Frame(f) = &mut n.kind {
                    f.clip = v;
                }
            },
        );
        let value = shared(nodes, &clip);
        let mut checked = value == Shared::Same(true);
        let resp = ui.add(
            egui::Checkbox::new(&mut checked, "Clip content").indeterminate(value == Shared::Mixed),
        );
        self.fields.push(("clip".into(), resp.rect));
        if resp.clicked() {
            let v = value != Shared::Same(true);
            self.push("clip", "Change clipping", false, clip.to(v));
        }
    }

    fn text(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        ui.strong("Text");
        let style = Lens::new(
            |n| match &n.kind {
                NodeKind::Text(t) => Some(t.style.clone()),
                _ => None,
            },
            |n, v| {
                if let NodeKind::Text(t) = &mut n.kind {
                    t.style = v;
                }
            },
        );
        let text_style = Lens::new(
            |n| match &n.kind {
                NodeKind::Text(t) => Some(t.text_style.clone()),
                _ => None,
            },
            |n, v| {
                if let NodeKind::Text(t) = &mut n.kind {
                    t.text_style = v;
                }
            },
        );
        let styled =
            self.style_row_with(ui, doc, nodes, "text.style", StyleClass::Text, text_style);
        if !styled {
            self.type_fields(ui, doc, nodes, &style);
        }
        let align = Lens::new(
            |n| match &n.kind {
                NodeKind::Text(t) => Some(t.align),
                _ => None,
            },
            |n, v| {
                if let NodeKind::Text(t) = &mut n.kind {
                    t.align = v;
                }
            },
        );
        ui.horizontal(|ui| {
            ui.label("Align");
            self.choice(
                ui,
                nodes,
                "text.align",
                "Change text alignment",
                align,
                &[
                    (TextAlign::Left, "Left", "left"),
                    (TextAlign::Center, "Center", "center"),
                    (TextAlign::Right, "Right", "right"),
                    (TextAlign::Justify, "Justify", "justify"),
                ],
            );
        });
    }

    /// Font, size, weight, line height and letter spacing: what a text
    /// style carries.
    fn type_fields(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        style: &Lens<doc::TextStyle>,
    ) {
        let family = style.map(|s| Some(s.font_family.clone()), |s, v| s.font_family = v);
        let family_lens = family.clone();
        ui.horizontal(|ui| {
            ui.label("Font");
            let display = match shared(nodes, &family) {
                Shared::Same(f) => f,
                _ => "Mixed".to_string(),
            };
            let parse: Parse = Rc::new(move |s: &str| {
                let s = s.trim();
                (!s.is_empty()).then(|| family.to(s.to_string()))
            });
            self.field(
                ui,
                "font.family",
                "Change font",
                &display,
                FIELD_W * 2.0,
                None,
                parse,
            );
        });
        ui.horizontal(|ui| {
            ui.label("Token");
            let current = match shared(nodes, &family_lens) {
                Shared::Same(f) => f,
                _ => String::new(),
            };
            let key = "font.token";
            if let Some(f) = pickers::font_token_picker(ui, doc, &mut self.fields, key, &current) {
                self.push(key, "Change font", false, family_lens.to(f));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Size");
            let size = style.map(|s| Some(s.font_size), |s, v| s.font_size = v);
            self.number(
                ui,
                nodes,
                "font.size",
                "Change font size",
                size,
                Num { min: 1.0, ..SIZE },
            );
            ui.label("Weight");
            let weight = style.map(
                |s| Some(s.font_weight as f32),
                |s, v| s.font_weight = ((v / 100.0).round() * 100.0).clamp(100.0, 900.0) as u16,
            );
            self.number(
                ui,
                nodes,
                "font.weight",
                "Change font weight",
                weight,
                Num {
                    speed: 2.0,
                    step: 100.0,
                    min: 100.0,
                    max: 900.0,
                    ..PX
                },
            );
        });
        ui.horizontal(|ui| {
            ui.label("Line height");
            let lh = style.map(|s| Some(s.line_height), |s, v| s.line_height = v);
            let mode = shared(nodes, &lh);
            let text = match &mode {
                Shared::Same(LineHeight::Auto) => "Auto",
                Shared::Same(LineHeight::Px(_)) => "px",
                Shared::Same(LineHeight::Percent(_)) => "%",
                _ => "Mixed",
            };
            let resp = egui::ComboBox::from_id_salt("line_height.mode")
                .width(50.0)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for (label, v) in [
                        ("Auto", LineHeight::Auto),
                        ("px", LineHeight::Px(24.0)),
                        ("%", LineHeight::Percent(120.0)),
                    ] {
                        let selected = matches!(&mode, Shared::Same(m)
                            if std::mem::discriminant(m) == std::mem::discriminant(&v));
                        if ui.selectable_label(selected, label).clicked() && !selected {
                            self.push("line_height", "Change line height", false, lh.to(v));
                        }
                    }
                });
            self.fields
                .push(("line_height.mode".into(), resp.response.rect));
            let value = lh.map(
                |l| match *l {
                    LineHeight::Auto => None,
                    LineHeight::Px(v) | LineHeight::Percent(v) => Some(v),
                },
                |l, v| match l {
                    LineHeight::Px(x) | LineHeight::Percent(x) => *x = v,
                    LineHeight::Auto => {}
                },
            );
            if matches!(
                mode,
                Shared::Same(LineHeight::Px(_) | LineHeight::Percent(_))
            ) {
                self.number(ui, nodes, "line_height", "Change line height", value, SIZE);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Letter spacing");
            let ls = style.map(|s| Some(s.letter_spacing), |s, v| s.letter_spacing = v);
            self.number(
                ui,
                nodes,
                "letter_spacing",
                "Change letter spacing",
                ls,
                Num {
                    speed: 0.1,
                    step: 0.1,
                    ..PX
                },
            );
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn paints(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        title: &str,
        list: Lens<Vec<Paint>>,
        default: Paint,
    ) {
        let value = shared(nodes, &list);
        let label = format!("Change {}", title.to_lowercase());
        ui.horizontal(|ui| {
            ui.strong(title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = ui.button("+").on_hover_text(format!("Add {}", title));
                self.fields.push((format!("{key}.add"), resp.rect));
                if resp.clicked() {
                    let edit = match &value {
                        Shared::Same(_) => {
                            let d = default.clone();
                            list.modify(move |l| l.push(d.clone()))
                        }
                        _ => list.to(vec![default.clone()]),
                    };
                    self.push(
                        &format!("{key}.add"),
                        &format!("Add {}", title.to_lowercase()),
                        false,
                        edit,
                    );
                }
            });
        });
        let paints = match value {
            Shared::Same(p) => p,
            Shared::Mixed => {
                ui.weak("Mixed — click + to replace");
                return;
            }
            Shared::Absent => return,
        };
        let len = paints.len();
        // Top-most paint first, as in the layers panel.
        for (i, paint) in paints.iter().enumerate().rev() {
            let k = format!("{key}.{i}");
            let lens = list.map(
                move |l| l.get(i).cloned(),
                move |l, p| {
                    if let Some(slot) = l.get_mut(i) {
                        *slot = p;
                    }
                },
            );
            let solid = matches!(paint, Paint::Solid(_)).then(|| {
                lens.map(
                    |p| match p {
                        Paint::Solid(s) => Some(s.color.clone()),
                        _ => None,
                    },
                    |p, c| {
                        if let Paint::Solid(s) = p {
                            s.color = c;
                        }
                    },
                )
            });
            ui.horizontal(|ui| {
                self.paint_kind(ui, &k, &label, paint, &lens);
                if let Some(color) = &solid {
                    self.color(ui, doc, nodes, &k, &label, color.clone());
                }
                if let Paint::Image(img) = paint {
                    ui.weak(format!("Image {}", img.asset.0));
                }
                let mut button = |ui: &mut Ui, text: &str, name: &str, enabled: bool| {
                    let resp = ui.add_enabled(enabled, egui::Button::new(text).small());
                    self.fields.push((format!("{k}.{name}"), resp.rect));
                    resp.clicked()
                };
                let (up, down, remove) = (
                    button(ui, "▲", "up", i + 1 < len),
                    button(ui, "▼", "down", i > 0),
                    button(ui, "✕", "remove", true),
                );
                // Up in the list is towards the front, i.e. a higher index.
                if up {
                    let e = list.modify(move |l| l.swap(i, i + 1));
                    self.push(&k, &label, false, e);
                }
                if down {
                    let e = list.modify(move |l| l.swap(i, i - 1));
                    self.push(&k, &label, false, e);
                }
                if remove {
                    let e = list.modify(move |l| {
                        if i < l.len() {
                            l.remove(i);
                        }
                    });
                    self.push(&k, &format!("Remove {}", key), false, e);
                }
            });
            if let Some(color) = solid {
                self.color_token(ui, doc, nodes, &k, &label, color);
            }
            if matches!(paint, Paint::LinearGradient(_) | Paint::RadialGradient(_)) {
                ui.indent(&k, |ui| {
                    self.gradient(ui, doc, nodes, &k, &label, paint, &lens)
                });
            }
        }
    }

    /// The paint type picker.
    fn paint_kind(
        &mut self,
        ui: &mut Ui,
        key: &str,
        label: &str,
        paint: &Paint,
        lens: &Lens<Paint>,
    ) {
        let name = |p: &Paint| match p {
            Paint::Solid(_) => "Solid",
            Paint::LinearGradient(_) => "Linear",
            Paint::RadialGradient(_) => "Radial",
            Paint::Image(_) => "Image",
        };
        let resp = egui::ComboBox::from_id_salt(format!("{key}.type"))
            .width(64.0)
            .selected_text(name(paint))
            .show_ui(ui, |ui| {
                for to in ["Solid", "Linear", "Radial"] {
                    if ui.selectable_label(name(paint) == to, to).clicked() && name(paint) != to {
                        let converted = convert_paint(paint, to);
                        self.push(key, label, false, lens.to(converted));
                    }
                }
            });
        self.fields
            .push((format!("{key}.type"), resp.response.rect));
    }

    #[allow(clippy::too_many_arguments)]
    fn gradient(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        label: &str,
        paint: &Paint,
        lens: &Lens<Paint>,
    ) {
        // Geometry, in the node's unit box.
        let point = |which: usize, axis: usize| {
            lens.map(
                move |p| match p {
                    Paint::LinearGradient(g) => Some([g.start, g.end][which][axis]),
                    Paint::RadialGradient(g) => Some([g.center, g.radius][which][axis]),
                    _ => None,
                },
                move |p, v| match p {
                    Paint::LinearGradient(g) => [&mut g.start, &mut g.end][which][axis] = v,
                    Paint::RadialGradient(g) => [&mut g.center, &mut g.radius][which][axis] = v,
                    _ => {}
                },
            )
        };
        let names = match paint {
            Paint::RadialGradient(_) => [("Center", "center"), ("Radius", "radius")],
            _ => [("Start", "start"), ("End", "end")],
        };
        for (which, (text, name)) in names.into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(text);
                for (axis, a) in ["x", "y"].into_iter().enumerate() {
                    self.number_w(
                        ui,
                        nodes,
                        &format!("{key}.{name}.{a}"),
                        label,
                        point(which, axis),
                        UNIT,
                        44.0,
                    );
                }
            });
        }

        let stops_of = |p: &Paint| match p {
            Paint::LinearGradient(g) => Some(g.stops.clone()),
            Paint::RadialGradient(g) => Some(g.stops.clone()),
            _ => None,
        };
        let stops = lens.map(stops_of, |p, v| match p {
            Paint::LinearGradient(g) => g.stops = v,
            Paint::RadialGradient(g) => g.stops = v,
            _ => {}
        });
        let list = stops_of(paint).unwrap_or_default();
        for (j, _) in list.iter().enumerate() {
            let k = format!("{key}.stop.{j}");
            let stop = stops.map(
                move |s| s.get(j).cloned(),
                move |s, v| {
                    if let Some(slot) = s.get_mut(j) {
                        *slot = v;
                    }
                },
            );
            let color = stop.map(|s| Some(s.color.clone()), |s, c| s.color = c);
            ui.horizontal(|ui| {
                let offset = stop.map(|s| Some(s.offset), |s, v| s.offset = v);
                self.number_w(
                    ui,
                    nodes,
                    &format!("{k}.offset"),
                    label,
                    offset,
                    PERCENT,
                    44.0,
                );
                self.color(ui, doc, nodes, &k, label, color.clone());
                let resp = ui.add_enabled(list.len() > 2, egui::Button::new("✕").small());
                self.fields.push((format!("{k}.remove"), resp.rect));
                if resp.clicked() {
                    let e = stops.modify(move |s| {
                        if j < s.len() {
                            s.remove(j);
                        }
                    });
                    self.push(&k, "Remove gradient stop", false, e);
                }
            });
            self.color_token(ui, doc, nodes, &k, label, color);
        }
        let resp = ui.small_button("+ stop");
        self.fields.push((format!("{key}.stop.add"), resp.rect));
        if resp.clicked() {
            let e = stops.modify(|s| {
                let last = s.last().cloned().unwrap_or(Stop {
                    offset: 1.0,
                    color: ColorRef::literal("#000000"),
                });
                let prev = if s.len() >= 2 {
                    s[s.len() - 2].offset
                } else {
                    0.0
                };
                s.insert(
                    s.len().saturating_sub(1),
                    Stop {
                        offset: (prev + last.offset) / 2.0,
                        color: last.color,
                    },
                );
            });
            self.push(key, "Add gradient stop", false, e);
        }
    }

    fn strokes(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        let shape = Lens::new(
            |n| n.shape().cloned(),
            |n, v| {
                if let Some(s) = n.shape_mut() {
                    *s = v;
                }
            },
        );
        let list = shape.map(|s| Some(s.strokes.clone()), |s, v| s.strokes = v);
        let default = Paint::solid(ColorRef::literal("#000000"));
        let styled = self.style_row(ui, doc, nodes, "stroke.style", StyleClass::Stroke);
        if !styled {
            self.paints(ui, doc, nodes, "stroke", "Stroke", list, default);
        }
        ui.horizontal(|ui| {
            if styled {
                return;
            }
            ui.label("Width");
            let width = shape.map(|s| Some(s.stroke_width), |s, v| s.stroke_width = v);
            self.number(
                ui,
                nodes,
                "stroke.width",
                "Change stroke width",
                width,
                Num {
                    speed: 0.25,
                    step: 0.5,
                    ..SIZE
                },
            );
        });
        ui.horizontal(|ui| {
            ui.label("Align");
            let align = shape.map(|s| Some(s.stroke_align), |s, v| s.stroke_align = v);
            self.choice(
                ui,
                nodes,
                "stroke.align",
                "Change stroke alignment",
                align,
                &[
                    (StrokeAlign::Inside, "Inside", "inside"),
                    (StrokeAlign::Center, "Center", "center"),
                    (StrokeAlign::Outside, "Outside", "outside"),
                ],
            );
        });
    }

    fn shadows(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        let list: Lens<Vec<Shadow>> = Lens::new(
            |n| n.shape().map(|s| s.shadows.clone()),
            |n, v| {
                if let Some(s) = n.shape_mut() {
                    s.shadows = v;
                }
            },
        );
        if self.style_row(ui, doc, nodes, "effect.style", StyleClass::Effect) {
            return;
        }
        let value = shared(nodes, &list);
        let default = Shadow {
            x: 0.0,
            y: 4.0,
            blur: 8.0,
            spread: 0.0,
            color: ColorRef::literal("#00000040"),
            extra: Extra::new(),
        };
        ui.horizontal(|ui| {
            ui.strong("Shadows");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = ui.button("+").on_hover_text("Add drop shadow");
                self.fields.push(("shadow.add".into(), resp.rect));
                if resp.clicked() {
                    let edit = match &value {
                        Shared::Same(_) => {
                            let d = default.clone();
                            list.modify(move |l| l.push(d.clone()))
                        }
                        _ => list.to(vec![default.clone()]),
                    };
                    self.push("shadow.add", "Add shadow", false, edit);
                }
            });
        });
        let shadows = match value {
            Shared::Same(s) => s,
            Shared::Mixed => {
                ui.weak("Mixed — click + to replace");
                return;
            }
            Shared::Absent => return,
        };
        for (i, _) in shadows.iter().enumerate() {
            let k = format!("shadow.{i}");
            let shadow = list.map(
                move |l| l.get(i).cloned(),
                move |l, s| {
                    if let Some(slot) = l.get_mut(i) {
                        *slot = s;
                    }
                },
            );
            ui.horizontal(|ui| {
                type Field = (
                    &'static str,
                    &'static str,
                    fn(&Shadow) -> f32,
                    fn(&mut Shadow, f32),
                );
                let fields: [Field; 4] = [
                    ("X", "x", |s| s.x, |s, v| s.x = v),
                    ("Y", "y", |s| s.y, |s, v| s.y = v),
                    ("B", "blur", |s| s.blur, |s, v| s.blur = v.max(0.0)),
                    ("S", "spread", |s| s.spread, |s, v| s.spread = v),
                ];
                for (text, name, get, set) in fields {
                    ui.label(text);
                    let lens = shadow.map(move |s| Some(get(s)), set);
                    self.number_w(
                        ui,
                        nodes,
                        &format!("{k}.{name}"),
                        "Change shadow",
                        lens,
                        PX,
                        36.0,
                    );
                }
            });
            let color = shadow.map(|s| Some(s.color.clone()), |s, c| s.color = c);
            ui.horizontal(|ui| {
                self.color(ui, doc, nodes, &k, "Change shadow", color.clone());
                let resp = ui.add(egui::Button::new("✕").small());
                self.fields.push((format!("{k}.remove"), resp.rect));
                if resp.clicked() {
                    let e = list.modify(move |l| {
                        if i < l.len() {
                            l.remove(i);
                        }
                    });
                    self.push(&k, "Remove shadow", false, e);
                }
            });
            self.color_token(ui, doc, nodes, &k, "Change shadow", color);
        }
    }

    // --- Tokens and styles --------------------------------------------------

    /// The style picker of `class` for shape nodes. True if every selected
    /// node uses one style that exists, so its values are shown by the
    /// style, not the node.
    fn style_row(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        class: StyleClass,
    ) -> bool {
        let lens = Lens::new(
            move |n| {
                n.shape()?;
                Some(styles_panel::style_of(n, class))
            },
            move |n, v| set_style(n, class, v),
        );
        self.style_row_with(ui, doc, nodes, key, class, lens)
    }

    fn style_row_with(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        class: StyleClass,
        lens: Lens<Option<StyleId>>,
    ) -> bool {
        let (current, mixed) = match shared(nodes, &lens) {
            Shared::Same(c) => (c, false),
            Shared::Mixed => (None, true),
            Shared::Absent => return false,
        };
        ui.horizontal(|ui| {
            ui.label(format!("{} style", class.label()));
            let picked = pickers::style_picker(
                ui,
                doc,
                &mut self.fields,
                key,
                class,
                current.as_ref(),
                mixed,
            );
            match picked {
                Some(Some(id)) => self.push(key, "Apply style", false, lens.to(Some(id))),
                Some(None) => {
                    let styles = doc.styles.clone();
                    let edit: Edit = Rc::new(move |n| detach_style(n, class, &styles));
                    self.push(key, "Detach style", false, edit);
                }
                None => {}
            }
        });
        current
            .as_ref()
            .is_some_and(|id| doc::resolve::style(doc, id).is_some())
    }

    /// A row with the token picker for a colour field.
    fn color_token(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<ColorRef>,
    ) {
        let current = match shared(nodes, &lens) {
            Shared::Same(c) => c,
            _ => ColorRef::literal("#000000"),
        };
        ui.horizontal(|ui| {
            ui.label("Token");
            let key = format!("{key}.token");
            if let Some(c) = pickers::color_token_picker(ui, doc, &mut self.fields, &key, &current)
            {
                self.push(&key, label, false, lens.to(c));
            }
        });
    }

    // --- Widgets ------------------------------------------------------------

    fn number(
        &mut self,
        ui: &mut Ui,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<f32>,
        num: Num,
    ) {
        self.number_w(ui, nodes, key, label, lens, num, FIELD_W);
    }

    /// A number field: drag sideways to change it, click to type.
    #[allow(clippy::too_many_arguments)]
    fn number_w(
        &mut self,
        ui: &mut Ui,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<f32>,
        num: Num,
        width: f32,
    ) {
        let display = match shared(nodes, &lens) {
            Shared::Same(v) => format!("{}{}", fmt_num(v * num.scale, num.decimals), num.suffix),
            Shared::Mixed => "Mixed".to_string(),
            Shared::Absent => return,
        };
        let set = lens.set.clone();
        let parse: Parse = Rc::new(move |s: &str| {
            let v: f32 = s.trim().trim_end_matches(num.suffix).trim().parse().ok()?;
            if !v.is_finite() {
                return None;
            }
            let v = (v / num.scale).clamp(num.min, num.max);
            let set = set.clone();
            Some(Rc::new(move |n: &mut Node| set(n, v)) as Edit)
        });
        let drag: Drag = Rc::new(move |points: f32| {
            let d = (points * num.speed / num.step).round() * num.step / num.scale;
            lens.modify(move |v| *v = (*v + d).clamp(num.min, num.max))
        });
        self.field(ui, key, label, &display, width, Some(drag), parse);
    }

    /// A colour: swatch with picker, hex field and alpha.
    fn color(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<ColorRef>,
    ) {
        let value = shared(nodes, &lens);
        let rgba = match &value {
            Shared::Same(c) => resolve_ref(doc, c),
            _ => None,
        };
        let mut hsva = match &self.hsva {
            Some((k, h)) if k == key => *h,
            _ => Hsva::from_srgba_unmultiplied(rgba.unwrap_or([0, 0, 0, 255])),
        };
        let resp = color_edit_button_hsva(ui, &mut hsva, Alpha::OnlyBlend);
        self.fields.push((format!("{key}.color"), resp.rect));
        if resp.changed() {
            self.hsva = Some((key.to_string(), hsva));
            let c = ColorRef::Literal(Color(to_hex(hsva.to_srgba_unmultiplied())));
            self.push(&format!("{key}.color"), label, true, lens.to(c));
        }

        let display = match &value {
            Shared::Same(ColorRef::Literal(c)) => c.0.trim_start_matches('#').to_string(),
            Shared::Same(ColorRef::Token(t)) => t.to_reference(),
            _ => "Mixed".to_string(),
        };
        let set = lens.clone();
        let parse: Parse = Rc::new(move |s: &str| {
            let c = parse_hex(s)?;
            Some(set.to(ColorRef::Literal(Color(to_hex(c)))))
        });
        self.field(
            ui,
            &format!("{key}.hex"),
            label,
            &display,
            HEX_W,
            None,
            parse,
        );

        let alpha = lens.map(
            |c| match c {
                ColorRef::Literal(c) => parse_hex(&c.0).map(|c| c[3] as f32 / 255.0),
                ColorRef::Token(_) => None,
            },
            |c, a| {
                if let ColorRef::Literal(lit) = c {
                    if let Some(mut rgba) = parse_hex(&lit.0) {
                        rgba[3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
                        *c = ColorRef::Literal(Color(to_hex(rgba)));
                    }
                }
            },
        );
        self.number_w(
            ui,
            nodes,
            &format!("{key}.alpha"),
            label,
            alpha,
            ALPHA,
            44.0,
        );
    }

    /// One button per option; the shared value is highlighted.
    fn choice<T: Clone + PartialEq + 'static>(
        &mut self,
        ui: &mut Ui,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<T>,
        options: &[(T, &str, &str)],
    ) {
        let value = shared(nodes, &lens);
        for (v, text, name) in options {
            let selected = value == Shared::Same(v.clone());
            let resp = ui.selectable_label(selected, *text);
            self.fields.push((format!("{key}.{name}"), resp.rect));
            if resp.clicked() && !selected {
                self.push(key, label, false, lens.to(v.clone()));
            }
        }
    }

    /// A text field that commits on Enter or blur and cancels on Esc. With
    /// `drag`, dragging sideways changes the value live as one gesture.
    #[allow(clippy::too_many_arguments)]
    fn field(
        &mut self,
        ui: &mut Ui,
        key: &str,
        label: &str,
        display: &str,
        width: f32,
        drag: Option<Drag>,
        parse: Parse,
    ) {
        let size = Vec2::new(width, ui.spacing().interact_size.y);
        if let Some(e) = self.editing.as_mut().filter(|e| e.key == key) {
            e.seen = true;
            e.parse = parse;
            let id = ui.id().with(("inspector-field", key));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let resp = ui.put(
                rect,
                TextEdit::singleline(&mut e.buffer)
                    .id(id)
                    .margin(egui::Margin::symmetric(4, 2)),
            );
            self.fields.push((key.to_string(), rect));
            if !e.focused {
                e.focused = true;
                resp.request_focus();
                // Select everything, so typing replaces the value.
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
                    self.commit_typed(e);
                }
            }
            return;
        }

        let sense = if drag.is_some() {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let (rect, resp) = ui.allocate_exact_size(size, sense);
        self.fields.push((key.to_string(), rect));
        self.texts.push((key.to_string(), display.to_string()));
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
            let color = if display == "Mixed" {
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().text_color()
            };
            ui.painter().with_clip_rect(rect.shrink(2.0)).text(
                rect.left_center() + Vec2::new(4.0, 0.0),
                Align2::LEFT_CENTER,
                display,
                FontId::proportional(13.0),
                color,
            );
        }
        let resp = match &drag {
            Some(_) => resp.on_hover_cursor(CursorIcon::ResizeHorizontal),
            None => resp.on_hover_cursor(CursorIcon::Text),
        };
        if let Some(drag) = drag {
            if resp.drag_started() {
                self.drag_total = 0.0;
            }
            if resp.dragged() && resp.drag_delta().x != 0.0 {
                self.drag_total += resp.drag_delta().x;
                self.push(key, label, true, drag(self.drag_total));
            }
        }
        if resp.clicked() {
            if let Some(e) = self.editing.take() {
                self.commit_typed(e);
            }
            self.editing = Some(Editing {
                key: key.to_string(),
                label: label.to_string(),
                buffer: if display == "Mixed" {
                    String::new()
                } else {
                    display.to_string()
                },
                targets: self.targets.clone(),
                focused: false,
                parse,
                seen: true,
            });
        }
    }
}

// --- Node helpers -----------------------------------------------------------

fn fills(n: &Node) -> Option<&Vec<Paint>> {
    match &n.kind {
        NodeKind::Text(t) => Some(&t.fills),
        _ => n.shape().map(|s| &s.fills),
    }
}

fn fills_mut(n: &mut Node) -> Option<&mut Vec<Paint>> {
    if matches!(n.kind, NodeKind::Text(_)) {
        let NodeKind::Text(t) = &mut n.kind else {
            unreachable!()
        };
        return Some(&mut t.fills);
    }
    n.shape_mut().map(|s| &mut s.fills)
}

/// Rect, frame and image corners (§3.4).
fn corner_radius(n: &Node) -> Option<&CornerRadius> {
    match &n.kind {
        NodeKind::Rect(r) => Some(&r.shape.corner_radius),
        NodeKind::Frame(f) => Some(&f.shape.corner_radius),
        NodeKind::Image(i) => Some(&i.corner_radius),
        _ => None,
    }
}

fn corner_radius_mut(n: &mut Node) -> Option<&mut CornerRadius> {
    match &mut n.kind {
        NodeKind::Rect(r) => Some(&mut r.shape.corner_radius),
        NodeKind::Frame(f) => Some(&mut f.shape.corner_radius),
        NodeKind::Image(i) => Some(&mut i.corner_radius),
        _ => None,
    }
}

/// Point the node's style of `class` at `id`.
fn set_style(n: &mut Node, class: StyleClass, id: Option<StyleId>) {
    if let NodeKind::Text(t) = &mut n.kind {
        if class == StyleClass::Text {
            t.text_style = id;
        }
    } else if let Some(s) = n.shape_mut() {
        match class {
            StyleClass::Fill => s.fill_style = id,
            StyleClass::Stroke => s.stroke_style = id,
            StyleClass::Effect => s.effect_style = id,
            StyleClass::Text => {}
        }
    }
}

/// Copy the values of the node's style of `class` onto the node and clear
/// the reference (the same as `doc::resolve::detach_style_patch`).
fn detach_style(n: &mut Node, class: StyleClass, styles: &[doc::Style]) {
    let Some(id) = styles_panel::style_of(n, class) else {
        return;
    };
    let kind = styles.iter().find(|s| s.id == id).map(|s| &s.kind);
    match (&mut n.kind, kind) {
        (NodeKind::Text(t), Some(doc::StyleKind::Text(d))) => t.style = d.style.clone(),
        (_, Some(doc::StyleKind::Fill(f))) => {
            if let Some(s) = n.shape_mut() {
                s.fills = f.fills.clone();
            }
        }
        (_, Some(doc::StyleKind::Stroke(st))) => {
            if let Some(s) = n.shape_mut() {
                s.strokes = st.strokes.clone();
                s.stroke_width = st.width;
            }
        }
        (_, Some(doc::StyleKind::Effect(e))) => {
            if let Some(s) = n.shape_mut() {
                s.shadows = e.shadows.clone();
            }
        }
        _ => {}
    }
    set_style(n, class, None);
}

/// The command that turns the node `base.id` into `new`, where `new` is
/// `base` with an edit applied. Text that sizes itself is measured again;
/// typing a width or height into such text makes it keep that size.
fn node_command(ed: &mut Editor, base: &Node, mut new: Node) -> Option<Command> {
    let cur = ed.doc.node(&base.id)?;
    if let NodeKind::Text(t) = &mut new.kind {
        if new.height != base.height {
            t.sizing = TextSizing::Fixed;
        } else if new.width != base.width && t.sizing == TextSizing::AutoWidth {
            t.sizing = TextSizing::AutoHeight;
        }
        if t.sizing != TextSizing::Fixed {
            if let Some(size) = ed.layout.measure_text(&ed.doc, &new) {
                let round = |v: f64| ((v * 100.0).round() / 100.0) as f32;
                new.width = round(size.width);
                new.height = round(size.height);
            }
        }
    }
    let patch = diff(cur, &new);
    if patch == NodePatch::default() {
        return None;
    }
    let geometry = NodePatch {
        x: patch.x,
        y: patch.y,
        width: patch.width,
        height: patch.height,
        rotation: patch.rotation,
        ..Default::default()
    };
    Some(if patch == geometry {
        Command::SetGeometry {
            id: new.id.clone(),
            x: new.x,
            y: new.y,
            width: new.width,
            height: new.height,
            rotation: new.rotation,
        }
    } else {
        Command::SetProps {
            id: new.id.clone(),
            patch,
        }
    })
}

/// The props the inspector edits that differ between `a` and `b`, with
/// `b`'s values.
fn diff(a: &Node, b: &Node) -> NodePatch {
    fn d<T: PartialEq + Clone>(a: &T, b: &T) -> Option<T> {
        (a != b).then(|| b.clone())
    }
    let mut p = NodePatch {
        x: d(&a.x, &b.x),
        y: d(&a.y, &b.y),
        width: d(&a.width, &b.width),
        height: d(&a.height, &b.height),
        rotation: d(&a.rotation, &b.rotation),
        opacity: d(&a.opacity, &b.opacity),
        ..Default::default()
    };
    let shapes = |sa: &Shape, sb: &Shape, p: &mut NodePatch| {
        p.fills = d(&sa.fills, &sb.fills);
        p.strokes = d(&sa.strokes, &sb.strokes);
        p.stroke_width = d(&sa.stroke_width, &sb.stroke_width);
        p.stroke_align = d(&sa.stroke_align, &sb.stroke_align);
        p.shadows = d(&sa.shadows, &sb.shadows);
        p.fill_style = (sa.fill_style != sb.fill_style).then(|| sb.fill_style.clone());
        p.stroke_style = (sa.stroke_style != sb.stroke_style).then(|| sb.stroke_style.clone());
        p.effect_style = (sa.effect_style != sb.effect_style).then(|| sb.effect_style.clone());
    };
    match (&a.kind, &b.kind) {
        (NodeKind::Frame(fa), NodeKind::Frame(fb)) => {
            shapes(&fa.shape, &fb.shape, &mut p);
            p.corner_radius = d(&fa.shape.corner_radius, &fb.shape.corner_radius);
            p.clip = d(&fa.clip, &fb.clip);
        }
        (NodeKind::Rect(ra), NodeKind::Rect(rb)) => {
            shapes(&ra.shape, &rb.shape, &mut p);
            p.corner_radius = d(&ra.shape.corner_radius, &rb.shape.corner_radius);
        }
        (NodeKind::Ellipse(ea), NodeKind::Ellipse(eb)) => shapes(&ea.shape, &eb.shape, &mut p),
        (NodeKind::Vector(va), NodeKind::Vector(vb)) => shapes(&va.shape, &vb.shape, &mut p),
        (NodeKind::Text(ta), NodeKind::Text(tb)) => {
            p.fills = d(&ta.fills, &tb.fills);
            p.style = d(&ta.style, &tb.style);
            p.text_style = (ta.text_style != tb.text_style).then(|| tb.text_style.clone());
            p.align = d(&ta.align, &tb.align);
            p.sizing = d(&ta.sizing, &tb.sizing);
        }
        (NodeKind::Image(ia), NodeKind::Image(ib)) => {
            p.corner_radius = d(&ia.corner_radius, &ib.corner_radius);
        }
        _ => {}
    }
    layout_section::diff_layout(a, b, &mut p);
    p
}

fn convert_paint(p: &Paint, to: &str) -> Paint {
    let stops = match p {
        Paint::LinearGradient(g) => g.stops.clone(),
        Paint::RadialGradient(g) => g.stops.clone(),
        Paint::Solid(s) => {
            let end = match &s.color {
                ColorRef::Literal(c) => parse_hex(&c.0)
                    .map(|mut c| {
                        c[3] = 0;
                        ColorRef::Literal(Color(to_hex(c)))
                    })
                    .unwrap_or_else(|| ColorRef::literal("#ffffff")),
                t => t.clone(),
            };
            vec![
                Stop {
                    offset: 0.0,
                    color: s.color.clone(),
                },
                Stop {
                    offset: 1.0,
                    color: end,
                },
            ]
        }
        Paint::Image(_) => vec![
            Stop {
                offset: 0.0,
                color: ColorRef::literal("#000000"),
            },
            Stop {
                offset: 1.0,
                color: ColorRef::literal("#ffffff"),
            },
        ],
    };
    match to {
        "Linear" => Paint::LinearGradient(LinearGradient {
            start: [0.0, 0.5],
            end: [1.0, 0.5],
            stops,
            extra: Extra::new(),
        }),
        "Radial" => Paint::RadialGradient(RadialGradient {
            center: [0.5, 0.5],
            radius: [0.5, 0.5],
            stops,
            extra: Extra::new(),
        }),
        _ => Paint::solid(
            stops
                .first()
                .map(|s| s.color.clone())
                .unwrap_or_else(|| ColorRef::literal("#d9d9d9")),
        ),
    }
}

// --- Colours and numbers ----------------------------------------------------

/// `rrggbb` or `rrggbbaa`, with or without `#`.
pub(crate) fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let s = s.trim().trim_start_matches('#');
    if !(s.len() == 6 || s.len() == 8) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if s.len() == 8 { byte(6)? } else { 255 },
    ])
}

pub(crate) fn to_hex([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

pub(crate) fn resolve_ref(doc: &Document, c: &ColorRef) -> Option<[u8; 4]> {
    match c {
        ColorRef::Literal(c) => parse_hex(&c.0),
        ColorRef::Token(t) => doc.tokens.colors.get(t).and_then(|c| parse_hex(&c.0)),
    }
}

/// At most `decimals` decimals, no trailing zeros.
pub(crate) fn fmt_num(v: f32, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Id, IdKind, Rect as RectKind};

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_hex("FF800080"), Some([255, 128, 0, 128]));
        assert_eq!(parse_hex("#ff80"), None);
        assert_eq!(parse_hex("#gg8000"), None);
        assert_eq!(to_hex([255, 128, 0, 255]), "#ff8000");
        assert_eq!(to_hex([255, 128, 0, 128]), "#ff800080");
    }

    #[test]
    fn numbers_show_at_most_two_decimals() {
        assert_eq!(fmt_num(100.0, 2), "100");
        assert_eq!(fmt_num(1.5, 2), "1.5");
        assert_eq!(fmt_num(0.126, 2), "0.13");
        assert_eq!(fmt_num(-0.001, 2), "0");
        assert_eq!(fmt_num(50.2, 0), "50");
    }

    #[test]
    fn diff_picks_geometry_or_props() {
        let mut ed = Editor::new();
        let n = Node::new(
            Id::new(IdKind::Node),
            "r",
            NodeKind::Rect(RectKind::default()),
        )
        .with_geometry(0.0, 0.0, 10.0, 10.0);
        let parent = ed.page_ref();
        ed.commit(
            "Add",
            vec![Command::CreateNode {
                parent,
                index: None,
                node: n.clone(),
            }],
            vec![],
        )
        .unwrap();
        let mut moved = n.clone();
        moved.x = 5.0;
        assert!(matches!(
            node_command(&mut ed, &n, moved),
            Some(Command::SetGeometry { x: 5.0, .. })
        ));
        let mut faded = n.clone();
        faded.opacity = 0.5;
        match node_command(&mut ed, &n, faded) {
            Some(Command::SetProps { patch, .. }) => {
                assert_eq!(patch.opacity, Some(0.5));
                assert_eq!(patch.x, None);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(node_command(&mut ed, &n, n.clone()), None);
    }

    #[test]
    fn solid_to_gradient_keeps_the_color() {
        let p = Paint::solid(ColorRef::literal("#ff0000"));
        let Paint::LinearGradient(g) = convert_paint(&p, "Linear") else {
            panic!()
        };
        assert_eq!(g.stops[0].color, ColorRef::literal("#ff0000"));
        assert_eq!(g.stops[1].color, ColorRef::literal("#ff000000"));
        assert_eq!(convert_paint(&Paint::LinearGradient(g), "Solid"), p);
    }
}
