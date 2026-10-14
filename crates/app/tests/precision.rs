//! Canvas precision (§9): snapping while moving and resizing, Ctrl to turn
//! it off, and rotating from outside a corner handle. Headless: these check
//! the document, not the painted guides.

mod common;

use common::Harness;
use egui::{Event, Key, Modifiers, Pos2};

/// Drag between page points with `mods` held throughout.
fn drag_with(h: &mut Harness, from: (f64, f64), to: (f64, f64), mods: Modifiers) {
    let (from, to): (Pos2, Pos2) = (h.at(from.0, from.1), h.at(to.0, to.1));
    h.step(vec![Event::ModifiersChanged(mods)]);
    h.move_to(from);
    h.button(true, mods);
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        h.move_to(from + (to - from) * t);
    }
    h.button(false, mods);
    h.step(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.step(vec![]);
}

fn rect(h: &mut Harness, x0: f64, y0: f64, x1: f64, y1: f64) {
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(x0, y0), h.at(x1, y1));
}

fn geometry(h: &Harness, i: usize) -> (f32, f32, f32, f32) {
    let n = &h.children()[i];
    (n.x, n.y, n.width, n.height)
}

#[test]
fn move_snaps_to_a_sibling_edge() {
    let mut h = Harness::new();
    rect(&mut h, 0.0, 0.0, 100.0, 100.0);
    rect(&mut h, 300.0, 0.0, 400.0, 100.0);
    // Right edge to 297 (3 px short of the sibling) and 2 px down.
    h.drag(h.at(50.0, 50.0), h.at(247.0, 52.0));
    assert_eq!(geometry(&h, 0), (200.0, 0.0, 100.0, 100.0));
    assert_eq!(h.ed().history.undo_label(), Some("Move layer"));
}

#[test]
fn ctrl_turns_snapping_off() {
    let mut h = Harness::new();
    rect(&mut h, 0.0, 0.0, 100.0, 100.0);
    rect(&mut h, 300.0, 0.0, 400.0, 100.0);
    drag_with(&mut h, (50.0, 50.0), (247.0, 52.0), Modifiers::COMMAND);
    assert_eq!(geometry(&h, 0), (197.0, 2.0, 100.0, 100.0));
}

#[test]
fn snapping_threshold_follows_zoom() {
    let mut h = Harness::new();
    rect(&mut h, 0.0, 0.0, 100.0, 100.0);
    rect(&mut h, 300.0, 0.0, 400.0, 100.0);
    h.move_to(h.at(50.0, 50.0));
    h.step(vec![Event::Zoom(4.0)]);
    assert_eq!(h.app.canvas().view.zoom, 4.0);
    // 3 page units is 12 screen px at 400%: too far to snap.
    h.click(h.at(50.0, 50.0));
    h.drag(h.at(50.0, 50.0), h.at(247.0, 50.0));
    assert_eq!(geometry(&h, 0), (197.0, 0.0, 100.0, 100.0));
}

#[test]
fn resize_snaps_the_dragged_edges() {
    let mut h = Harness::new();
    rect(&mut h, 0.0, 0.0, 100.0, 100.0);
    rect(&mut h, 300.0, 0.0, 400.0, 100.0);
    h.click(h.at(50.0, 50.0));
    // Bottom-right handle to (296, 103): right edge snaps to the sibling's
    // left edge, bottom to its bottom.
    h.drag(h.at(100.0, 100.0), h.at(296.0, 103.0));
    assert_eq!(geometry(&h, 0), (0.0, 0.0, 300.0, 100.0));
    assert_eq!(h.ed().history.undo_label(), Some("Resize layer"));
}

/// A page point at `deg` degrees (clockwise from +x) and `r` from `c`.
fn polar(c: (f64, f64), r: f64, deg: f64) -> (f64, f64) {
    let a = deg.to_radians();
    (c.0 + r * a.cos(), c.1 + r * a.sin())
}

#[test]
fn rotate_outside_a_corner_with_shift_and_one_undo_step() {
    let mut h = Harness::new();
    rect(&mut h, 100.0, 100.0, 200.0, 200.0);
    let id = h.children()[0].id.clone();
    assert_eq!(h.ed().selection, vec![id.clone()]);

    // Just outside the bottom-right corner (45° from the center), swept to
    // 95°: 50° of turn, which Shift makes 45°.
    let (ex, ey) = polar((150.0, 150.0), 85.0, 95.0);
    drag_with(&mut h, (208.0, 208.0), (ex, ey), Modifiers::SHIFT);
    let n = &h.children()[0];
    assert_eq!(n.rotation, 45.0);
    assert_eq!(n.rotation % 15.0, 0.0);
    // It turns about its own center.
    assert_eq!((n.x, n.y, n.width, n.height), (100.0, 100.0, 100.0, 100.0));
    assert_eq!(h.ed().history.undo_label(), Some("Rotate layer"));

    // Without Shift the angle is free: another 50°.
    // Bounds of the diamond reach 150 ± 70.7; start just past a corner.
    let (sx, sy) = polar((150.0, 150.0), 110.0, 45.0);
    let (ex, ey) = polar((150.0, 150.0), 110.0, 95.0);
    h.drag(h.at(sx, sy), h.at(ex, ey));
    let r = h.children()[0].rotation;
    assert!((r - 95.0).abs() < 0.05, "{r}");

    // Each rotation is one undo step.
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children()[0].rotation, 45.0);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children()[0].rotation, 0.0);
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(
        h.children().is_empty(),
        "the next step back is the creation"
    );
}

#[test]
fn rotated_nodes_still_hit_test_move_and_resize() {
    let mut h = Harness::new();
    rect(&mut h, 100.0, 100.0, 200.0, 200.0);
    let (ex, ey) = polar((150.0, 150.0), 85.0, 90.0);
    drag_with(&mut h, (208.0, 208.0), (ex, ey), Modifiers::SHIFT);
    assert_eq!(h.children()[0].rotation, 45.0);
    let id = h.children()[0].id.clone();

    // The corner of the old box is outside the rotated diamond.
    h.click(h.at(103.0, 103.0));
    assert!(h.ed().selection.is_empty());
    h.click(h.at(150.0, 150.0));
    assert_eq!(h.ed().selection, vec![id]);

    h.drag(h.at(150.0, 150.0), h.at(170.0, 160.0));
    let n = &h.children()[0];
    assert_eq!((n.x, n.y, n.rotation), (120.0, 110.0, 45.0));

    // Resizing from a bounds handle keeps the rotation and grows the box.
    let b = h.ed().geometry.get(&n.id).unwrap().bounds;
    h.drag(h.at(b.x1, b.y1), h.at(b.x1 + 20.0, b.y1 + 20.0));
    let n = &h.children()[0];
    assert_eq!(n.rotation, 45.0);
    assert!(n.width > 100.0 && n.height > 100.0, "{n:?}");
}

#[test]
fn move_snaps_to_the_parent_frame_centre() {
    let mut h = Harness::new();
    h.key(Key::F, Modifiers::NONE);
    h.drag(h.at(0.0, 0.0), h.at(400.0, 300.0));
    rect(&mut h, 50.0, 60.0, 150.0, 160.0);
    // Centre to (198, 147), 2 and 3 px from the frame's centre.
    h.drag(h.at(100.0, 110.0), h.at(198.0, 147.0));
    let kids = h.children();
    let child = &kids[0].children().unwrap()[0];
    assert_eq!((child.x, child.y), (150.0, 100.0));
}
