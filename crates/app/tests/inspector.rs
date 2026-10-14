//! The inspector panel, driven headlessly like `ui.rs`: synthetic clicks,
//! drags and typing on its fields, then checks of the document and history.

mod common;

use common::Harness;
use doc::{ColorRef, Node, NodeKind, Paint, Shadow, StrokeAlign};
use egui::{Key, Modifiers, Pos2, Vec2};

fn rect_at(h: &mut Harness, x: f64, w: f64) {
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(x, 0.0), h.at(x + w, 100.0));
}

/// One rectangle at (0, 0), 100×100, selected.
fn one_rect() -> Harness {
    let mut h = Harness::new();
    rect_at(&mut h, 0.0, 100.0);
    h.step(vec![]);
    h
}

/// Two rectangles of different x and width, both selected.
fn two_rects() -> Harness {
    let mut h = Harness::new();
    rect_at(&mut h, 0.0, 100.0);
    rect_at(&mut h, 200.0, 50.0);
    h.key(Key::A, Modifiers::COMMAND);
    h.step(vec![]);
    assert_eq!(h.ed().selection.len(), 2);
    h
}

fn field(h: &Harness, key: &str) -> Pos2 {
    h.app
        .inspector()
        .field_rect(key)
        .unwrap_or_else(|| {
            panic!(
                "no field {key}; drawn: {:?}",
                h.app.inspector().field_keys()
            )
        })
        .center()
}

fn text(h: &Harness, key: &str) -> String {
    h.app
        .inspector()
        .field_text(key)
        .unwrap_or_else(|| panic!("no text field {key}"))
        .to_string()
}

/// Click a field, type `value` over its contents and press Enter.
fn type_into(h: &mut Harness, key: &str, value: &str) {
    h.click(field(h, key));
    assert!(
        h.app.inspector().is_editing(),
        "{key} did not start editing"
    );
    h.type_text(value);
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
}

fn undo_depth(h: &Harness) -> usize {
    h.ed().history.transactions().len()
}

fn shape_of(n: &Node) -> &doc::Shape {
    n.shape().expect("a shape")
}

fn solid(p: &Paint) -> &str {
    match p {
        Paint::Solid(s) => match &s.color {
            ColorRef::Literal(c) => &c.0,
            ColorRef::Token(t) => &t.0,
        },
        other => panic!("not solid: {other:?}"),
    }
}

#[test]
fn shows_the_selected_values() {
    let h = one_rect();
    assert_eq!(text(&h, "x"), "0");
    assert_eq!(text(&h, "width"), "100");
    assert_eq!(text(&h, "opacity"), "100%");
    assert_eq!(text(&h, "fill.0.hex"), "d9d9d9");
}

#[test]
fn typing_x_and_width_is_one_undo_step_each() {
    let mut h = one_rect();
    let depth = undo_depth(&h);
    type_into(&mut h, "x", "42");
    assert_eq!(h.children()[0].x, 42.0);
    assert_eq!(undo_depth(&h), depth + 1);
    assert_eq!(text(&h, "x"), "42");

    type_into(&mut h, "width", "180");
    assert_eq!(h.children()[0].width, 180.0);
    assert_eq!(undo_depth(&h), depth + 2);

    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children()[0].width, 100.0);
    assert_eq!(h.children()[0].x, 42.0);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children()[0].x, 0.0);
}

#[test]
fn typing_opacity_as_a_percentage() {
    let mut h = one_rect();
    type_into(&mut h, "opacity", "50");
    assert_eq!(h.children()[0].opacity, 0.5);
    assert_eq!(text(&h, "opacity"), "50%");
    // Out-of-range values clamp instead of failing validation.
    type_into(&mut h, "opacity", "250%");
    assert_eq!(h.children()[0].opacity, 1.0);
}

#[test]
fn escape_cancels_typing() {
    let mut h = one_rect();
    let depth = undo_depth(&h);
    h.click(field(&h, "x"));
    h.type_text("999");
    h.key(Key::Escape, Modifiers::NONE);
    h.step(vec![]);
    assert!(!h.app.inspector().is_editing());
    assert_eq!(h.children()[0].x, 0.0);
    assert_eq!(undo_depth(&h), depth);
    // Escape went to the field, not the canvas: the selection stays.
    assert_eq!(h.ed().selection.len(), 1);
}

#[test]
fn clicking_another_field_commits_the_first() {
    let mut h = one_rect();
    h.click(field(&h, "x"));
    h.type_text("77");
    h.click(field(&h, "y"));
    h.step(vec![]);
    assert_eq!(h.children()[0].x, 77.0);
    h.type_text("12");
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
    assert_eq!(h.children()[0].y, 12.0);
}

#[test]
fn invalid_text_changes_nothing() {
    let mut h = one_rect();
    let depth = undo_depth(&h);
    type_into(&mut h, "x", "abc");
    assert_eq!(h.children()[0].x, 0.0);
    assert_eq!(undo_depth(&h), depth);
}

/// Drag field `key` sideways by `dx` points in several moves.
fn drag_field(h: &mut Harness, key: &str, dx: f32) {
    let from = field(h, key);
    h.drag(from, from + Vec2::new(dx, 0.0));
}

#[test]
fn dragging_a_number_is_one_gesture() {
    let mut h = one_rect();
    let depth = undo_depth(&h);
    drag_field(&mut h, "x", 40.0);
    let x = h.children()[0].x;
    assert!(x > 20.0, "x = {x}");
    assert_eq!(undo_depth(&h), depth + 1, "one undo step for the drag");
    assert_eq!(h.ed().history.undo_label(), Some("Change X"));
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children()[0].x, 0.0);
    h.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.children()[0].x, x);
}

#[test]
fn dragging_opacity_is_one_gesture() {
    let mut h = one_rect();
    let depth = undo_depth(&h);
    drag_field(&mut h, "opacity", -60.0);
    let o = h.children()[0].opacity;
    assert!(o < 1.0 && o > 0.0, "opacity = {o}");
    assert_eq!(undo_depth(&h), depth + 1);
    // One SetProps for the whole drag, not one per frame.
    let tx = h.ed().history.transactions().last().unwrap();
    assert_eq!(tx.forward.len(), 1, "{:?}", tx.forward);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children()[0].opacity, 1.0);
}

#[test]
fn multi_selection_shows_mixed_and_edits_all() {
    let mut h = two_rects();
    assert_eq!(text(&h, "x"), "Mixed");
    assert_eq!(text(&h, "width"), "Mixed");
    assert_eq!(text(&h, "y"), "0");
    assert_eq!(text(&h, "height"), "100");
    assert_eq!(text(&h, "fill.0.hex"), "d9d9d9");

    let depth = undo_depth(&h);
    type_into(&mut h, "width", "60");
    assert!(h.children().iter().all(|n| n.width == 60.0));
    assert_eq!(text(&h, "width"), "60");
    assert_eq!(undo_depth(&h), depth + 1);

    type_into(&mut h, "opacity", "25");
    assert!(h.children().iter().all(|n| n.opacity == 0.25));

    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    let widths: Vec<f32> = h.children().iter().map(|n| n.width).collect();
    assert_eq!(widths, [100.0, 50.0]);
    assert_eq!(h.ed().selection.len(), 2);
}

#[test]
fn dragging_a_mixed_number_moves_each_by_the_same_amount() {
    let mut h = two_rects();
    let depth = undo_depth(&h);
    drag_field(&mut h, "x", 30.0);
    let kids = h.children();
    let d = kids[0].x;
    assert!(d > 10.0, "{d}");
    assert_eq!(kids[1].x, 200.0 + d);
    assert_eq!(undo_depth(&h), depth + 1);
    h.key(Key::Z, Modifiers::COMMAND);
    let xs: Vec<f32> = h.children().iter().map(|n| n.x).collect();
    assert_eq!(xs, [0.0, 200.0]);
}

#[test]
fn fill_colour_and_alpha() {
    let mut h = one_rect();
    let depth = undo_depth(&h);
    type_into(&mut h, "fill.0.hex", "#FF0000");
    assert_eq!(solid(&shape_of(&h.children()[0]).fills[0]), "#ff0000");
    type_into(&mut h, "fill.0.alpha", "50");
    assert_eq!(solid(&shape_of(&h.children()[0]).fills[0]), "#ff000080");
    assert_eq!(text(&h, "fill.0.alpha"), "50%");
    assert_eq!(undo_depth(&h), depth + 2);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(solid(&shape_of(&h.children()[0]).fills[0]), "#ff0000");
}

#[test]
fn fills_add_remove_and_mixed() {
    let mut h = two_rects();
    type_into(&mut h, "fill.0.hex", "00ff00");
    for n in h.children() {
        assert_eq!(solid(&shape_of(&n).fills[0]), "#00ff00");
    }
    h.click(field(&h, "fill.add"));
    assert!(h.children().iter().all(|n| shape_of(n).fills.len() == 2));
    h.click(field(&h, "fill.1.remove"));
    h.click(field(&h, "fill.0.remove"));
    assert!(h.children().iter().all(|n| shape_of(n).fills.is_empty()));

    // Give only the first rect a fill: the lists differ, so it is mixed.
    h.click(h.at(50.0, 50.0));
    h.click(field(&h, "fill.add"));
    h.key(Key::A, Modifiers::COMMAND);
    h.step(vec![]);
    assert!(h.app.inspector().field_rect("fill.0.hex").is_none());
    h.click(field(&h, "fill.add"));
    assert!(h.children().iter().all(|n| shape_of(n).fills.len() == 1));
}

#[test]
fn fill_type_switches_to_a_gradient_and_back() {
    let mut h = one_rect();
    h.click(field(&h, "fill.0.type"));
    // The popup lists Solid, Linear, Radial under the combo box.
    let combo = h.app.inspector().field_rect("fill.0.type").unwrap();
    let row = combo.height();
    h.click(Pos2::new(combo.center().x, combo.max.y + row * 1.5 + 4.0));
    let kids = h.children();
    let fills = &shape_of(&kids[0]).fills;
    assert!(
        matches!(fills[0], Paint::LinearGradient(_)),
        "{:?}",
        fills[0]
    );
    // Stops are editable.
    type_into(&mut h, "fill.0.stop.1.hex", "0000ff");
    let kids = h.children();
    let Paint::LinearGradient(g) = &shape_of(&kids[0]).fills[0] else {
        unreachable!()
    };
    assert_eq!(g.stops[1].color, ColorRef::literal("#0000ff"));
    type_into(&mut h, "fill.0.stop.0.offset", "20");
    let kids = h.children();
    let Paint::LinearGradient(g) = &shape_of(&kids[0]).fills[0] else {
        unreachable!()
    };
    assert_eq!(g.stops[0].offset, 0.2);
}

#[test]
fn strokes_add_width_and_align() {
    let mut h = two_rects();
    h.click(field(&h, "stroke.add"));
    for n in h.children() {
        assert_eq!(solid(&shape_of(&n).strokes[0]), "#000000");
    }
    type_into(&mut h, "stroke.width", "3");
    h.click(field(&h, "stroke.align.center"));
    for n in h.children() {
        let s = shape_of(&n);
        assert_eq!(s.stroke_width, 3.0);
        assert_eq!(s.stroke_align, StrokeAlign::Center);
    }
    type_into(&mut h, "stroke.0.hex", "123456");
    assert_eq!(solid(&shape_of(&h.children()[1]).strokes[0]), "#123456");
    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(h.children().iter().all(|n| shape_of(n).stroke_width == 1.0));
}

#[test]
fn shadows_add_edit_and_remove() {
    let mut h = one_rect();
    h.click(field(&h, "shadow.add"));
    let shadows =
        || -> fn(&Harness) -> Vec<Shadow> { |h| shape_of(&h.children()[0]).shadows.clone() };
    assert_eq!(shadows()(&h).len(), 1);
    type_into(&mut h, "shadow.0.x", "5");
    type_into(&mut h, "shadow.0.blur", "-3");
    type_into(&mut h, "shadow.0.hex", "ff000080");
    let s = &shadows()(&h)[0];
    assert_eq!((s.x, s.y, s.blur), (5.0, 4.0, 0.0));
    assert_eq!(s.color, ColorRef::literal("#ff000080"));
    let depth = undo_depth(&h);
    drag_field(&mut h, "shadow.0.y", 20.0);
    assert!(shadows()(&h)[0].y > 4.0);
    assert_eq!(undo_depth(&h), depth + 1);
    h.click(field(&h, "shadow.0.remove"));
    assert!(shadows()(&h).is_empty());
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(shadows()(&h).len(), 1);
}

#[test]
fn shadows_on_a_multi_selection() {
    let mut h = two_rects();
    h.click(field(&h, "shadow.add"));
    type_into(&mut h, "shadow.0.spread", "2");
    for n in h.children() {
        assert_eq!(shape_of(&n).shadows[0].spread, 2.0);
    }
}

#[test]
fn corner_radius_uniform_and_per_corner() {
    let mut h = one_rect();
    type_into(&mut h, "radius", "8");
    assert_eq!(
        shape_of(&h.children()[0]).corner_radius,
        doc::CornerRadius::px(8.0)
    );
    h.click(field(&h, "radius.each"));
    type_into(&mut h, "radius.2", "20");
    assert_eq!(
        shape_of(&h.children()[0]).corner_radius,
        doc::CornerRadius::Each([8.0, 8.0, 20.0, 8.0].map(doc::RadiusRef::Px))
    );
    h.click(field(&h, "radius.each"));
    assert_eq!(
        shape_of(&h.children()[0]).corner_radius,
        doc::CornerRadius::px(8.0)
    );
}

#[test]
fn frame_clip_toggle() {
    let mut h = Harness::new();
    h.key(Key::F, Modifiers::NONE);
    h.drag(h.at(0.0, 0.0), h.at(200.0, 200.0));
    h.step(vec![]);
    let clip = |h: &Harness| match &h.children()[0].kind {
        NodeKind::Frame(f) => f.clip,
        _ => unreachable!(),
    };
    assert!(clip(&h));
    h.click(field(&h, "clip"));
    assert!(!clip(&h));
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(clip(&h));
}

#[test]
fn text_properties() {
    let mut h = Harness::new();
    h.key(Key::T, Modifiers::NONE);
    h.click(h.at(100.0, 100.0));
    h.type_text("Hello");
    h.key(Key::Escape, Modifiers::NONE);
    h.step(vec![]);
    let style = |h: &Harness| match &h.children()[0].kind {
        NodeKind::Text(t) => (t.style.clone(), t.align),
        _ => unreachable!(),
    };
    h.key(Key::V, Modifiers::NONE);
    h.click(h.at(105.0, 105.0));
    assert_eq!(h.ed().selection.len(), 1);
    let width = h.children()[0].width;
    assert_eq!(text(&h, "font.size"), "16");
    type_into(&mut h, "font.size", "32");
    assert_eq!(style(&h).0.font_size, 32.0);
    // Auto-width text is measured again.
    if width > 0.0 {
        assert!(h.children()[0].width > width, "{width}");
    }
    type_into(&mut h, "font.weight", "700");
    type_into(&mut h, "letter_spacing", "1.5");
    type_into(&mut h, "font.family", "Roboto");
    h.click(field(&h, "text.align.center"));
    let (s, align) = style(&h);
    assert_eq!(s.font_weight, 700);
    assert_eq!(s.letter_spacing, 1.5);
    assert_eq!(s.font_family, "Roboto");
    assert_eq!(align, doc::TextAlign::Center);
    // Text colour is its fill.
    type_into(&mut h, "fill.0.hex", "336699");
    let NodeKind::Text(t) = &h.children()[0].kind else {
        unreachable!()
    };
    assert_eq!(solid(&t.fills[0]), "#336699");
}

#[test]
fn nothing_selected_shows_no_fields() {
    let mut h = one_rect();
    h.key(Key::Escape, Modifiers::NONE);
    h.step(vec![]);
    assert!(h.ed().selection.is_empty());
    assert!(h.app.inspector().field_rect("x").is_none());
}
