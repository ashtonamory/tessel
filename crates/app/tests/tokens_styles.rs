//! The tokens and styles panels and the inspector's pickers, driven
//! headlessly: synthetic clicks and typing, then checks of the document and
//! history.

mod common;

use app::app::LeftTab;
use common::Harness;
use doc::resolve::{effective_fills, resolve_color};
use doc::{ColorRef, CornerRadius, Node, Paint, RadiusRef, StyleId, StyleKind, TokenId};
use egui::{Key, Modifiers, Pos2};

/// One rectangle at (0, 0), 100×100, selected.
fn one_rect() -> Harness {
    let mut h = Harness::new();
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(0.0, 0.0), h.at(100.0, 100.0));
    h.step(vec![]);
    h
}

fn depth(h: &Harness) -> usize {
    h.ed().history.transactions().len()
}

fn tab(h: &mut Harness, tab: LeftTab) {
    let r = h.app.left_tab_rect(tab).expect("tab drawn").center();
    h.click(r);
    assert_eq!(h.app.left_tab(), tab);
}

fn token_field(h: &Harness, key: &str) -> Pos2 {
    h.app
        .tokens()
        .field_rect(key)
        .unwrap_or_else(|| panic!("no tokens field {key}"))
        .center()
}

fn style_field(h: &Harness, key: &str) -> Pos2 {
    h.app
        .styles()
        .field_rect(key)
        .unwrap_or_else(|| panic!("no styles field {key}"))
        .center()
}

fn inspector_field(h: &Harness, key: &str) -> Pos2 {
    h.app
        .inspector()
        .field_rect(key)
        .unwrap_or_else(|| {
            panic!(
                "no inspector field {key}; drawn: {:?}",
                h.app.inspector().field_keys()
            )
        })
        .center()
}

/// Click a panel field, type over it and press Enter.
fn type_at(h: &mut Harness, at: Pos2, value: &str) {
    h.click(at);
    h.type_text(value);
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
}

fn type_tok(h: &mut Harness, key: &str, value: &str) {
    let at = token_field(h, key);
    type_at(h, at, value);
}

fn type_sty(h: &mut Harness, key: &str, value: &str) {
    let at = style_field(h, key);
    type_at(h, at, value);
}

fn token(name: &str) -> TokenId {
    TokenId(name.to_string())
}

fn the_rect(h: &Harness) -> Node {
    h.children()[0].clone()
}

fn fill_ref(n: &Node) -> ColorRef {
    match &n.shape().expect("a shape").fills[0] {
        Paint::Solid(s) => s.color.clone(),
        other => panic!("not solid: {other:?}"),
    }
}

/// The colour the rectangle's fill resolves to.
fn effective_fill(h: &Harness) -> String {
    let doc = &h.ed().doc;
    let n = doc.node(&the_rect(h).id).unwrap();
    match &effective_fills(doc, n)[0] {
        Paint::Solid(s) => resolve_color(doc, &s.color).expect("resolves").0.clone(),
        other => panic!("not solid: {other:?}"),
    }
}

/// Add a colour token on the Tokens tab; returns to nothing else.
fn add_color_token(h: &mut Harness) {
    tab(h, LeftTab::Tokens);
    h.click(token_field(h, "add.color"));
}

/// Pick token `name` for the rectangle's first fill in the inspector.
fn pick_fill_token(h: &mut Harness, name: &str) {
    h.click(inspector_field(h, "fill.0.token"));
    h.click(inspector_field(h, &format!("fill.0.token.item.{name}")));
}

#[test]
fn add_and_rename_a_color_token_updates_references_in_one_undo_step() {
    let mut h = one_rect();
    let d0 = depth(&h);
    add_color_token(&mut h);
    assert_eq!(depth(&h), d0 + 1);
    assert!(h.ed().doc.tokens.colors.contains_key(&token("color.new")));

    pick_fill_token(&mut h, "color.new");
    assert_eq!(fill_ref(&the_rect(&h)), ColorRef::Token(token("color.new")));
    let d1 = depth(&h);

    type_tok(&mut h, "color.new.name", "color.primary");
    let t = &h.ed().doc.tokens;
    assert!(t.colors.contains_key(&token("color.primary")));
    assert!(!t.colors.contains_key(&token("color.new")));
    assert_eq!(
        fill_ref(&the_rect(&h)),
        ColorRef::Token(token("color.primary"))
    );
    assert_eq!(depth(&h), d1 + 1);

    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(depth(&h), d1);
    assert!(h.ed().doc.tokens.colors.contains_key(&token("color.new")));
    assert_eq!(fill_ref(&the_rect(&h)), ColorRef::Token(token("color.new")));
}

#[test]
fn bad_and_duplicate_token_names_are_refused() {
    let mut h = one_rect();
    add_color_token(&mut h);
    let d = depth(&h);
    type_tok(&mut h, "color.new.name", "primary");
    assert!(h.app.tokens().notice().unwrap().contains("token name"));
    assert!(h.ed().doc.tokens.colors.contains_key(&token("color.new")));
    assert_eq!(depth(&h), d);

    h.click(token_field(&h, "add.color"));
    assert!(h.ed().doc.tokens.colors.contains_key(&token("color.new-2")));
    type_tok(&mut h, "color.new-2.name", "color.new");
    assert!(h.app.tokens().notice().unwrap().contains("already exists"));
}

#[test]
fn deleting_a_used_token_is_refused_and_an_unused_one_is_not() {
    let mut h = one_rect();
    add_color_token(&mut h);
    pick_fill_token(&mut h, "color.new");
    let d = depth(&h);

    h.click(token_field(&h, "color.new.delete"));
    assert!(h.ed().doc.tokens.colors.contains_key(&token("color.new")));
    assert_eq!(depth(&h), d);
    let notice = h.app.tokens().notice().expect("a refusal");
    assert!(notice.contains("Can't delete color.new"), "{notice}");

    // Unlinking the fill frees the token.
    h.click(inspector_field(&h, "fill.0.token"));
    h.click(inspector_field(&h, "fill.0.token.none"));
    assert!(matches!(fill_ref(&the_rect(&h)), ColorRef::Literal(_)));
    h.click(token_field(&h, "color.new.delete"));
    assert!(h.ed().doc.tokens.colors.is_empty());
    assert!(h.app.tokens().notice().is_none());
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(h.ed().doc.tokens.colors.contains_key(&token("color.new")));
}

#[test]
fn changing_a_picked_token_changes_the_effective_fill() {
    let mut h = one_rect();
    add_color_token(&mut h);
    pick_fill_token(&mut h, "color.new");
    assert_eq!(effective_fill(&h), "#808080");

    type_tok(&mut h, "color.new.value", "#ff0000");
    assert_eq!(effective_fill(&h), "#ff0000");
    // The node still holds the reference, not the value.
    assert_eq!(fill_ref(&the_rect(&h)), ColorRef::Token(token("color.new")));
    // The inspector shows the token it is bound to.
    assert_eq!(
        h.app.inspector().field_text("fill.0.hex"),
        Some("{color.new}")
    );
}

#[test]
fn a_value_that_is_not_a_color_is_refused() {
    let mut h = one_rect();
    add_color_token(&mut h);
    let d = depth(&h);
    type_tok(&mut h, "color.new.value", "red");
    assert_eq!(depth(&h), d);
    assert!(h.app.tokens().notice().unwrap().contains("#rrggbb"));
}

#[test]
fn a_radius_token_sets_the_corner_radius() {
    let mut h = one_rect();
    tab(&mut h, LeftTab::Tokens);
    h.click(token_field(&h, "add.radius"));
    type_tok(&mut h, "radius.new.value", "12");
    assert_eq!(h.ed().doc.tokens.radii[&token("radius.new")], 12.0);

    let d = depth(&h);
    h.click(inspector_field(&h, "radius.token"));
    h.click(inspector_field(&h, "radius.token.item.radius.new"));
    assert_eq!(depth(&h), d + 1);
    let corner = |h: &Harness| match the_rect(h).kind {
        doc::NodeKind::Rect(r) => r.shape.corner_radius,
        _ => panic!("a rect"),
    };
    // The node links the token; the inspector shows the reference.
    assert_eq!(
        corner(&h),
        CornerRadius::All(RadiusRef::Token(token("radius.new")))
    );
    assert_eq!(h.app.inspector().field_text("radius"), Some("{radius.new}"));
    let effective = |h: &Harness| doc::resolve::effective_corner_radius(&h.ed().doc, &the_rect(h));
    assert_eq!(effective(&h), [12.0; 4]);

    // Editing the token moves the node's radius, one undo step each.
    type_tok(&mut h, "radius.new.value", "20");
    assert_eq!(depth(&h), d + 2);
    assert_eq!(effective(&h), [20.0; 4]);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(effective(&h), [12.0; 4]);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(corner(&h), CornerRadius::px(0.0));

    // Unlinking gives back the px value.
    h.key(Key::Y, Modifiers::COMMAND);
    h.click(inspector_field(&h, "radius.token"));
    h.click(inspector_field(&h, "radius.token.none"));
    assert_eq!(corner(&h), CornerRadius::px(12.0));
}

#[test]
fn per_corner_radius_tokens() {
    let mut h = one_rect();
    tab(&mut h, LeftTab::Tokens);
    h.click(token_field(&h, "add.radius"));
    type_tok(&mut h, "radius.new.value", "12");
    h.click(inspector_field(&h, "radius.each"));
    h.click(inspector_field(&h, "radius.2.token"));
    h.click(inspector_field(&h, "radius.2.token.item.radius.new"));
    let r = match the_rect(&h).kind {
        doc::NodeKind::Rect(r) => r.shape.corner_radius,
        _ => panic!("a rect"),
    };
    let t = RadiusRef::Token(token("radius.new"));
    assert_eq!(
        r,
        CornerRadius::Each([
            RadiusRef::Px(0.0),
            RadiusRef::Px(0.0),
            t,
            RadiusRef::Px(0.0)
        ])
    );
    assert_eq!(
        h.app.inspector().field_text("radius.2"),
        Some("{radius.new}")
    );
}

fn only_style(h: &Harness) -> doc::Style {
    assert_eq!(h.ed().doc.styles.len(), 1);
    h.ed().doc.styles[0].clone()
}

fn make_fill_style(h: &mut Harness) -> StyleId {
    tab(h, LeftTab::Styles);
    h.click(style_field(h, "new.fill"));
    only_style(h).id
}

#[test]
fn creating_a_fill_style_from_the_selection_uses_it() {
    let mut h = one_rect();
    let before = the_rect(&h);
    let d = depth(&h);
    let id = make_fill_style(&mut h);
    assert_eq!(depth(&h), d + 1);

    let style = only_style(&h);
    let StyleKind::Fill(f) = &style.kind else {
        panic!("a fill style")
    };
    assert_eq!(f.fills, before.shape().unwrap().fills);
    assert_eq!(the_rect(&h).shape().unwrap().fill_style, Some(id));
    // The inspector now shows the style, not the fill list.
    assert!(h.app.inspector().field_rect("fill.0.hex").is_none());

    h.key(Key::Z, Modifiers::COMMAND);
    assert!(h.ed().doc.styles.is_empty());
    assert_eq!(the_rect(&h).shape().unwrap().fill_style, None);
}

#[test]
fn a_style_needs_a_layer_that_can_have_it() {
    let mut h = Harness::new();
    tab(&mut h, LeftTab::Styles);
    h.click(style_field(&h, "new.fill"));
    assert!(h.ed().doc.styles.is_empty());
}

#[test]
fn applying_and_detaching_a_style() {
    let mut h = one_rect();
    let id = make_fill_style(&mut h);
    let name = only_style(&h).name;
    let style_fills = match only_style(&h).kind {
        StyleKind::Fill(f) => f.fills,
        _ => unreachable!(),
    };

    // Edit the style: the node follows it.
    type_sty(&mut h, &format!("{id}.color"), "#00ff00");
    assert_eq!(effective_fill(&h), "#00ff00");

    // Detach: the node keeps the style's values and loses the reference.
    let d = depth(&h);
    h.click(inspector_field(&h, "fill.style"));
    h.click(inspector_field(&h, "fill.style.none"));
    assert_eq!(depth(&h), d + 1);
    let n = the_rect(&h);
    assert_eq!(n.shape().unwrap().fill_style, None);
    assert_eq!(n.shape().unwrap().fills.len(), 1);
    assert_eq!(effective_fill(&h), "#00ff00");
    assert_ne!(n.shape().unwrap().fills, style_fills);
    assert_eq!(h.app.inspector().field_text("fill.0.hex"), Some("00ff00"));

    // Apply it again.
    h.click(inspector_field(&h, "fill.style"));
    h.click(inspector_field(&h, &format!("fill.style.item.{name}")));
    assert_eq!(depth(&h), d + 2);
    assert_eq!(the_rect(&h).shape().unwrap().fill_style, Some(id));
}

#[test]
fn styles_can_be_renamed_and_grouped_by_slash() {
    let mut h = one_rect();
    let id = make_fill_style(&mut h);
    let d = depth(&h);
    type_sty(&mut h, &format!("{id}.name"), "Surface/Card");
    assert_eq!(only_style(&h).name, "Surface/Card");
    assert_eq!(depth(&h), d + 1);
    assert_eq!(
        h.app.styles().field_text(&format!("{id}.name")),
        Some("Surface/Card")
    );
}

#[test]
fn deleting_a_used_style_confirms_and_detaches() {
    let mut h = one_rect();
    let id = make_fill_style(&mut h);
    let fills = match only_style(&h).kind {
        StyleKind::Fill(f) => f.fills,
        _ => unreachable!(),
    };
    let d = depth(&h);

    h.click(style_field(&h, &format!("{id}.delete")));
    assert_eq!(h.app.styles().pending_delete(), Some((&id, 1)));
    assert_eq!(h.ed().doc.styles.len(), 1);
    assert_eq!(depth(&h), d);

    // Cancel keeps it.
    h.click(style_field(&h, "confirm.cancel"));
    assert!(h.app.styles().pending_delete().is_none());
    assert_eq!(h.ed().doc.styles.len(), 1);

    h.click(style_field(&h, &format!("{id}.delete")));
    h.click(style_field(&h, "confirm.ok"));
    assert!(h.ed().doc.styles.is_empty());
    assert_eq!(depth(&h), d + 1);
    let n = the_rect(&h);
    assert_eq!(n.shape().unwrap().fill_style, None);
    assert_eq!(n.shape().unwrap().fills, fills);

    // One undo brings the style and the reference back.
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.ed().doc.styles.len(), 1);
    assert_eq!(the_rect(&h).shape().unwrap().fill_style, Some(id));
}

#[test]
fn deleting_an_unused_style_needs_no_confirmation() {
    let mut h = one_rect();
    let id = make_fill_style(&mut h);
    h.click(inspector_field(&h, "fill.style"));
    h.click(inspector_field(&h, "fill.style.none"));
    h.click(style_field(&h, &format!("{id}.delete")));
    assert!(h.app.styles().pending_delete().is_none());
    assert!(h.ed().doc.styles.is_empty());
}
