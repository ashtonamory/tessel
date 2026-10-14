//! M2 structure: pages, group/ungroup/frame, z-order, copy/paste/duplicate,
//! lock and layer multi-select. Editor-level tests check that every command
//! is one undo step that restores the exact document; UI tests drive the
//! same through the headless app.

mod common;

use app::pages::PageButton;
use app::Editor;
use common::Harness;
use doc::{Command, Id, IdKind, Node, NodeId, NodeKind, Rect as RectKind};
use egui::{Event, Key, Modifiers};

fn rect(name: &str, x: f32, y: f32, w: f32, h: f32) -> Node {
    Node::new(
        Id::new(IdKind::Node),
        name,
        NodeKind::Rect(RectKind::default()),
    )
    .with_geometry(x, y, w, h)
}

fn add(ed: &mut Editor, n: Node) -> NodeId {
    let id = n.id.clone();
    let parent = ed.page_ref();
    ed.commit(
        "Add",
        vec![Command::CreateNode {
            parent,
            index: None,
            node: n,
        }],
        vec![id.clone()],
    )
    .unwrap();
    id
}

fn names(ed: &Editor) -> Vec<String> {
    ed.page().children.iter().map(|n| n.name.clone()).collect()
}

fn three() -> (Editor, [NodeId; 3]) {
    let mut ed = Editor::new();
    let a = add(&mut ed, rect("a", 10.0, 10.0, 20.0, 20.0));
    let b = add(&mut ed, rect("b", 50.0, 40.0, 30.0, 30.0));
    let c = add(&mut ed, rect("c", 100.0, 100.0, 10.0, 10.0));
    (ed, [a, b, c])
}

/// Run `f`, expect exactly one new undo step, and check undo restores the
/// document and redo reapplies the change.
fn one_undo_step(ed: &mut Editor, label: &str, f: impl FnOnce(&mut Editor)) {
    let before = ed.doc.clone();
    let sel_before = ed.selection.clone();
    let depth = ed.history.transactions().len();
    f(ed);
    assert_eq!(ed.history.transactions().len(), depth + 1, "{label}");
    assert_eq!(ed.history.undo_label(), Some(label));
    let after = ed.doc.clone();
    assert_ne!(after, before);
    ed.undo();
    assert_eq!(ed.doc, before, "undo of {label}");
    assert_eq!(ed.selection, sel_before);
    ed.redo();
    assert_eq!(ed.doc, after, "redo of {label}");
}

// --- Editor level ---------------------------------------------------------

#[test]
fn pages_add_rename_reorder_delete() {
    let (mut ed, [a, ..]) = three();
    let first = ed.page().id.clone();
    one_undo_step(&mut ed, "Add page", |ed| {
        ed.add_page();
    });
    assert_eq!(ed.doc.pages.len(), 2);
    assert_eq!(ed.doc.pages[1].name, "Page 2");
    // The new page is shown and empty; the old page keeps its layers.
    assert_eq!(ed.page().id, ed.doc.pages[1].id);
    assert!(ed.page().children.is_empty());
    assert!(ed.node(&a).is_some());
    let second = ed.page().id.clone();

    one_undo_step(&mut ed, "Rename page", |ed| {
        ed.rename_page(&second, "Settings".into())
    });
    one_undo_step(&mut ed, "Reorder page", |ed| ed.move_page(&second, 0));
    assert_eq!(ed.doc.pages[0].id, second);
    assert_eq!(ed.doc.pages[1].id, first);
    assert_eq!(ed.doc.pages[1].children.len(), 3);

    ed.switch_page(&first);
    assert!(ed.selection.is_empty());
    assert_eq!(ed.page().children.len(), 3);
    one_undo_step(&mut ed, "Delete page", |ed| ed.delete_page(&second));
    assert_eq!(ed.doc.pages.len(), 1);
    // The last page cannot be deleted.
    let depth = ed.history.transactions().len();
    ed.delete_page(&first);
    assert_eq!(ed.doc.pages.len(), 1);
    assert_eq!(ed.history.transactions().len(), depth);
}

#[test]
fn deleting_the_current_page_shows_a_neighbour() {
    let mut ed = Editor::new();
    let first = ed.page().id.clone();
    let second = ed.add_page().unwrap();
    ed.delete_page(&second);
    assert_eq!(ed.page().id, first);
    ed.undo();
    assert_eq!(ed.doc.pages.len(), 2);
    // Switching is not an edit: it does not dirty a saved document.
    let rev = ed.revision;
    ed.switch_page(&second);
    assert!(ed.revision > rev);
}

#[test]
fn group_keeps_absolute_positions_and_ungroup_restores() {
    let (mut ed, [a, b, c]) = three();
    let before: Vec<_> = [&a, &b, &c]
        .iter()
        .map(|id| ed.geometry.get(id).unwrap().bounds)
        .collect();
    ed.selection = vec![a.clone(), b.clone()];
    one_undo_step(&mut ed, "Group layers", |ed| ed.group_selection());
    let g = ed.selection[0].clone();
    let gn = ed.node(&g).unwrap();
    assert!(matches!(gn.kind, NodeKind::Group(_)));
    assert_eq!((gn.x, gn.y, gn.width, gn.height), (10.0, 10.0, 70.0, 60.0));
    assert_eq!(gn.name, "Group 1");
    // a and b are inside, at the same place on the page; c is still on top.
    assert_eq!(names(&ed), ["Group 1", "c"]);
    for (id, want) in [&a, &b, &c].iter().zip(&before) {
        assert_eq!(ed.geometry.get(id).unwrap().bounds, *want);
    }
    assert_eq!(ed.node(&a).unwrap().x, 0.0);

    one_undo_step(&mut ed, "Ungroup", |ed| ed.ungroup_selection());
    assert_eq!(names(&ed), ["a", "b", "c"]);
    assert_eq!(ed.selection, vec![a.clone(), b.clone()]);
    for (id, want) in [&a, &b, &c].iter().zip(&before) {
        assert_eq!(ed.geometry.get(id).unwrap().bounds, *want);
    }
    assert_eq!(ed.node(&a).unwrap().x, 10.0);
}

#[test]
fn frame_selection_wraps_in_a_frame() {
    let (mut ed, [a, b, _]) = three();
    ed.selection = vec![a.clone(), b.clone()];
    let bounds = ed.geometry.get(&b).unwrap().bounds;
    one_undo_step(&mut ed, "Frame selection", |ed| ed.frame_selection());
    let f = ed.selection[0].clone();
    assert!(matches!(ed.node(&f).unwrap().kind, NodeKind::Frame(_)));
    assert_eq!(ed.node(&f).unwrap().name, "Frame 1");
    assert_eq!(ed.geometry.get(&b).unwrap().bounds, bounds);
}

#[test]
fn grouping_across_parents_is_refused() {
    let (mut ed, [a, b, c]) = three();
    ed.selection = vec![a.clone(), b.clone()];
    ed.group_selection();
    let g = ed.selection[0].clone();
    ed.selection = vec![a.clone(), c.clone()];
    let before = ed.doc.clone();
    ed.group_selection();
    assert_eq!(ed.doc, before);
    assert!(ed.status.as_deref().unwrap().contains("same parent"));
    // Ungrouping something that is not a group is a notice, not an edit.
    ed.selection = vec![c];
    ed.ungroup_selection();
    assert_eq!(ed.doc, before);
    assert!(ed.node(&g).is_some());
}

#[test]
fn z_order_commands() {
    let (mut ed, [a, b, c]) = three();
    ed.selection = vec![a.clone()];
    one_undo_step(&mut ed, "Bring forward", |ed| {
        ed.reorder(app::structure::ZOrder::Forward)
    });
    assert_eq!(names(&ed), ["b", "a", "c"]);
    one_undo_step(&mut ed, "Bring to front", |ed| {
        ed.reorder(app::structure::ZOrder::Front)
    });
    assert_eq!(names(&ed), ["b", "c", "a"]);
    one_undo_step(&mut ed, "Send backward", |ed| {
        ed.reorder(app::structure::ZOrder::Backward)
    });
    assert_eq!(names(&ed), ["b", "a", "c"]);
    one_undo_step(&mut ed, "Send to back", |ed| {
        ed.reorder(app::structure::ZOrder::Back)
    });
    assert_eq!(
        names(&ed),
        ["a", "b", "c"].map(String::from).map(|s| s).to_vec()
    );

    // Several layers keep their relative order; touching the top is a no-op.
    ed.selection = vec![a.clone(), b.clone()];
    ed.reorder(app::structure::ZOrder::Forward);
    assert_eq!(names(&ed), ["c", "a", "b"]);
    let depth = ed.history.transactions().len();
    ed.reorder(app::structure::ZOrder::Front);
    assert_eq!(ed.history.transactions().len(), depth);
    ed.selection = vec![c, b];
    ed.reorder(app::structure::ZOrder::Back);
    assert_eq!(names(&ed), ["c", "b", "a"]);
}

#[test]
fn duplicate_makes_fresh_ids_above_the_original() {
    let (mut ed, [a, b, _]) = three();
    ed.selection = vec![a.clone(), b.clone()];
    one_undo_step(&mut ed, "Duplicate 2 layers", |ed| ed.duplicate_selection());
    assert_eq!(names(&ed), ["a", "a", "b", "b", "c"]);
    assert_eq!(ed.selection.len(), 2);
    assert!(!ed.selection.contains(&a) && !ed.selection.contains(&b));
    let copy = ed.node(&ed.selection[0]).unwrap();
    assert_eq!((copy.x, copy.y), (20.0, 20.0));
    let mut ids = Vec::new();
    ed.doc.walk_nodes(&mut |n| ids.push(n.id.clone()));
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 5);
}

#[test]
fn copy_paste_cut_use_marked_json_and_fresh_ids() {
    let (mut ed, [a, b, _]) = three();
    ed.selection = vec![a.clone(), b.clone()];
    let text = ed.copy_text().unwrap();
    assert!(text.contains(app::clipboard::MARKER));
    assert_eq!(ed.doc.pages[0].children.len(), 3, "copy changes nothing");

    // Pasting over the source offsets the copies; again, offsets further.
    one_undo_step(
        &mut ed,
        "Paste 2 layers",
        |ed| assert!(ed.paste_text(&text)),
    );
    assert_eq!(ed.page().children.len(), 5);
    let p = ed.node(&ed.selection[0]).unwrap().clone();
    assert_ne!(p.id, a);
    assert_eq!((p.x, p.y), (20.0, 20.0));
    ed.paste_text(&text);
    assert_eq!(ed.node(&ed.selection[0]).unwrap().x, 30.0);

    // Other text is not ours.
    assert!(!ed.paste_text("hello"));
    assert!(!ed.paste_text("{\"clipboard\":\"other\",\"nodes\":[]}"));

    // Cut removes the selection and its text pastes back where it was.
    let mut ed = three().0;
    let a = ed.page().children[0].id.clone();
    ed.selection = vec![a.clone()];
    let text = ed.cut_text().unwrap();
    assert_eq!(names(&ed), ["b", "c"]);
    ed.paste_text(&text);
    let back = ed.node(&ed.selection[0]).unwrap();
    assert_eq!((back.x, back.y), (10.0, 10.0));
    assert_ne!(back.id, a);
}

#[test]
fn copy_inside_a_frame_pastes_at_page_position() {
    let mut ed = Editor::new();
    let f = add(&mut ed, rect("f", 100.0, 100.0, 200.0, 200.0));
    let c = add(&mut ed, rect("c", 120.0, 130.0, 10.0, 10.0));
    ed.selection = vec![f.clone(), c.clone()];
    ed.group_selection();
    // `c` is now inside a group at (20, 30) relative to its parent.
    ed.selection = vec![c.clone()];
    let text = ed.copy_text().unwrap();
    ed.selection.clear();
    ed.paste_text(&text);
    let p = ed.node(&ed.selection[0]).unwrap();
    // It lands on the page, beside its source (which is nested).
    assert_eq!((p.x, p.y), (130.0, 140.0));
}

#[test]
fn lock_is_a_command() {
    let (mut ed, [a, ..]) = three();
    one_undo_step(&mut ed, "Lock layer", |ed| ed.set_locked(&a, true));
    assert!(ed.node(&a).unwrap().locked);
    ed.select_all();
    assert!(!ed.selection.contains(&a));
}

// --- Headless UI ----------------------------------------------------------

fn draw(h: &mut Harness, from: (f64, f64), to: (f64, f64)) -> NodeId {
    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(from.0, from.1), h.at(to.0, to.1));
    h.ed().selection[0].clone()
}

fn row(h: &Harness, id: &NodeId) -> egui::Rect {
    h.app
        .layers()
        .rows()
        .iter()
        .find(|(r, _)| r == id)
        .unwrap()
        .1
}

#[test]
fn ui_pages_add_switch_rename_reorder_delete() {
    let mut h = Harness::new();
    let a = draw(&mut h, (100.0, 100.0), (200.0, 200.0));
    let first = h.ed().page().id.clone();

    let add = h.app.layers().pages.add_button().unwrap().center();
    h.click(add);
    assert_eq!(h.ed().doc.pages.len(), 2);
    let second = h.ed().page().id.clone();
    assert_ne!(first, second);
    assert!(h.children().is_empty());
    // The first page's layers are not on the canvas or in the panel.
    h.click(h.at(150.0, 150.0));
    assert!(h.ed().selection.is_empty());

    let rows = h.app.layers().pages.rows().to_vec();
    assert_eq!(rows.len(), 2);
    h.click(rows[0].1.center());
    assert_eq!(h.ed().page().id, first);
    h.click(h.at(150.0, 150.0));
    assert_eq!(h.ed().selection, vec![a]);

    // Rename by double-click.
    let r = h.app.layers().pages.rows()[1].1;
    h.click(r.center());
    h.double_click(r.left_center() + egui::vec2(30.0, 0.0));
    assert!(h.app.layers().is_renaming());
    h.type_text("X");
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.ed().doc.pages[1].name, "Page 2X");

    // Reorder with the up button on the current page.
    let r = h.app.layers().pages.rows()[1].1;
    h.click(PageButton::Up.rect(r).center());
    assert_eq!(h.ed().doc.pages[0].id, second);
    assert_eq!(h.ed().page().id, second, "the page stays current");

    // Delete the current page with its button; the other remains.
    let r = h.app.layers().pages.rows()[0].1;
    h.click(PageButton::Delete.rect(r).center());
    assert_eq!(h.ed().doc.pages.len(), 1);
    assert_eq!(h.ed().page().id, first);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.ed().doc.pages.len(), 2);
}

#[test]
fn ui_group_ungroup_and_frame_shortcuts() {
    let mut h = Harness::new();
    draw(&mut h, (100.0, 100.0), (200.0, 200.0));
    draw(&mut h, (300.0, 150.0), (400.0, 300.0));
    let ids: Vec<_> = h.children().iter().map(|n| n.id.clone()).collect();
    let before: Vec<_> = ids
        .iter()
        .map(|i| h.ed().geometry.get(i).unwrap().bounds)
        .collect();
    h.key(Key::A, Modifiers::COMMAND);
    h.key(Key::G, Modifiers::COMMAND);
    assert_eq!(h.children().len(), 1);
    assert!(matches!(h.children()[0].kind, NodeKind::Group(_)));
    for (i, b) in ids.iter().zip(&before) {
        assert_eq!(h.ed().geometry.get(i).unwrap().bounds, *b);
    }
    h.key(Key::G, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.children().len(), 2);
    for (i, b) in ids.iter().zip(&before) {
        assert_eq!(h.ed().geometry.get(i).unwrap().bounds, *b);
    }
    h.key(Key::A, Modifiers::COMMAND);
    h.key(Key::G, Modifiers::COMMAND | Modifiers::ALT);
    assert!(matches!(h.children()[0].kind, NodeKind::Frame(_)));
    assert_eq!(h.children().len(), 1);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children().len(), 2);
}

#[test]
fn ui_z_order_shortcuts() {
    let mut h = Harness::new();
    let a = draw(&mut h, (100.0, 100.0), (150.0, 150.0));
    draw(&mut h, (200.0, 100.0), (250.0, 150.0));
    draw(&mut h, (300.0, 100.0), (350.0, 150.0));
    h.click(h.at(120.0, 120.0));
    assert_eq!(h.ed().selection, vec![a.clone()]);
    let pos = |h: &Harness| h.children().iter().position(|n| n.id == a).unwrap();
    h.key(Key::CloseBracket, Modifiers::COMMAND);
    assert_eq!(pos(&h), 1);
    h.key(Key::CloseBracket, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(pos(&h), 2);
    h.key(Key::OpenBracket, Modifiers::COMMAND);
    assert_eq!(pos(&h), 1);
    h.key(Key::OpenBracket, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(pos(&h), 0);
}

#[test]
fn ui_copy_paste_duplicate() {
    let mut h = Harness::new();
    let a = draw(&mut h, (100.0, 100.0), (200.0, 200.0));
    h.step(vec![Event::Copy]);
    let text = h.app.last_copied().unwrap().to_string();
    assert!(text.contains(app::clipboard::MARKER));
    h.step(vec![Event::Paste(text.clone())]);
    assert_eq!(h.children().len(), 2);
    let p = h.ed().selection[0].clone();
    assert_ne!(p, a);
    assert_eq!(h.ed().node(&p).unwrap().x, 110.0);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.children().len(), 1);

    h.key(Key::D, Modifiers::COMMAND);
    assert_eq!(h.children().len(), 2);
    assert_ne!(h.ed().selection[0], a);

    // Cut removes, and the text still pastes.
    h.step(vec![Event::Cut]);
    assert_eq!(h.children().len(), 1);
    let cut = h.app.last_copied().unwrap().to_string();
    h.step(vec![Event::Paste(cut)]);
    assert_eq!(h.children().len(), 2);

    // Text that is not ours is not pasted as nodes.
    h.step(vec![Event::Paste("hello".into())]);
    assert_eq!(h.children().len(), 2);
}

#[test]
fn ui_locked_layers_cannot_be_picked_on_canvas() {
    let mut h = Harness::new();
    let a = draw(&mut h, (100.0, 100.0), (200.0, 200.0));
    h.click(h.at(500.0, 500.0));
    // Click the padlock in the layer's row.
    let r = row(&h, &a);
    h.move_to(r.center());
    h.click(egui::pos2(r.max.x - 34.0, r.center().y));
    assert!(h.ed().node(&a).unwrap().locked);
    assert!(
        h.ed().selection.is_empty(),
        "the lock click is not a select"
    );

    h.click(h.at(150.0, 150.0));
    assert!(h.ed().selection.is_empty());
    // Marquee skips it too, and so does Select all.
    h.drag(h.at(50.0, 50.0), h.at(300.0, 300.0));
    assert!(h.ed().selection.is_empty());
    h.key(Key::A, Modifiers::COMMAND);
    assert!(h.ed().selection.is_empty());

    // Unlock from the panel and it can be picked again.
    let r = row(&h, &a);
    h.move_to(r.center());
    h.click(egui::pos2(r.max.x - 34.0, r.center().y));
    assert!(!h.ed().node(&a).unwrap().locked);
    h.click(h.at(150.0, 150.0));
    assert_eq!(h.ed().selection, vec![a]);
}

#[test]
fn ui_layers_multi_select_and_collapse() {
    let mut h = Harness::new();
    let a = draw(&mut h, (100.0, 100.0), (150.0, 150.0));
    let b = draw(&mut h, (200.0, 100.0), (250.0, 150.0));
    let c = draw(&mut h, (300.0, 100.0), (350.0, 150.0));
    let d = draw(&mut h, (400.0, 100.0), (450.0, 150.0));

    // Front-most is on top: d, c, b, a.
    h.click(row(&h, &c).center());
    assert_eq!(h.ed().selection, vec![c.clone()]);
    h.click_with(row(&h, &a).center(), Modifiers::COMMAND);
    assert_eq!(h.ed().selection, vec![c.clone(), a.clone()]);
    h.click_with(row(&h, &c).center(), Modifiers::COMMAND);
    assert_eq!(h.ed().selection, vec![a.clone()]);

    h.click(row(&h, &d).center());
    h.click_with(row(&h, &b).center(), Modifiers::SHIFT);
    assert_eq!(h.ed().selection, vec![d.clone(), c.clone(), b.clone()]);

    // Group them, then collapse and expand the group's row.
    h.key(Key::G, Modifiers::COMMAND);
    let g = h.ed().selection[0].clone();
    let rows = h.app.layers().rows().len();
    assert_eq!(rows, 5);
    let r = row(&h, &g);
    let arrow = egui::pos2(r.min.x + 10.0, r.center().y);
    h.click(arrow);
    assert!(h.app.layers().is_collapsed(&g));
    assert_eq!(h.app.layers().rows().len(), 2);
    h.click(arrow);
    assert!(!h.app.layers().is_collapsed(&g));
    assert_eq!(h.app.layers().rows().len(), 5);
}

#[test]
fn ungroup_keeps_geometry_of_rotated_groups_and_children() {
    let mut ed = Editor::new();
    let mut a = rect("a", 10.0, 20.0, 40.0, 20.0);
    a.rotation = 30.0;
    let mut b = rect("b", 70.0, 5.0, 30.0, 50.0);
    b.rotation = -50.0;
    let a = add(&mut ed, a);
    let b = add(&mut ed, b);
    ed.selection = vec![a.clone(), b.clone()];
    ed.group_selection();
    let g = ed.selection[0].clone();
    ed.commit(
        "Rotate",
        vec![Command::SetProps {
            id: g.clone(),
            patch: doc::NodePatch {
                rotation: Some(37.0),
                ..Default::default()
            },
        }],
        vec![g],
    )
    .unwrap();
    let before: Vec<_> = [&a, &b]
        .iter()
        .map(|id| ed.geometry.get(id).unwrap().transform.as_coeffs())
        .collect();
    ed.ungroup_selection();
    assert_eq!(names(&ed), ["a", "b"]);
    for (id, want) in [&a, &b].iter().zip(&before) {
        let got = ed.geometry.get(id).unwrap().transform.as_coeffs();
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 0.01, "{got:?} vs {want:?}");
        }
    }
    assert!((ed.node(&a).unwrap().rotation - 67.0).abs() < 1e-3);
}

fn rotate(ed: &mut Editor, id: &NodeId, deg: f32) {
    ed.commit(
        "Rotate",
        vec![Command::SetProps {
            id: id.clone(),
            patch: doc::NodePatch {
                rotation: Some(deg),
                ..Default::default()
            },
        }],
        vec![id.clone()],
    )
    .unwrap();
}

#[test]
fn ungroup_without_rotation_is_bit_identical_to_the_plain_sum() {
    let mut ed = Editor::new();
    let a = add(&mut ed, rect("a", 10.1, 20.3, 20.0, 20.0));
    let b = add(&mut ed, rect("b", 50.7, 40.9, 30.0, 30.0));
    ed.selection = vec![a.clone(), b.clone()];
    ed.group_selection();
    let g = ed.node(&ed.selection[0].clone()).unwrap().clone();
    let (ax, ay) = (ed.node(&a).unwrap().x, ed.node(&a).unwrap().y);
    ed.ungroup_selection();
    let round2 = |v: f32| (v * 100.0).round() / 100.0;
    assert_eq!(ed.node(&a).unwrap().x.to_bits(), round2(ax + g.x).to_bits());
    assert_eq!(ed.node(&a).unwrap().y.to_bits(), round2(ay + g.y).to_bits());
    assert_eq!(ed.node(&a).unwrap().rotation, 0.0);
}

#[test]
fn ungroup_a_nested_rotated_group_and_normalizes_rotation() {
    let mut ed = Editor::new();
    let mut a = rect("a", 10.0, 20.0, 40.0, 20.0);
    a.rotation = 100.0;
    let a = add(&mut ed, a);
    let b = add(&mut ed, rect("b", 70.0, 5.0, 30.0, 50.0));
    let c = add(&mut ed, rect("c", 120.0, 90.0, 25.0, 25.0));
    ed.selection = vec![a.clone(), b.clone()];
    ed.group_selection();
    let inner = ed.selection[0].clone();
    rotate(&mut ed, &inner, 20.0);
    ed.selection = vec![inner.clone(), c.clone()];
    ed.group_selection();
    let outer = ed.selection[0].clone();
    rotate(&mut ed, &outer, 130.0);

    let leaves = [&a, &b, &c];
    let before: Vec<_> = leaves
        .iter()
        .map(|id| ed.geometry.get(id).unwrap().transform.as_coeffs())
        .collect();
    ed.selection = vec![outer];
    ed.ungroup_selection();
    assert_eq!(ed.page().children.len(), 2);
    ed.selection = vec![inner.clone()];
    ed.ungroup_selection();
    assert_eq!(names(&ed).len(), 3);
    for (id, want) in leaves.iter().zip(&before) {
        let got = ed.geometry.get(id).unwrap().transform.as_coeffs();
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 0.01, "{got:?} vs {want:?}");
        }
        let r = ed.node(id).unwrap().rotation;
        assert!(r > -180.0 && r <= 180.0, "{r}");
    }
}
