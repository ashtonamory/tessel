//! Drives the whole app headlessly: synthetic pointer and keyboard events go
//! through egui into `DesignApp`, and the tests check the document. No window
//! or GPU is needed (the canvas shows a placeholder instead of the vello
//! texture), so this covers tools, selection, gestures and shortcuts, not
//! pixels.

mod common;

use common::Harness;
use doc::NodeKind;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2};

#[test]
fn draw_rect_and_ellipse_by_dragging_then_undo_redo() {
    let mut h = Harness::new();
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(100.0, 100.0), h.at(300.0, 250.0));
    let kids = h.children();
    assert_eq!(kids.len(), 1);
    let r = &kids[0];
    assert!(matches!(r.kind, NodeKind::Rect(_)));
    assert_eq!((r.x, r.y, r.width, r.height), (100.0, 100.0, 200.0, 150.0));
    assert_eq!(r.name, "Rectangle 1");
    assert_eq!(h.ed().selection, vec![r.id.clone()]);

    h.key(Key::O, Modifiers::NONE);
    h.drag(h.at(400.0, 100.0), h.at(450.0, 180.0));
    assert!(matches!(h.children()[1].kind, NodeKind::Ellipse(_)));

    // One undo step per drawn shape.
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children().len(), 1);
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(h.children().is_empty());
    h.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.children().len(), 1);
    assert_eq!(h.children()[0].width, 200.0);
}

#[test]
fn click_select_move_and_resize() {
    let mut h = Harness::new();
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(100.0, 100.0), h.at(200.0, 200.0));
    let id = h.children()[0].id.clone();

    // Clicking empty canvas clears the selection; clicking the rect selects it.
    h.click(h.at(500.0, 500.0));
    assert!(h.ed().selection.is_empty());
    h.click(h.at(150.0, 150.0));
    assert_eq!(h.ed().selection, vec![id.clone()]);

    // Drag the body to move.
    h.drag(h.at(150.0, 150.0), h.at(180.0, 170.0));
    let r = &h.children()[0];
    assert_eq!((r.x, r.y), (130.0, 120.0));

    // Drag the bottom-right handle to resize.
    h.drag(h.at(230.0, 220.0), h.at(260.0, 260.0));
    let r = &h.children()[0];
    assert_eq!((r.x, r.y, r.width, r.height), (130.0, 120.0, 130.0, 140.0));

    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    let r = &h.children()[0];
    assert_eq!((r.x, r.y, r.width, r.height), (100.0, 100.0, 100.0, 100.0));
}

#[test]
fn marquee_selects_and_delete_removes() {
    let mut h = Harness::new();
    for x in [0.0, 150.0, 300.0] {
        h.key(Key::R, Modifiers::NONE);
        h.drag(h.at(x, 0.0), h.at(x + 100.0, 100.0));
    }
    h.click(h.at(600.0, 600.0));
    h.drag(h.at(-20.0, -20.0), h.at(200.0, 50.0));
    assert_eq!(h.ed().selection.len(), 2);
    h.key(Key::Delete, Modifiers::NONE);
    assert_eq!(h.children().len(), 1);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children().len(), 3);
    assert_eq!(h.ed().selection.len(), 2, "undo restores the selection");
}

#[test]
fn shapes_drawn_inside_a_frame_become_its_children() {
    let mut h = Harness::new();
    h.key(Key::F, Modifiers::NONE);
    h.drag(h.at(0.0, 0.0), h.at(400.0, 300.0));
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(50.0, 60.0), h.at(150.0, 160.0));
    let kids = h.children();
    assert_eq!(kids.len(), 1);
    let frame_children = kids[0].children().unwrap();
    assert_eq!(frame_children.len(), 1);
    assert_eq!((frame_children[0].x, frame_children[0].y), (50.0, 60.0));
}

#[test]
fn text_tool_types_inline_and_commits_one_step() {
    let mut h = Harness::new();
    h.key(Key::T, Modifiers::NONE);
    h.click(h.at(100.0, 100.0));
    assert!(h.app.canvas().is_editing_text());
    h.type_text("Hello");
    h.key(Key::Escape, Modifiers::NONE);
    h.step(vec![]);
    assert!(!h.app.canvas().is_editing_text());
    let kids = h.children();
    assert_eq!(kids.len(), 1, "{kids:?}");
    let NodeKind::Text(t) = &kids[0].kind else {
        panic!("not text: {:?}", kids[0].kind)
    };
    assert_eq!(t.content, "Hello");
    assert_eq!(kids[0].name, "Hello");
    assert_eq!(h.ed().history.undo_label(), Some("Create text"));

    // Double-click edits it again.
    h.double_click(h.at(105.0, 105.0));
    assert!(h.app.canvas().is_editing_text());
    h.type_text(" world");
    h.key(Key::Escape, Modifiers::NONE);
    h.step(vec![]);
    let NodeKind::Text(t) = &h.children()[0].kind else {
        unreachable!()
    };
    assert_eq!(t.content, "Hello world");
    h.key(Key::Z, Modifiers::COMMAND);
    let NodeKind::Text(t) = &h.children()[0].kind else {
        unreachable!()
    };
    assert_eq!(t.content, "Hello");
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(h.children().is_empty());
}

#[test]
fn empty_text_is_discarded() {
    let mut h = Harness::new();
    h.key(Key::T, Modifiers::NONE);
    h.click(h.at(100.0, 100.0));
    h.key(Key::Escape, Modifiers::NONE);
    h.step(vec![]);
    assert!(h.children().is_empty());
    assert!(!h.ed().history.can_undo());
}

#[test]
fn pinch_zooms_around_the_pointer() {
    let mut h = Harness::new();
    let p = h.at(200.0, 100.0);
    h.move_to(p);
    let before = h.app.canvas().view;
    h.step(vec![Event::Zoom(2.0)]);
    let view = h.app.canvas().view;
    assert!((view.zoom - before.zoom * 2.0).abs() < 1e-4, "{view:?}");
    let page = view.to_page(h.app.canvas().rect.min, p);
    assert!((page.x - 200.0).abs() < 1e-3 && (page.y - 100.0).abs() < 1e-3);
}

fn row(h: &Harness, name: &str) -> Rect {
    let id = h
        .children()
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.id.clone())
        .unwrap_or_else(|| panic!("no layer {name}"));
    h.app
        .layers()
        .rows()
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("row not drawn; rows: {:?}", h.app.layers().rows()))
}

fn names(h: &Harness) -> Vec<String> {
    h.children().iter().map(|n| n.name.clone()).collect()
}

fn three_rects() -> Harness {
    let mut h = Harness::new();
    for x in [0.0, 150.0, 300.0] {
        h.key(Key::R, Modifiers::NONE);
        h.drag(h.at(x, 0.0), h.at(x + 100.0, 100.0));
    }
    h.step(vec![]);
    h
}

#[test]
fn layers_panel_lists_front_most_first_and_selects() {
    let mut h = three_rects();
    let order: Vec<f32> = ["Rectangle 3", "Rectangle 2", "Rectangle 1"]
        .iter()
        .map(|n| row(&h, n).min.y)
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    h.click(row(&h, "Rectangle 2").center());
    let id = h.children()[1].id.clone();
    assert_eq!(h.ed().selection, vec![id]);
}

#[test]
fn layers_panel_renames_on_double_click() {
    let mut h = three_rects();
    h.double_click(row(&h, "Rectangle 1").center());
    h.key(Key::A, Modifiers::COMMAND);
    h.type_text("Hero");
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
    assert_eq!(names(&h), ["Hero", "Rectangle 2", "Rectangle 3"]);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(names(&h)[0], "Rectangle 1");
}

#[test]
fn layers_panel_toggles_visibility() {
    let mut h = three_rects();
    let r = row(&h, "Rectangle 2");
    h.click(Pos2::new(r.max.x - 14.0, r.center().y));
    assert!(!h.children()[1].visible);
    h.click(Pos2::new(r.max.x - 14.0, r.center().y));
    assert!(h.children()[1].visible);
}

#[test]
fn layers_panel_reorders_by_drag() {
    let mut h = three_rects();
    // Drag the back-most layer above the front-most one.
    let from = row(&h, "Rectangle 1").center();
    let to = row(&h, "Rectangle 3");
    h.drag(from, Pos2::new(to.center().x, to.min.y + 3.0));
    assert_eq!(names(&h), ["Rectangle 2", "Rectangle 3", "Rectangle 1"]);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(names(&h), ["Rectangle 1", "Rectangle 2", "Rectangle 3"]);
}

fn wheel(delta: Vec2, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        phase: egui::TouchPhase::Move,
        modifiers,
    }
}

#[test]
fn scroll_pans_and_ctrl_scroll_zooms() {
    let mut h = Harness::new();
    let p = h.at(100.0, 100.0);
    h.move_to(p);
    let before = h.app.canvas().view;
    h.step(vec![wheel(Vec2::new(0.0, -50.0), Modifiers::NONE)]);
    for _ in 0..20 {
        h.step(vec![]);
    }
    let after = h.app.canvas().view;
    assert_eq!(after.zoom, before.zoom);
    assert!(after.offset.y < before.offset.y, "{before:?} -> {after:?}");

    h.step(vec![wheel(Vec2::new(0.0, 50.0), Modifiers::COMMAND)]);
    let zoomed = h.app.canvas().view;
    assert!(zoomed.zoom > after.zoom, "{after:?} -> {zoomed:?}");
}

#[test]
fn middle_drag_pans() {
    let mut h = Harness::new();
    let before = h.app.canvas().view;
    let from = h.at(100.0, 100.0);
    h.move_to(from);
    let press = |pressed| Event::PointerButton {
        pos: from,
        button: PointerButton::Middle,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.step(vec![press(true)]);
    for i in 1..=5 {
        h.move_to(from + Vec2::new(10.0 * i as f32, 0.0));
    }
    h.step(vec![press(false)]);
    let after = h.app.canvas().view;
    assert!(
        after.offset.x > before.offset.x + 30.0,
        "{before:?} -> {after:?}"
    );
    assert!(h.children().is_empty());
}

#[test]
fn space_drag_pans_instead_of_selecting() {
    let mut h = Harness::new();
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(0.0, 0.0), h.at(100.0, 100.0));
    let before = h.app.canvas().view;
    let space = |pressed| Event::Key {
        key: Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    h.step(vec![space(true)]);
    h.drag(h.at(50.0, 50.0), h.at(50.0, 90.0));
    h.step(vec![space(false)]);
    let after = h.app.canvas().view;
    assert!(
        after.offset.y > before.offset.y + 30.0,
        "{before:?} -> {after:?}"
    );
    let r = &h.children()[0];
    assert_eq!((r.x, r.y), (0.0, 0.0), "the rect did not move");
}

#[test]
fn undo_during_a_drag_ends_it_cleanly() {
    let mut h = Harness::new();
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(0.0, 0.0), h.at(100.0, 100.0));
    // Start moving the rect, press Ctrl+Z mid-drag, keep dragging.
    h.move_to(h.at(50.0, 50.0));
    h.button(true, Modifiers::NONE);
    for i in 1..=4 {
        h.move_to(h.at(50.0 + 10.0 * i as f64, 50.0));
    }
    h.key(Key::Z, Modifiers::COMMAND);
    h.move_to(h.at(120.0, 50.0));
    h.button(false, Modifiers::NONE);
    h.step(vec![]);
    // The move was recorded and undone; the rect is back where it was drawn.
    let r = &h.children()[0];
    assert_eq!((r.x, r.y), (0.0, 0.0));
    assert!(h.ed().status.is_none(), "{:?}", h.ed().status);
    h.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.children()[0].x, 40.0);
}
