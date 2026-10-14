//! The inspector's layout sections (§3.5, §9): auto-layout of frames,
//! how a node sits in an auto-layout parent, and constraints in frames
//! without auto-layout. A child module of `inspector`, so it uses the same
//! fields, lenses and commit path: one command per edit, one undo step.
//!
//! Gap and padding are px fields; a value that is a spacing token shows
//! the token's reference and typing a number replaces it with px.

use std::rc::Rc;

use doc::{
    Align, AutoLayout, ChildPosition, Constraints, Direction, Document, HConstraint, Justify,
    LayoutChild, Node, NodeKind, NodePatch, ParentRef, RadiusRef, Sizing, SpaceRef, TokenId,
    VConstraint,
};
use egui::{Context, Id, Ui};

use super::{fmt_num, shared, Drag, Inspector, Lens, Parse, Shared, FIELD_W, SIZE};
use crate::editor::Editor;
use crate::pickers;

/// Asked for by a button and done by `apply_actions`, since adding or
/// removing auto-layout also moves children.
#[derive(Clone, Copy, Default, PartialEq)]
enum Action {
    #[default]
    None,
    Add,
    Remove,
}

fn action_id() -> Id {
    Id::new("inspector-layout-action")
}

fn each_side_id() -> Id {
    Id::new("inspector-layout-padding-each")
}

/// Run the add/remove asked for by the last `ui`.
pub(super) fn apply_actions(ctx: &Context, ed: &mut Editor) {
    match ctx.data_mut(|d| d.remove_temp::<Action>(action_id())) {
        Some(Action::Add) => {
            ed.add_auto_layout();
        }
        Some(Action::Remove) => {
            ed.remove_auto_layout();
        }
        Some(Action::None) | None => {}
    }
}

/// The layout props that differ between `a` and `b`, with `b`'s values,
/// for the inspector's `diff`.
pub(super) fn diff_layout(a: &Node, b: &Node, p: &mut NodePatch) {
    if a.layout_child != b.layout_child {
        p.layout_child = Some(b.layout_child.clone());
    }
    if a.constraints != b.constraints {
        p.constraints = Some(b.constraints.clone());
    }
    if let (NodeKind::Frame(fa), NodeKind::Frame(fb)) = (&a.kind, &b.kind) {
        if fa.layout != fb.layout {
            p.layout = Some(fb.layout.clone());
        }
    }
}

fn layout_lens() -> Lens<AutoLayout> {
    Lens::new(
        |n| match &n.kind {
            NodeKind::Frame(f) => f.layout.clone(),
            _ => None,
        },
        |n, v| {
            if let NodeKind::Frame(f) = &mut n.kind {
                f.layout = Some(v);
            }
        },
    )
}

fn child_lens() -> Lens<LayoutChild> {
    Lens::new(|n| Some(n.layout_child.clone()), |n, v| n.layout_child = v)
}

fn constraints_lens() -> Lens<Constraints> {
    Lens::new(|n| Some(n.constraints.clone()), |n, v| n.constraints = v)
}

/// The node's parent, when it is a node.
fn parent<'a>(doc: &'a Document, n: &Node) -> Option<&'a Node> {
    match doc.parent_of(&n.id)? {
        (ParentRef::Node(p), _) => doc.node(&p),
        _ => None,
    }
}

fn has_layout(n: &Node) -> bool {
    matches!(&n.kind, NodeKind::Frame(f) if f.layout.is_some())
}

const SIZING: [(Sizing, &str, &str); 3] = [
    (Sizing::Fixed, "Fixed", "fixed"),
    (Sizing::Hug, "Hug", "hug"),
    (Sizing::Fill, "Fill", "fill"),
];

impl Inspector {
    /// The auto-layout section for frames, the child section for children
    /// of auto-layout frames and constraints for children of frames
    /// without it (and absolute children of frames with it).
    pub(super) fn layout_sections(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        if nodes.iter().all(|n| matches!(n.kind, NodeKind::Frame(_))) {
            ui.separator();
            self.auto_layout(ui, doc, nodes);
        }
        let parents: Vec<Option<&Node>> = nodes.iter().map(|n| parent(doc, n)).collect();
        if parents.iter().all(|p| p.is_some_and(has_layout)) {
            ui.separator();
            self.layout_child(ui, nodes);
        }
        let absolute = nodes.iter().zip(&parents).all(|(n, p)| match p {
            Some(p) if has_layout(p) => n.layout_child.position == ChildPosition::Absolute,
            Some(p) => matches!(p.kind, NodeKind::Frame(_)),
            None => false,
        });
        if absolute {
            ui.separator();
            self.constraints(ui, nodes);
        }
    }

    fn auto_layout(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        let all = nodes.iter().all(|n| has_layout(n));
        ui.horizontal(|ui| {
            ui.strong("Auto layout");
            let (text, hint, action) = if all {
                ("−", "Remove auto layout (Shift+A)", Action::Remove)
            } else {
                ("+", "Add auto layout (Shift+A)", Action::Add)
            };
            let resp = ui.small_button(text).on_hover_text(hint);
            self.fields.push(("layout.toggle".into(), resp.rect));
            if resp.clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(action_id(), action));
            }
        });
        if !all {
            return;
        }
        let al = layout_lens();
        ui.horizontal(|ui| {
            ui.label("Direction");
            let dir = al.map(|a| Some(a.direction), |a, v| a.direction = v);
            self.choice(
                ui,
                nodes,
                "layout.direction",
                "Change layout direction",
                dir,
                &[
                    (Direction::Row, "→ Row", "row"),
                    (Direction::Column, "↓ Column", "column"),
                ],
            );
            let wrap = al.map(|a| Some(a.wrap), |a, v| a.wrap = v);
            let value = shared(nodes, &wrap);
            let mut checked = value == Shared::Same(true);
            let resp = ui.add(
                egui::Checkbox::new(&mut checked, "Wrap").indeterminate(value == Shared::Mixed),
            );
            self.fields.push(("layout.wrap".into(), resp.rect));
            if resp.clicked() {
                let v = value != Shared::Same(true);
                self.push("layout.wrap", "Change wrap", false, wrap.to(v));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Gap");
            let gap = al.map(|a| Some(a.gap.clone()), |a, v| a.gap = v);
            self.space(ui, doc, nodes, "layout.gap", "Change gap", gap);
        });
        let wraps = nodes.iter().any(
            |n| matches!(&n.kind, NodeKind::Frame(f) if f.layout.as_ref().is_some_and(|l| l.wrap)),
        );
        if wraps {
            ui.horizontal(|ui| {
                ui.label("Lines");
                // Without a cross gap, lines are `gap` apart (CSS `gap`).
                let cross = al.map(
                    |a| Some(a.cross_gap.clone().unwrap_or_else(|| a.gap.clone())),
                    |a, v| a.cross_gap = Some(v),
                );
                self.space(ui, doc, nodes, "layout.cross_gap", "Change line gap", cross);
            });
        }
        self.padding(ui, doc, nodes, &al);
        ui.horizontal_wrapped(|ui| {
            ui.label("Justify");
            let justify = al.map(|a| Some(a.justify), |a, v| a.justify = v);
            self.choice(
                ui,
                nodes,
                "layout.justify",
                "Change justify",
                justify,
                &[
                    (Justify::Start, "Start", "start"),
                    (Justify::Center, "Center", "center"),
                    (Justify::End, "End", "end"),
                    (Justify::SpaceBetween, "Between", "space_between"),
                ],
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Align");
            let align = al.map(|a| Some(a.align), |a, v| a.align = v);
            self.choice(
                ui,
                nodes,
                "layout.align",
                "Change align",
                align,
                &[
                    (Align::Start, "Start", "start"),
                    (Align::Center, "Center", "center"),
                    (Align::End, "End", "end"),
                    (Align::Stretch, "Stretch", "stretch"),
                    (Align::Baseline, "Baseline", "baseline"),
                ],
            );
        });
        ui.horizontal(|ui| {
            ui.label("W");
            let w = al.map(|a| Some(a.width), |a, v| a.width = v);
            self.choice(ui, nodes, "layout.width", "Change width sizing", w, &SIZING);
        });
        ui.horizontal(|ui| {
            ui.label("H");
            let h = al.map(|a| Some(a.height), |a, v| a.height = v);
            self.choice(
                ui,
                nodes,
                "layout.height",
                "Change height sizing",
                h,
                &SIZING,
            );
        });
    }

    fn padding(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node], al: &Lens<AutoLayout>) {
        let padding = al.map(|a| Some(a.padding.clone()), |a, v| a.padding = v);
        let uneven = nodes.iter().any(|n| match &n.kind {
            NodeKind::Frame(f) => f
                .layout
                .as_ref()
                .is_some_and(|l| l.padding.iter().any(|p| *p != l.padding[0])),
            _ => false,
        });
        let ctx = ui.ctx().clone();
        let each = uneven || ctx.data(|d| d.get_temp::<bool>(each_side_id()).unwrap_or(false));
        ui.horizontal(|ui| {
            ui.label("Padding");
            if !each {
                let all = padding.map(
                    |p| p.iter().all(|s| *s == p[0]).then(|| p[0].clone()),
                    |p, v| *p = std::array::from_fn(|_| v.clone()),
                );
                self.space(ui, doc, nodes, "layout.padding", "Change padding", all);
            }
            let resp = ui
                .selectable_label(each, "⛶")
                .on_hover_text("Padding per side");
            self.fields.push(("layout.padding.each".into(), resp.rect));
            if resp.clicked() {
                if each {
                    ctx.data_mut(|d| d.insert_temp(each_side_id(), false));
                    // Back to one value: the top's.
                    let to_all = padding.modify(|p| *p = std::array::from_fn(|_| p[0].clone()));
                    self.push("layout.padding", "Change padding", false, to_all);
                } else {
                    ctx.data_mut(|d| d.insert_temp(each_side_id(), true));
                }
            }
        });
        if each {
            // One row per side: each has a field and a token picker.
            for (i, (label, name)) in [("↑", "top"), ("→", "right"), ("↓", "bottom"), ("←", "left")]
                .into_iter()
                .enumerate()
            {
                ui.horizontal(|ui| {
                    ui.label(label);
                    let side = padding.map(move |p| Some(p[i].clone()), move |p, v| p[i] = v);
                    self.space_w(
                        ui,
                        doc,
                        nodes,
                        &format!("layout.padding.{name}"),
                        "Change padding",
                        side,
                        40.0,
                    );
                });
            }
        }
    }

    fn layout_child(&mut self, ui: &mut Ui, nodes: &[&Node]) {
        ui.strong("Auto layout child");
        let lc = child_lens();
        ui.horizontal(|ui| {
            ui.label("Position");
            let position = lc.map(|c| Some(c.position), |c, v| c.position = v);
            self.choice(
                ui,
                nodes,
                "child.position",
                "Change position",
                position,
                &[
                    (ChildPosition::Flow, "Flow", "flow"),
                    (ChildPosition::Absolute, "Absolute", "absolute"),
                ],
            );
        });
        ui.horizontal(|ui| {
            ui.label("W");
            let w = lc.map(|c| Some(c.width), |c, v| c.width = v);
            self.choice(ui, nodes, "child.width", "Change width sizing", w, &SIZING);
        });
        ui.horizontal(|ui| {
            ui.label("H");
            let h = lc.map(|c| Some(c.height), |c, v| c.height = v);
            self.choice(
                ui,
                nodes,
                "child.height",
                "Change height sizing",
                h,
                &SIZING,
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Align self");
            let a = lc.map(|c| Some(c.align_self), |c, v| c.align_self = v);
            self.choice(
                ui,
                nodes,
                "child.align_self",
                "Change align self",
                a,
                &[
                    (None, "Auto", "auto"),
                    (Some(Align::Start), "Start", "start"),
                    (Some(Align::Center), "Center", "center"),
                    (Some(Align::End), "End", "end"),
                    (Some(Align::Stretch), "Stretch", "stretch"),
                ],
            );
        });
        type Field = fn(&mut LayoutChild) -> &mut Option<f32>;
        let limits: [[(&str, &str, Field); 2]; 2] = [
            [
                ("Min W", "min_width", |c| &mut c.min_width),
                ("Max W", "max_width", |c| &mut c.max_width),
            ],
            [
                ("Min H", "min_height", |c| &mut c.min_height),
                ("Max H", "max_height", |c| &mut c.max_height),
            ],
        ];
        for row in limits {
            ui.horizontal(|ui| {
                for (label, name, field) in row {
                    ui.label(label);
                    let lens = lc.map(
                        move |c| Some(*field(&mut c.clone())),
                        move |c, v| *field(c) = v,
                    );
                    self.optional_size(ui, nodes, &format!("child.{name}"), "Change limit", lens);
                }
            });
        }
    }

    fn constraints(&mut self, ui: &mut Ui, nodes: &[&Node]) {
        ui.strong("Constraints");
        let c = constraints_lens();
        ui.horizontal_wrapped(|ui| {
            ui.label("H");
            let h = c.map(|c| Some(c.horizontal), |c, v| c.horizontal = v);
            self.choice(
                ui,
                nodes,
                "constraints.h",
                "Change constraints",
                h,
                &[
                    (HConstraint::Left, "Left", "left"),
                    (HConstraint::Right, "Right", "right"),
                    (HConstraint::LeftRight, "Left & right", "left_right"),
                    (HConstraint::Center, "Center", "center"),
                    (HConstraint::Scale, "Scale", "scale"),
                ],
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("V");
            let v = c.map(|c| Some(c.vertical), |c, v| c.vertical = v);
            self.choice(
                ui,
                nodes,
                "constraints.v",
                "Change constraints",
                v,
                &[
                    (VConstraint::Top, "Top", "top"),
                    (VConstraint::Bottom, "Bottom", "bottom"),
                    (VConstraint::TopBottom, "Top & bottom", "top_bottom"),
                    (VConstraint::Center, "Center", "center"),
                    (VConstraint::Scale, "Scale", "scale"),
                ],
            );
        });
    }

    // --- Widgets ------------------------------------------------------------

    fn space(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<SpaceRef>,
    ) {
        self.space_w(ui, doc, nodes, key, label, lens, FIELD_W);
    }

    /// A spacing field: px, dragged or typed; a token shows its reference
    /// (`{spacing.md}`) and typing a number replaces it with px. Typing a
    /// reference sets a token, and so does the token picker beside it
    /// (`{key}.token`), whose "Unlink" turns the token into its px value.
    #[allow(clippy::too_many_arguments)]
    fn space_w(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<SpaceRef>,
        width: f32,
    ) {
        let value = shared(nodes, &lens);
        let display = match &value {
            Shared::Same(SpaceRef::Px(v)) => fmt_num(*v, 2),
            Shared::Same(SpaceRef::Token(t)) => t.to_reference(),
            Shared::Mixed => "Mixed".to_string(),
            Shared::Absent => return,
        };
        let picker = lens.clone();
        let set = lens.clone();
        let parse: Parse = Rc::new(move |s: &str| {
            let s = s.trim();
            if let Some(t) = TokenId::from_reference(s) {
                return Some(set.to(SpaceRef::Token(t)));
            }
            let v: f32 = s.parse().ok()?;
            v.is_finite().then(|| set.to(SpaceRef::Px(v.max(0.0))))
        });
        let tokens = doc.tokens.spacing.clone();
        let drag: Drag = Rc::new(move |points: f32| {
            let d = points.round();
            let tokens = tokens.clone();
            lens.modify(move |s| {
                let v = match s {
                    SpaceRef::Px(v) => *v,
                    SpaceRef::Token(t) => tokens.get(t).copied().unwrap_or(0.0),
                };
                *s = SpaceRef::Px((v + d).max(0.0));
            })
        });
        self.field(ui, key, label, &display, width, Some(drag), parse);
        let current = match value {
            Shared::Same(s) => s,
            _ => SpaceRef::Px(0.0),
        };
        let token_key = format!("{key}.token");
        if let Some(s) =
            pickers::space_token_picker(ui, doc, &mut self.fields, &token_key, &current)
        {
            self.push(&token_key, label, false, picker.to(s));
        }
    }

    /// A corner radius field: px, dragged or typed; a token shows its
    /// reference (`{radius.md}`) and typing a number replaces it with px.
    /// Typing a reference sets a token, and so does the token picker beside
    /// it (`{key}.token`), whose "Unlink" turns the token into its px value.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn radius_w(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        lens: Lens<RadiusRef>,
        width: f32,
        picker_beside: bool,
    ) {
        let value = shared(nodes, &lens);
        let display = match &value {
            Shared::Same(RadiusRef::Px(v)) => fmt_num(*v, 2),
            Shared::Same(RadiusRef::Token(t)) => t.to_reference(),
            Shared::Mixed => "Mixed".to_string(),
            Shared::Absent => return,
        };
        let picker = lens.clone();
        let set = lens.clone();
        let parse: Parse = Rc::new(move |s: &str| {
            let s = s.trim();
            if let Some(t) = TokenId::from_reference(s) {
                return Some(set.to(RadiusRef::Token(t)));
            }
            let v: f32 = s.parse().ok()?;
            v.is_finite().then(|| set.to(RadiusRef::Px(v.max(0.0))))
        });
        let tokens = doc.tokens.radii.clone();
        let drag: Drag = Rc::new(move |points: f32| {
            let d = points.round();
            let tokens = tokens.clone();
            lens.modify(move |r| {
                let v = match r {
                    RadiusRef::Px(v) => *v,
                    RadiusRef::Token(t) => tokens.get(t).copied().unwrap_or(0.0),
                };
                *r = RadiusRef::Px((v + d).max(0.0));
            })
        });
        self.field(
            ui,
            key,
            "Change corner radius",
            &display,
            width,
            Some(drag),
            parse,
        );
        if picker_beside {
            self.radius_picker(ui, doc, nodes, key, picker);
        }
    }

    /// The token picker (`{key}.token`) of a corner radius field.
    pub(super) fn radius_picker(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        key: &str,
        lens: Lens<RadiusRef>,
    ) {
        let current = match shared(nodes, &lens) {
            Shared::Same(r) => r,
            _ => RadiusRef::Px(0.0),
        };
        let token_key = format!("{key}.token");
        if let Some(r) =
            pickers::radius_token_picker(ui, doc, &mut self.fields, &token_key, &current)
        {
            self.push(&token_key, "Change corner radius", false, lens.to(r));
        }
    }

    /// An optional size: empty shows "–" and typing nothing clears it.
    fn optional_size(
        &mut self,
        ui: &mut Ui,
        nodes: &[&Node],
        key: &str,
        label: &str,
        lens: Lens<Option<f32>>,
    ) {
        let display = match shared(nodes, &lens) {
            Shared::Same(Some(v)) => fmt_num(v, 2),
            Shared::Same(None) => "–".to_string(),
            Shared::Mixed => "Mixed".to_string(),
            Shared::Absent => return,
        };
        let set = lens.clone();
        let parse: Parse = Rc::new(move |s: &str| {
            let s = s.trim();
            if s.is_empty() || s == "–" {
                return Some(set.to(None));
            }
            let v: f32 = s.parse().ok()?;
            v.is_finite()
                .then(|| set.to(Some(v.clamp(SIZE.min, SIZE.max))))
        });
        let drag: Drag = Rc::new(move |points: f32| {
            let d = points.round();
            lens.modify(move |v| *v = Some((v.unwrap_or(0.0) + d).max(0.0)))
        });
        self.field(ui, key, label, &display, 44.0, Some(drag), parse);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Frame, Id as NodeIdGen, IdKind};

    #[test]
    fn diff_reports_layout_props() {
        let a = Node::new(
            NodeIdGen::new(IdKind::Node),
            "f",
            NodeKind::Frame(Frame::default()),
        );
        let mut b = a.clone();
        b.layout_child.width = Sizing::Fill;
        b.constraints.horizontal = HConstraint::Center;
        if let NodeKind::Frame(f) = &mut b.kind {
            f.layout = Some(crate::auto_layout::new_layout(
                Direction::Column,
                Sizing::Hug,
                Sizing::Fixed,
            ));
        }
        let mut p = NodePatch::default();
        diff_layout(&a, &b, &mut p);
        assert_eq!(p.layout_child.unwrap().width, Sizing::Fill);
        assert_eq!(p.constraints.unwrap().horizontal, HConstraint::Center);
        assert_eq!(p.layout.unwrap().unwrap().direction, Direction::Column);
    }
}
