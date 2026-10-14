//! Auto-layout UI (§3.5, M3): Shift+A, the inspector's auto-layout, child
//! and constraints sections, size write-back, constraints on resize and
//! the canvas's reorder drag and padding/gap handles, driven headlessly.

mod common;

use app::{DesignApp, Editor};
use common::Harness;
use doc::{
    Command, Direction, Frame, HConstraint, Id, IdKind, Node, NodeId, NodeKind, Rect, Sizing,
    SpaceRef, VConstraint,
};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};

fn rect(name: &str, x: f32, y: f32, w: f32, h: f32) -> Node {
    Node::new(Id::new(IdKind::Node), name, NodeKind::Rect(Rect::default()))
        .with_geometry(x, y, w, h)
}

/// A 300×200 frame at (0, 0) with rects a, b, c (40×40) in a row at
/// x = 10, 60, 110, y = 10. Returns the frame and the children's IDs.
fn frame_with_row(ed: &mut Editor) -> (NodeId, [NodeId; 3]) {
    let kids = [
        rect("a", 10.0, 10.0, 40.0, 40.0),
        rect("b", 60.0, 10.0, 40.0, 40.0),
        rect("c", 110.0, 10.0, 40.0, 40.0),
    ];
    let ids = kids.clone().map(|n| n.id);
    let frame = Node::new(
        Id::new(IdKind::Node),
        "Card",
        NodeKind::Frame(Frame {
            children: kids.to_vec(),
            ..Default::default()
        }),
    )
    .with_geometry(0.0, 0.0, 300.0, 200.0);
    let fid = frame.id.clone();
    let parent = ed.page_ref();
    ed.commit(
        "Add",
        vec![Command::CreateNode {
            parent,
            index: None,
            node: frame,
        }],
        vec![fid.clone()],
    )
    .unwrap();
    (fid, ids)
}

fn layout_of(ed: &Editor, id: &NodeId) -> Option<doc::AutoLayout> {
    match &ed.node(id)?.kind {
        NodeKind::Frame(f) => f.layout.clone(),
        _ => None,
    }
}

fn depth(ed: &Editor) -> usize {
    ed.history.transactions().len()
}

/// The page-space top-left of `id`.
fn pos(ed: &Editor, id: &NodeId) -> (f64, f64) {
    let b = ed.geometry.get(id).unwrap().bounds;
    (b.x0, b.y0)
}

fn harness(ed: Editor) -> Harness {
    Harness::with_app(eframe::Frame::_new_kittest(), DesignApp::new(ed))
}

#[test]
fn shift_a_adds_auto_layout_inferred_from_children_and_undo_removes_it() {
    let mut ed = Editor::new();
    let (f, [a, b, c]) = frame_with_row(&mut ed);
    let mut h = harness(ed);
    let before = depth(h.ed());
    h.key(Key::A, Modifiers::SHIFT);
    let ed = h.ed();
    assert_eq!(depth(ed), before + 1);
    assert_eq!(ed.history.undo_label(), Some("Add auto layout"));
    let al = layout_of(ed, &f).expect("auto-layout");
    assert_eq!(al.direction, Direction::Row);
    assert_eq!(al.gap, SpaceRef::Px(10.0));
    assert_eq!(
        al.padding,
        [
            SpaceRef::Px(10.0),
            SpaceRef::Px(150.0),
            SpaceRef::Px(150.0),
            SpaceRef::Px(10.0)
        ]
    );
    // Nothing moved.
    assert_eq!(pos(ed, &a), (10.0, 10.0));
    assert_eq!(pos(ed, &b), (60.0, 10.0));
    assert_eq!(pos(ed, &c), (110.0, 10.0));

    h.key(Key::Z, Modifiers::COMMAND);
    assert!(layout_of(h.ed(), &f).is_none());
    assert_eq!(depth(h.ed()), before);
    h.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(layout_of(h.ed(), &f).is_some());

    // Shift+A again removes it; the children stay where layout put them.
    h.key(Key::A, Modifiers::SHIFT);
    let ed = h.ed();
    assert!(layout_of(ed, &f).is_none());
    assert_eq!(ed.history.undo_label(), Some("Remove auto layout"));
    assert_eq!(pos(ed, &c), (110.0, 10.0));
}

#[test]
fn shift_a_orders_children_along_the_inferred_direction() {
    let mut ed = Editor::new();
    let kids = vec![
        rect("low", 0.0, 100.0, 50.0, 20.0),
        rect("top", 0.0, 0.0, 50.0, 20.0),
        rect("mid", 0.0, 40.0, 50.0, 20.0),
    ];
    let frame = Node::new(
        Id::new(IdKind::Node),
        "Stack",
        NodeKind::Frame(Frame {
            children: kids,
            ..Default::default()
        }),
    )
    .with_geometry(0.0, 0.0, 50.0, 120.0);
    let fid = frame.id.clone();
    let parent = ed.page_ref();
    ed.commit(
        "Add",
        vec![Command::CreateNode {
            parent,
            index: None,
            node: frame,
        }],
        vec![fid.clone()],
    )
    .unwrap();
    ed.toggle_auto_layout();
    let al = layout_of(&ed, &fid).unwrap();
    assert_eq!(al.direction, Direction::Column);
    assert_eq!(al.gap, SpaceRef::Px(30.0));
    let names: Vec<_> = ed
        .node(&fid)
        .unwrap()
        .children()
        .unwrap()
        .iter()
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(names, ["top", "mid", "low"]);
    ed.undo();
    let names: Vec<_> = ed
        .node(&fid)
        .unwrap()
        .children()
        .unwrap()
        .iter()
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(names, ["low", "top", "mid"]);
}

#[test]
fn shift_a_on_layers_wraps_them_in_a_hugging_auto_layout_frame() {
    let mut ed = Editor::new();
    let a = rect("a", 10.0, 10.0, 40.0, 30.0);
    let b = rect("b", 70.0, 10.0, 40.0, 30.0);
    let ids = vec![a.id.clone(), b.id.clone()];
    let parent = ed.page_ref();
    ed.commit(
        "Add",
        vec![
            Command::CreateNode {
                parent: parent.clone(),
                index: None,
                node: a,
            },
            Command::CreateNode {
                parent,
                index: None,
                node: b,
            },
        ],
        ids.clone(),
    )
    .unwrap();
    let before = depth(&ed);
    ed.toggle_auto_layout();
    assert_eq!(depth(&ed), before + 1);
    assert_eq!(ed.history.undo_label(), Some("Add auto layout"));
    let f = ed.selection[0].clone();
    let al = layout_of(&ed, &f).unwrap();
    assert_eq!((al.width, al.height), (Sizing::Hug, Sizing::Hug));
    assert_eq!(al.gap, SpaceRef::Px(20.0));
    let n = ed.node(&f).unwrap();
    assert_eq!((n.x, n.y, n.width, n.height), (10.0, 10.0, 100.0, 30.0));
    assert_eq!(pos(&ed, &ids[1]), (70.0, 10.0));
    ed.undo();
    assert_eq!(ed.page().children.len(), 2);
    assert_eq!(pos(&ed, &ids[1]), (70.0, 10.0));
}

#[test]
fn write_back_is_part_of_the_same_undo_step() {
    let mut ed = Editor::new();
    let (f, [a, ..]) = frame_with_row(&mut ed);
    ed.selection = vec![f.clone()];
    ed.toggle_auto_layout();
    let before = depth(&ed);
    // Make the frame hug: its stored size becomes the computed one,
    // 10 + 3×40 + 2×10 + 150 = 300 wide, 10 + 40 + 150 = 200 high, so
    // first shrink the padding to see a change.
    let mut al = layout_of(&ed, &f).unwrap();
    al.padding = std::array::from_fn(|_| SpaceRef::Px(5.0));
    al.width = Sizing::Hug;
    al.height = Sizing::Hug;
    ed.commit(
        "Change auto layout",
        vec![Command::SetProps {
            id: f.clone(),
            patch: doc::NodePatch {
                layout: Some(Some(al)),
                ..Default::default()
            },
        }],
        vec![f.clone()],
    )
    .unwrap();
    assert_eq!(depth(&ed), before + 1);
    let n = ed.node(&f).unwrap();
    assert_eq!((n.width, n.height), (150.0, 50.0));
    // A child that fills stores its computed width too.
    let mut lc = ed.node(&a).unwrap().layout_child.clone();
    lc.height = Sizing::Fill;
    ed.commit(
        "Fill",
        vec![Command::SetProps {
            id: a.clone(),
            patch: doc::NodePatch {
                layout_child: Some(lc),
                ..Default::default()
            },
        }],
        vec![],
    )
    .unwrap();
    assert_eq!(ed.node(&a).unwrap().height, 40.0);
    ed.undo();
    ed.undo();
    let n = ed.node(&f).unwrap();
    assert_eq!((n.width, n.height), (300.0, 200.0));
    assert_eq!(depth(&ed), before);
    ed.redo();
    assert_eq!(ed.node(&f).unwrap().width, 150.0);
    let saved = doc::save_string(&ed.doc);
    assert!(saved.contains("\"width\": 150"), "{saved}");
}

#[test]
fn constraints_move_children_of_a_resized_frame() {
    use app::auto_layout::constrained_children;
    use kurbo::Size;
    let mut frame = Node::new(
        Id::new(IdKind::Node),
        "F",
        NodeKind::Frame(Frame::default()),
    )
    .with_geometry(0.0, 0.0, 200.0, 100.0);
    let mut stretch = rect("stretch", 20.0, 10.0, 40.0, 20.0);
    stretch.constraints.horizontal = HConstraint::LeftRight;
    let mut center = rect("center", 80.0, 40.0, 40.0, 20.0);
    center.constraints.horizontal = HConstraint::Center;
    center.constraints.vertical = VConstraint::Center;
    let mut scale = rect("scale", 20.0, 60.0, 40.0, 20.0);
    scale.constraints.horizontal = HConstraint::Scale;
    scale.constraints.vertical = VConstraint::Scale;
    if let NodeKind::Frame(f) = &mut frame.kind {
        f.children = vec![stretch, center, scale];
    }
    let mut out = Vec::new();
    constrained_children(
        &frame,
        Size::new(200.0, 100.0),
        Size::new(300.0, 200.0),
        &mut out,
    );
    let geo: Vec<_> = out
        .iter()
        .map(|c| match c {
            Command::SetGeometry {
                x,
                y,
                width,
                height,
                ..
            } => (*x, *y, *width, *height),
            _ => panic!("{c:?}"),
        })
        .collect();
    assert_eq!(
        geo,
        [
            (20.0, 10.0, 140.0, 20.0),
            (130.0, 90.0, 40.0, 20.0),
            (30.0, 120.0, 60.0, 40.0)
        ]
    );
}

// --- Inspector ----------------------------------------------------------------

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

/// The frame of `frame_with_row`, selected, with auto-layout added.
fn auto_frame() -> (Harness, NodeId, [NodeId; 3]) {
    let mut ed = Editor::new();
    let (f, kids) = frame_with_row(&mut ed);
    let mut h = harness(ed);
    h.click(field(&h, "layout.toggle"));
    assert!(layout_of(h.ed(), &f).is_some());
    (h, f, kids)
}

#[test]
fn inspector_adds_and_removes_auto_layout_in_one_step_each() {
    let mut ed = Editor::new();
    let (f, [_, _, c]) = frame_with_row(&mut ed);
    let mut h = harness(ed);
    let before = depth(h.ed());
    assert!(h.app.inspector().field_rect("layout.gap").is_none());
    h.click(field(&h, "layout.toggle"));
    assert_eq!(depth(h.ed()), before + 1);
    assert_eq!(h.ed().history.undo_label(), Some("Add auto layout"));
    assert_eq!(text(&h, "layout.gap"), "10");
    // The sides differ, so each has its own field.
    assert_eq!(text(&h, "layout.padding.right"), "150");
    h.click(field(&h, "layout.toggle"));
    assert!(layout_of(h.ed(), &f).is_none());
    assert_eq!(pos(h.ed(), &c), (110.0, 10.0));
    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(layout_of(h.ed(), &f).is_none());
    assert_eq!(depth(h.ed()), before);
}

#[test]
fn inspector_changes_direction_gap_padding_and_justify() {
    let (mut h, f, [a, b, _]) = auto_frame();
    let before = depth(h.ed());

    type_into(&mut h, "layout.gap", "24");
    assert_eq!(layout_of(h.ed(), &f).unwrap().gap, SpaceRef::Px(24.0));
    assert_eq!(pos(h.ed(), &b), (74.0, 10.0));
    assert_eq!(h.ed().history.undo_label(), Some("Change gap"));

    // Per-side padding is shown because the sides differ; back to one.
    h.click(field(&h, "layout.padding.each"));
    assert_eq!(text(&h, "layout.padding"), "10");
    type_into(&mut h, "layout.padding", "20");
    let al = layout_of(h.ed(), &f).unwrap();
    assert!(al.padding.iter().all(|p| *p == SpaceRef::Px(20.0)));
    assert_eq!(pos(h.ed(), &a), (20.0, 20.0));

    // One side.
    h.click(field(&h, "layout.padding.each"));
    type_into(&mut h, "layout.padding.left", "5");
    assert_eq!(pos(h.ed(), &a), (5.0, 20.0));

    h.click(field(&h, "layout.direction.column"));
    assert_eq!(layout_of(h.ed(), &f).unwrap().direction, Direction::Column);
    assert_eq!(pos(h.ed(), &b), (5.0, 84.0));

    h.click(field(&h, "layout.justify.center"));
    assert_eq!(layout_of(h.ed(), &f).unwrap().justify, doc::Justify::Center);
    // 3×40 + 2×24 = 168 in 200 - 40 padding overflows by 8, centered.
    assert_eq!(pos(h.ed(), &a), (5.0, 16.0));

    h.click(field(&h, "layout.align.center"));
    // 300 - 5 - 20 = 275 wide, 40 wide child: (275 - 40) / 2 + 5.
    assert_eq!(pos(h.ed(), &a), (122.5, 16.0));

    assert_eq!(depth(h.ed()), before + 7);
    for _ in 0..7 {
        h.key(Key::Z, Modifiers::COMMAND);
    }
    assert_eq!(pos(h.ed(), &b), (60.0, 10.0));
}

#[test]
fn a_spacing_token_shows_its_name_and_typing_px_replaces_it() {
    let mut ed = Editor::new();
    let (f, [_, b, _]) = frame_with_row(&mut ed);
    let mut patch = doc::TokensPatch::default();
    patch
        .spacing
        .insert(doc::TokenId("space.md".into()), Some(16.0));
    ed.commit(
        "Tokens",
        vec![Command::UpsertTokens { patch }],
        vec![f.clone()],
    )
    .unwrap();
    ed.toggle_auto_layout();
    let mut al = layout_of(&ed, &f).unwrap();
    al.gap = SpaceRef::Token(doc::TokenId("space.md".into()));
    ed.commit(
        "Token gap",
        vec![Command::SetProps {
            id: f.clone(),
            patch: doc::NodePatch {
                layout: Some(Some(al)),
                ..Default::default()
            },
        }],
        vec![f.clone()],
    )
    .unwrap();
    assert_eq!(pos(&ed, &b), (66.0, 10.0));
    let mut h = harness(ed);
    assert_eq!(text(&h, "layout.gap"), "{space.md}");
    type_into(&mut h, "layout.gap", "4");
    assert_eq!(layout_of(h.ed(), &f).unwrap().gap, SpaceRef::Px(4.0));
    assert_eq!(pos(h.ed(), &b), (54.0, 10.0));
}

#[test]
fn hug_and_fill_children() {
    let (mut h, f, [a, b, c]) = auto_frame();
    // The frame hugs its content: 10 + 140 + 150 padding, so drop the big
    // right and bottom padding first.
    h.click(field(&h, "layout.padding.each"));
    type_into(&mut h, "layout.padding", "10");
    h.click(field(&h, "layout.width.hug"));
    h.click(field(&h, "layout.height.hug"));
    let n = h.ed().node(&f).unwrap();
    assert_eq!((n.width, n.height), (160.0, 60.0));

    // Back to a fixed 300 wide, then b fills the rest of the row.
    h.click(field(&h, "layout.width.fixed"));
    type_into(&mut h, "width", "300");
    assert_eq!(h.ed().node(&f).unwrap().width, 300.0);
    h.click_with(h.at(80.0, 30.0), Modifiers::COMMAND);
    assert_eq!(h.ed().selection, vec![b.clone()]);
    let before = depth(h.ed());
    h.click(field(&h, "child.width.fill"));
    assert_eq!(depth(h.ed()), before + 1);
    let ed = h.ed();
    // 300 - 20 padding - 2×40 - 2×10 gaps = 180.
    assert_eq!(ed.geometry.get(&b).unwrap().size.width, 180.0);
    // Written back to the file in the same step.
    assert_eq!(ed.node(&b).unwrap().width, 180.0);
    assert_eq!(pos(ed, &c), (250.0, 10.0));
    // Fill on the cross axis stretches.
    h.click(field(&h, "child.height.fill"));
    assert_eq!(h.ed().node(&b).unwrap().height, 40.0);
    h.click(field(&h, "child.align_self.end"));
    assert_eq!(
        h.ed().node(&b).unwrap().layout_child.align_self,
        Some(doc::Align::End)
    );

    type_into(&mut h, "child.max_width", "100");
    assert_eq!(h.ed().node(&b).unwrap().width, 100.0);
    h.click(field(&h, "child.max_width"));
    h.key(Key::Backspace, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
    assert_eq!(h.ed().node(&b).unwrap().layout_child.max_width, None);
    assert_eq!(h.ed().node(&b).unwrap().width, 180.0);

    // Absolute children leave the flow and get constraints.
    h.click(field(&h, "child.position.absolute"));
    assert!(h.app.inspector().field_rect("constraints.h.left").is_some());
    assert_eq!(pos(h.ed(), &c), (60.0, 10.0));
    let _ = a;
}

// --- Constraints on resize ----------------------------------------------------

#[test]
fn resizing_a_frame_applies_its_childrens_constraints_in_one_step() {
    let mut ed = Editor::new();
    let mut stretch = rect("stretch", 20.0, 10.0, 40.0, 20.0);
    stretch.constraints.horizontal = HConstraint::LeftRight;
    let mut center = rect("center", 80.0, 40.0, 40.0, 20.0);
    center.constraints.horizontal = HConstraint::Center;
    center.constraints.vertical = VConstraint::Center;
    let mut scale = rect("scale", 20.0, 60.0, 40.0, 20.0);
    scale.constraints.horizontal = HConstraint::Scale;
    scale.constraints.vertical = VConstraint::Scale;
    let left = rect("left", 150.0, 10.0, 20.0, 20.0);
    let ids = [&stretch, &center, &scale, &left].map(|n| n.id.clone());
    let frame = Node::new(
        Id::new(IdKind::Node),
        "F",
        NodeKind::Frame(Frame {
            children: vec![stretch, center, scale, left],
            ..Default::default()
        }),
    )
    .with_geometry(0.0, 0.0, 200.0, 100.0);
    let f = frame.id.clone();
    let parent = ed.page_ref();
    ed.commit(
        "Add",
        vec![Command::CreateNode {
            parent,
            index: None,
            node: frame,
        }],
        vec![f.clone()],
    )
    .unwrap();
    let mut h = harness(ed);
    let before = depth(h.ed());
    h.drag(h.at(200.0, 100.0), h.at(300.0, 200.0));
    let ed = h.ed();
    let n = ed.node(&f).unwrap();
    assert_eq!((n.width, n.height), (300.0, 200.0));
    assert_eq!(depth(ed), before + 1);
    let geo = |id: &NodeId| {
        let n = ed.node(id).unwrap();
        (n.x, n.y, n.width, n.height)
    };
    assert_eq!(geo(&ids[0]), (20.0, 10.0, 140.0, 20.0));
    assert_eq!(geo(&ids[1]), (130.0, 90.0, 40.0, 20.0));
    assert_eq!(geo(&ids[2]), (30.0, 120.0, 60.0, 40.0));
    assert_eq!(geo(&ids[3]), (150.0, 10.0, 20.0, 20.0));
    h.key(Key::Z, Modifiers::COMMAND);
    let ed = h.ed();
    assert_eq!(ed.node(&ids[0]).unwrap().width, 40.0);
    assert_eq!(ed.node(&ids[1]).unwrap().x, 80.0);
    assert_eq!(ed.node(&f).unwrap().width, 200.0);
}

#[test]
fn constraints_controls_appear_for_children_of_plain_frames() {
    let mut ed = Editor::new();
    let (_, [a, ..]) = frame_with_row(&mut ed);
    ed.selection = vec![a.clone()];
    let mut h = harness(ed);
    assert!(h.app.inspector().field_rect("child.width.fill").is_none());
    h.click(field(&h, "constraints.h.left_right"));
    h.click(field(&h, "constraints.v.bottom"));
    let c = &h.ed().node(&a).unwrap().constraints;
    assert_eq!(
        (c.horizontal, c.vertical),
        (HConstraint::LeftRight, VConstraint::Bottom)
    );
}

#[test]
fn resizing_a_hugging_auto_layout_frame_makes_it_fixed() {
    let (mut h, f, _) = auto_frame();
    h.click(field(&h, "layout.width.hug"));
    // 10 + 140 + 150 padding.
    assert_eq!(h.ed().node(&f).unwrap().width, 300.0);
    h.click(field(&h, "layout.padding.right"));
    h.type_text("10");
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
    assert_eq!(h.ed().node(&f).unwrap().width, 160.0);
    let before = depth(h.ed());
    h.drag(h.at(160.0, 200.0), h.at(250.0, 200.0));
    let ed = h.ed();
    assert_eq!(depth(ed), before + 1);
    assert_eq!(ed.node(&f).unwrap().width, 250.0);
    assert_eq!(layout_of(ed, &f).unwrap().width, Sizing::Fixed);
}

// --- Canvas -------------------------------------------------------------------

/// A drag with `mods` held for every frame.
fn drag_with(h: &mut Harness, from: Pos2, to: Pos2, mods: Modifiers) {
    let m = Event::ModifiersChanged(mods);
    h.step(vec![m.clone(), Event::PointerMoved(from)]);
    let press = |pressed, pos| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: mods,
    };
    h.step(vec![m.clone(), press(true, from)]);
    for i in 1..=8 {
        let p = from + (to - from) * (i as f32 / 8.0);
        h.step(vec![m.clone(), Event::PointerMoved(p)]);
    }
    h.step(vec![m.clone(), press(false, to)]);
    h.step(vec![Event::ModifiersChanged(Modifiers::NONE)]);
}

fn child_names(ed: &Editor, f: &NodeId) -> Vec<String> {
    ed.node(f)
        .unwrap()
        .children()
        .unwrap()
        .iter()
        .map(|n| n.name.clone())
        .collect()
}

#[test]
fn dragging_a_flow_child_reorders_it() {
    let (mut h, f, [_, b, _]) = auto_frame();
    let before = depth(h.ed());
    let stored = h.ed().node(&b).unwrap().clone();
    h.drag(h.at(80.0, 30.0), h.at(140.0, 30.0));
    let ed = h.ed();
    assert_eq!(child_names(ed, &f), ["a", "c", "b"]);
    assert_eq!(depth(ed), before + 1);
    assert_eq!(ed.history.undo_label(), Some("Reorder layer"));
    assert_eq!(ed.selection, vec![b.clone()]);
    // Not moved: layout places it.
    let n = ed.node(&b).unwrap();
    assert_eq!((n.x, n.y), (stored.x, stored.y));
    assert_eq!(pos(ed, &b), (110.0, 10.0));

    // Back to the front, before a.
    h.drag(h.at(130.0, 30.0), h.at(15.0, 30.0));
    assert_eq!(child_names(h.ed(), &f), ["b", "a", "c"]);
    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(child_names(h.ed(), &f), ["a", "b", "c"]);
}

#[test]
fn alt_drag_takes_a_flow_child_out_of_the_frame() {
    let (mut h, f, [_, _, c]) = auto_frame();
    let before = depth(h.ed());
    let (from, to) = (h.at(130.0, 30.0), h.at(130.0, 330.0));
    drag_with(&mut h, from, to, Modifiers::ALT);
    let ed = h.ed();
    assert_eq!(child_names(ed, &f), ["a", "b"]);
    assert_eq!(depth(ed), before + 1);
    let names: Vec<_> = ed.page().children.iter().map(|n| n.name.clone()).collect();
    assert_eq!(names, ["Card", "c"]);
    assert_eq!(pos(ed, &c), (110.0, 310.0));
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(child_names(h.ed(), &f), ["a", "b", "c"]);
    assert_eq!(pos(h.ed(), &c), (110.0, 10.0));
}

#[test]
fn padding_and_gap_handles_drag_in_one_step_each() {
    let (mut h, f, [a, b, _]) = auto_frame();
    let handles = app::layout_handles::handles(h.ed(), &f, h.app.canvas().view.zoom as f64);
    let find = |s| handles.iter().find(|(k, _)| *k == s).unwrap().1;
    use app::layout_handles::Spacing;
    // Left padding 10: its handle is in the middle of the band.
    let left = find(Spacing::Padding(3));
    let before = depth(h.ed());
    h.drag(h.at(left.x, left.y), h.at(left.x + 20.0, left.y));
    let al = layout_of(h.ed(), &f).unwrap();
    assert_eq!(al.padding[3], SpaceRef::Px(30.0));
    assert_eq!(pos(h.ed(), &a), (30.0, 10.0));
    assert_eq!(depth(h.ed()), before + 1);
    assert_eq!(h.ed().history.undo_label(), Some("Change padding"));

    let handles = app::layout_handles::handles(h.ed(), &f, h.app.canvas().view.zoom as f64);
    let gap = handles.iter().find(|(k, _)| *k == Spacing::Gap).unwrap().1;
    h.drag(h.at(gap.x, gap.y), h.at(gap.x + 15.0, gap.y));
    assert_eq!(layout_of(h.ed(), &f).unwrap().gap, SpaceRef::Px(25.0));
    assert_eq!(pos(h.ed(), &b), (95.0, 10.0));
    assert_eq!(depth(h.ed()), before + 2);
    assert_eq!(h.ed().history.undo_label(), Some("Change gap"));
    h.key(Key::Z, Modifiers::COMMAND);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(pos(h.ed(), &b), (60.0, 10.0));
}

#[test]
fn spacing_areas_cover_padding_and_gaps() {
    let (h, f, _) = auto_frame();
    let areas = app::layout_handles::areas(h.ed(), &f);
    let gaps: Vec<_> = areas
        .iter()
        .filter(|a| a.spacing == app::layout_handles::Spacing::Gap)
        .map(|a| (a.rect.x0, a.rect.x1))
        .collect();
    assert_eq!(gaps, [(50.0, 60.0), (100.0, 110.0)]);
    assert_eq!(areas[1].rect, kurbo::Rect::new(150.0, 0.0, 300.0, 200.0));
}

#[test]
fn spacing_token_pickers_set_and_unlink_gap_and_padding_tokens() {
    let mut ed = Editor::new();
    let (f, [a, b, _]) = frame_with_row(&mut ed);
    let mut patch = doc::TokensPatch::default();
    patch
        .spacing
        .insert(doc::TokenId("space.lg".into()), Some(24.0));
    ed.commit(
        "Tokens",
        vec![Command::UpsertTokens { patch }],
        vec![f.clone()],
    )
    .unwrap();
    ed.toggle_auto_layout();
    let mut h = harness(ed);

    let before = depth(h.ed());
    h.click(field(&h, "layout.gap.token"));
    h.click(field(&h, "layout.gap.token.item.space.lg"));
    let token = SpaceRef::Token(doc::TokenId("space.lg".into()));
    assert_eq!(layout_of(h.ed(), &f).unwrap().gap, token);
    assert_eq!(text(&h, "layout.gap"), "{space.lg}");
    assert_eq!(pos(h.ed(), &b), (74.0, 10.0));
    assert_eq!(depth(h.ed()), before + 1);
    assert_eq!(h.ed().history.undo_label(), Some("Change gap"));

    // Unlinking keeps the px value the token gave.
    h.click(field(&h, "layout.gap.token"));
    h.click(field(&h, "layout.gap.token.none"));
    assert_eq!(layout_of(h.ed(), &f).unwrap().gap, SpaceRef::Px(24.0));

    // Padding per side (the sides differ) has a picker for each side.
    h.click(field(&h, "layout.padding.left.token"));
    h.click(field(&h, "layout.padding.left.token.item.space.lg"));
    assert_eq!(layout_of(h.ed(), &f).unwrap().padding[3], token);
    assert_eq!(pos(h.ed(), &a), (24.0, 10.0));
    assert_eq!(depth(h.ed()), before + 3);
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(pos(h.ed(), &a), (10.0, 10.0));
}
