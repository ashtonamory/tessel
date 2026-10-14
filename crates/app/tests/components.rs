//! M5 components in the app (§3.7): create from selection, edit the master
//! on the Components page, instances with overrides, detach and delete.
//! Editor-level tests check the document and history; UI tests drive the
//! same through the headless app.

mod common;

use app::Editor;
use doc::expand::component;
use doc::{
    ColorRef, Command, ComponentId, Frame, Id, IdKind, Node, NodeId, NodeKind, NodePatch, Override,
    Paint, Rect as RectKind, Text,
};

fn rect(name: &str, x: f32, y: f32, w: f32, h: f32) -> Node {
    Node::new(
        Id::new(IdKind::Node),
        name,
        NodeKind::Rect(RectKind::default()),
    )
    .with_geometry(x, y, w, h)
}

fn text(name: &str, content: &str) -> Node {
    Node::new(
        Id::new(IdKind::Node),
        name,
        NodeKind::Text(Text {
            content: content.into(),
            ..Default::default()
        }),
    )
    .with_geometry(10.0, 10.0, 80.0, 20.0)
}

fn frame(name: &str, children: Vec<Node>) -> Node {
    Node::new(
        Id::new(IdKind::Node),
        name,
        NodeKind::Frame(Frame {
            children,
            ..Default::default()
        }),
    )
    .with_geometry(100.0, 50.0, 200.0, 100.0)
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

fn fill(c: &str) -> Vec<Paint> {
    vec![Paint::solid(ColorRef::literal(c))]
}

/// A page with a frame "Button" (a rect "bg" and a text "label"), made a
/// component; returns the instance (the frame's ID), the component and the
/// master IDs of bg and label.
fn button(ed: &mut Editor) -> (NodeId, ComponentId, NodeId, NodeId) {
    let bg = rect("bg", 0.0, 0.0, 200.0, 100.0);
    let label = text("label", "OK");
    let (bg_id, label_id) = (bg.id.clone(), label.id.clone());
    let f = add(ed, frame("Button", vec![bg, label]));
    ed.selection = vec![f.clone()];
    ed.create_component_from_selection();
    let c = ed.doc.components[0].id.clone();
    (f, c, bg_id, label_id)
}

fn derived(instance: &NodeId, master: &NodeId) -> NodeId {
    Id::derived(instance, master)
}

#[test]
fn a_frame_becomes_a_component_in_one_undo_step() {
    let mut ed = Editor::new();
    let before = ed.doc.clone();
    let (inst, c, _, _) = button(&mut ed);
    assert_eq!(ed.history.undo_label(), Some("Create component"));
    assert_eq!(ed.doc.components.len(), 1);
    assert_eq!(ed.doc.components[0].name, "Button");
    assert!(matches!(&ed.node(&inst).unwrap().kind, NodeKind::Instance(i) if i.component == c));
    assert_eq!(ed.selection, vec![inst.clone()]);
    // The instance shows the master's children under derived IDs.
    let shown = ed.geometry.expanded(&inst).unwrap();
    assert_eq!(shown.children().unwrap().len(), 2);
    ed.undo();
    ed.undo();
    assert_eq!(doc::save_string(&ed.doc), doc::save_string(&before));
}

#[test]
fn other_selections_are_framed_first() {
    let mut ed = Editor::new();
    let a = add(&mut ed, rect("a", 10.0, 10.0, 20.0, 20.0));
    let b = add(&mut ed, rect("b", 50.0, 40.0, 30.0, 30.0));
    let depth = ed.history.transactions().len();
    ed.selection = vec![a.clone(), b.clone()];
    ed.create_component_from_selection();
    assert_eq!(ed.history.transactions().len(), depth + 1);
    let page = &ed.page().children;
    assert_eq!(page.len(), 1);
    let NodeKind::Instance(i) = &page[0].kind else {
        panic!("an instance: {:?}", page[0].kind.tag());
    };
    let c = component(&ed.doc, &i.component).unwrap();
    assert_eq!(c.root.children().unwrap().len(), 2);
    assert_eq!((page[0].x, page[0].y), (10.0, 10.0));
    ed.undo();
    assert_eq!(ed.page().children.len(), 2);
    assert!(ed.doc.components.is_empty());
}

#[test]
fn editing_the_master_updates_every_instance() {
    let mut ed = Editor::new();
    let (inst, c, bg, _) = button(&mut ed);
    // A second instance.
    let other = ed
        .insert_instance(&c, Some(kurbo::Point::new(600.0, 400.0)), kurbo::Rect::ZERO)
        .unwrap();
    let page = ed.page().id.clone();
    assert!(ed.edit_component(&c));
    assert_eq!(ed.editing_component(), Some(&c));
    assert_eq!(
        ed.page().children.len(),
        1,
        "the Components page holds the master"
    );
    assert_eq!(ed.document().pages[0].id, page);
    let root = ed.master_root().unwrap().clone();
    assert_eq!(ed.selection, vec![root]);

    let patch = NodePatch {
        fills: Some(fill("#ff0000")),
        ..Default::default()
    };
    ed.commit(
        "Change fill",
        vec![Command::SetProps {
            id: bg.clone(),
            patch,
        }],
        vec![bg.clone()],
    )
    .unwrap();
    assert_eq!(ed.history.undo_label(), Some("Change fill"));
    let tx = ed.history.transactions().last().unwrap();
    assert!(matches!(&tx.forward[..], [Command::UpsertComponent { .. }]));
    let master_bg = |ed: &Editor| {
        let c = component(ed.document(), &c).unwrap();
        c.root.children().unwrap()[0].shape().unwrap().fills.clone()
    };
    assert_eq!(master_bg(&ed), fill("#ff0000"));

    // A drag in the view is one step too.
    ed.begin_gesture("Move").unwrap();
    for x in 1..=5 {
        ed.gesture_apply(Command::SetGeometry {
            id: bg.clone(),
            x: x as f32,
            y: 0.0,
            width: 200.0,
            height: 100.0,
            rotation: 0.0,
        })
        .unwrap();
    }
    ed.end_gesture();
    assert_eq!(ed.history.undo_label(), Some("Move"), "{:?}", ed.status);
    let x_of = |ed: &Editor| {
        component(ed.document(), &c)
            .unwrap()
            .root
            .children()
            .unwrap()[0]
            .x
    };
    assert_eq!(x_of(&ed), 5.0);

    // Undo stays in the view.
    ed.undo();
    assert_eq!(ed.editing_component(), Some(&c));
    assert_eq!(x_of(&ed), 0.0);
    assert_eq!(ed.node(&bg).unwrap().x, 0.0);

    ed.leave_component();
    assert_eq!(ed.editing_component(), None);
    assert_eq!(ed.page().id, page);
    for i in [&inst, &other] {
        let shown = ed.geometry.expanded(i).unwrap();
        let bg_shown = &shown.children().unwrap()[0];
        assert_eq!(bg_shown.id, derived(i, &bg));
        assert_eq!(bg_shown.shape().unwrap().fills, fill("#ff0000"));
    }
}

#[test]
fn the_master_root_stays_and_layers_stay_inside_it() {
    let mut ed = Editor::new();
    let (_, c, _, _) = button(&mut ed);
    ed.edit_component(&c);
    let root = ed.master_root().unwrap().clone();
    let depth = ed.history.transactions().len();
    ed.selection = vec![root.clone()];
    ed.delete_selection();
    assert_eq!(ed.history.transactions().len(), depth);
    assert!(ed.node(&root).is_some(), "the view is rolled back");
    assert!(ed.status.as_deref().unwrap().contains("inside its frame"));
    // New layers go in the root.
    assert_eq!(ed.insert_parent(), doc::ParentRef::Node(root.clone()));
    let r = rect("new", 5.0, 5.0, 10.0, 10.0);
    let rid = r.id.clone();
    let parent = ed.insert_parent();
    ed.commit(
        "Create rectangle",
        vec![Command::CreateNode {
            parent,
            index: None,
            node: r,
        }],
        vec![rid.clone()],
    )
    .unwrap();
    let master = component(ed.document(), &c).unwrap();
    assert!(master.root.contains(&rid));
}

#[test]
fn undoing_a_page_edit_leaves_the_component_view() {
    let mut ed = Editor::new();
    let (_, c, _, _) = button(&mut ed);
    let extra = add(&mut ed, rect("extra", 500.0, 0.0, 10.0, 10.0));
    ed.edit_component(&c);
    ed.undo();
    assert_eq!(ed.editing_component(), None);
    assert!(ed.node(&extra).is_none());
}

#[test]
fn saving_while_editing_a_component_saves_the_document() {
    let mut ed = Editor::new();
    let (inst, c, _, _) = button(&mut ed);
    ed.edit_component(&c);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tmp")
        .join(format!("app-components-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c.tessel.json");
    ed.save_as(&path).unwrap();
    let loaded = doc::load_file(&path).unwrap();
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert!(loaded.doc.node(&inst).is_some());
    assert_eq!(loaded.doc.pages.len(), 1);
    assert_eq!(loaded.doc.pages[0].name, "Page 1");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn overrides_set_and_reset_on_derived_children() {
    let mut ed = Editor::new();
    let (inst, _, bg, label) = button(&mut ed);
    let label_d = derived(&inst, &label);
    ed.selection = vec![label_d.clone()];
    let patch = ed.text_patch(&label_d, "Cancel").unwrap();
    ed.set_override("Override text", &label_d, patch);
    assert_eq!(
        ed.override_of(&label_d),
        Some(Override::Text {
            content: "Cancel".into()
        })
    );
    assert_eq!(
        ed.selection,
        vec![label_d.clone()],
        "the derived selection stays"
    );
    let bg_d = derived(&inst, &bg);
    ed.set_override(
        "Hide layer",
        &bg_d,
        NodePatch {
            visible: Some(false),
            ..Default::default()
        },
    );
    assert!(!ed.shown_node(&bg_d).unwrap().visible);
    // Only overridable props can change.
    let r = ed.commit(
        "Move",
        vec![Command::SetGeometry {
            id: bg_d.clone(),
            x: 5.0,
            y: 5.0,
            width: 1.0,
            height: 1.0,
            rotation: 0.0,
        }],
        vec![],
    );
    assert!(r.is_err());
    ed.reset_override(&label_d);
    assert_eq!(ed.override_of(&label_d), None);
    match ed.shown_node(&label_d).unwrap().kind {
        NodeKind::Text(t) => assert_eq!(t.content, "OK"),
        _ => panic!("a text"),
    }
    ed.reset_all_overrides(&inst);
    assert!(ed.shown_node(&bg_d).unwrap().visible);
    ed.undo();
    assert!(!ed.shown_node(&bg_d).unwrap().visible);
}

#[test]
fn detach_and_delete_with_detach() {
    let mut ed = Editor::new();
    let (inst, c, _, _) = button(&mut ed);
    let other = ed
        .insert_instance(&c, Some(kurbo::Point::new(600.0, 400.0)), kurbo::Rect::ZERO)
        .unwrap();
    assert_eq!(ed.component_users(&c), 2);
    ed.selection = vec![inst.clone()];
    ed.detach_selection();
    assert_eq!(ed.history.undo_label(), Some("Detach instance"));
    assert!(matches!(ed.node(&inst).unwrap().kind, NodeKind::Frame(_)));
    assert_eq!(ed.component_users(&c), 1);
    ed.delete_component(&c, true);
    assert!(ed.doc.components.is_empty());
    assert!(matches!(ed.node(&other).unwrap().kind, NodeKind::Frame(_)));
    ed.undo();
    assert_eq!(ed.doc.components.len(), 1);
    assert!(matches!(
        ed.node(&other).unwrap().kind,
        NodeKind::Instance(_)
    ));
}

#[test]
fn rename_and_props_while_editing() {
    let mut ed = Editor::new();
    let (_, c, _, label) = button(&mut ed);
    ed.edit_component(&c);
    ed.rename_component(&c, "Button/Primary".into());
    ed.update_component("Add property", &c, |comp| {
        comp.props.push(doc::ComponentProp {
            name: "label".into(),
            target: label.clone(),
            kind: doc::PropKind::Text,
        })
    })
    .unwrap();
    assert_eq!(ed.editing_component(), Some(&c));
    let comp = component(ed.document(), &c).unwrap();
    assert_eq!(comp.name, "Button/Primary");
    assert_eq!(comp.props.len(), 1);
    ed.undo();
    assert_eq!(ed.editing_component(), Some(&c));
    assert!(component(ed.document(), &c).unwrap().props.is_empty());
}

// --- UI -------------------------------------------------------------------

use app::app::LeftTab;
use app::components_panel::ComponentButton;
use common::Harness;
use egui::{Key, Modifiers};

const CMD_ALT: Modifiers = Modifiers {
    alt: true,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: true,
};

/// The app with the "Button" frame on the page, selected.
fn ui_with_button() -> (Harness, NodeId, NodeId, NodeId) {
    let mut h = Harness::new();
    let bg = rect("bg", 0.0, 0.0, 200.0, 100.0);
    let label = text("label", "OK");
    let (bg_id, label_id) = (bg.id.clone(), label.id.clone());
    let f = add(h.app.editor_mut(), frame("Button", vec![bg, label]));
    h.step(vec![]);
    (h, f, bg_id, label_id)
}

fn show_tab(h: &mut Harness, tab: LeftTab) {
    let r = h.app.left_tab_rect(tab).expect("tab drawn").center();
    h.click(r);
    assert_eq!(h.app.left_tab(), tab);
}

fn component_row(h: &Harness, c: &ComponentId) -> egui::Rect {
    h.app
        .components()
        .rows()
        .iter()
        .find(|(id, _)| id == c)
        .expect("component row drawn")
        .1
}

fn inspector_field(h: &Harness, key: &str) -> egui::Pos2 {
    h.app
        .instance_inspector()
        .field_rect(key)
        .unwrap_or_else(|| {
            panic!(
                "{key} not drawn; fields: {:?}",
                h.app.instance_inspector().field_keys()
            )
        })
        .center()
}

#[test]
fn ui_ctrl_alt_k_makes_a_component_and_ctrl_alt_b_detaches() {
    let (mut h, f, _, _) = ui_with_button();
    h.app.editor_mut().selection = vec![f.clone()];
    h.key(Key::K, CMD_ALT);
    assert_eq!(h.ed().doc.components.len(), 1);
    assert!(matches!(
        h.ed().node(&f).unwrap().kind,
        NodeKind::Instance(_)
    ));
    // The inspector shows the instance's section.
    h.step(vec![]);
    inspector_field(&h, "detach");
    h.key(Key::B, CMD_ALT);
    assert!(matches!(h.ed().node(&f).unwrap().kind, NodeKind::Frame(_)));
    assert_eq!(h.ed().history.undo_label(), Some("Detach instance"));
}

#[test]
fn ui_components_tab_places_renames_and_deletes() {
    let (mut h, f, _, _) = ui_with_button();
    h.app.editor_mut().selection = vec![f.clone()];
    h.key(Key::K, CMD_ALT);
    let c = h.ed().doc.components[0].id.clone();
    h.app
        .editor_mut()
        .rename_component(&c, "Buttons/Primary".into());
    show_tab(&mut h, LeftTab::Components);
    assert_eq!(h.app.components().group_rows()[0].0, "Buttons");

    // Click places an instance in the middle of the canvas.
    let row = component_row(&h, &c);
    h.click(egui::pos2(row.min.x + 40.0, row.center().y));
    assert_eq!(h.ed().history.undo_label(), Some("Insert instance"));
    assert_eq!(h.children().len(), 2);
    assert_eq!(h.ed().component_users(&c), 2);

    // Drag onto the canvas places one where it is dropped.
    let row = component_row(&h, &c);
    let drop = h.at(700.0, 500.0);
    h.drag(egui::pos2(row.min.x + 40.0, row.center().y), drop);
    assert_eq!(h.children().len(), 3);
    let placed = h.children().last().unwrap().clone();
    assert_eq!(
        (
            placed.x + placed.width / 2.0,
            placed.y + placed.height / 2.0
        ),
        (700.0, 500.0)
    );

    // Rename.
    let row = component_row(&h, &c);
    h.move_to(row.center());
    h.click(ComponentButton::Rename.rect(row).center());
    h.key(Key::A, Modifiers::COMMAND);
    h.type_text("Buttons/Secondary");
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(
        component(&h.ed().doc, &c).unwrap().name,
        "Buttons/Secondary"
    );

    // Delete asks first, then detaches every instance.
    let row = component_row(&h, &c);
    h.move_to(row.center());
    h.click(ComponentButton::Delete.rect(row).center());
    assert_eq!(h.app.components().pending_delete(), Some(&c));
    assert_eq!(h.ed().doc.components.len(), 1);
    h.app.answer_delete_component(true);
    assert!(h.ed().doc.components.is_empty());
    assert!(h
        .children()
        .iter()
        .all(|n| matches!(n.kind, NodeKind::Frame(_))));
}

#[test]
fn ui_edit_component_then_back_updates_instances() {
    let (mut h, f, bg, _) = ui_with_button();
    h.app.editor_mut().selection = vec![f.clone()];
    h.key(Key::K, CMD_ALT);
    let c = h.ed().doc.components[0].id.clone();
    let page = h.ed().page().id.clone();
    show_tab(&mut h, LeftTab::Components);
    let row = component_row(&h, &c);
    h.move_to(row.center());
    h.click(ComponentButton::Edit.rect(row).center());
    assert_eq!(h.ed().editing_component(), Some(&c));
    h.step(vec![]);
    // Drag the master's background on the canvas: one component update.
    h.app.editor_mut().selection = vec![bg.clone()];
    h.step(vec![]);
    let from = h.at(100.0, 50.0);
    let to = h.at(130.0, 50.0);
    h.drag(from, to);
    let master_x = |h: &Harness| {
        component(h.ed().document(), &c)
            .unwrap()
            .root
            .children()
            .unwrap()[0]
            .x
    };
    // Snapping may round the drop.
    let x = master_x(&h);
    assert!((25.0..=35.0).contains(&x), "{x}");
    assert!(matches!(
        &h.ed().history.transactions().last().unwrap().forward[..],
        [Command::UpsertComponent { .. }]
    ));
    // Esc clears the selection, a second Esc goes back to the page.
    h.key(Key::Escape, Modifiers::NONE);
    h.key(Key::Escape, Modifiers::NONE);
    assert_eq!(h.ed().editing_component(), None);
    assert_eq!(h.ed().page().id, page);
    let shown = h.ed().geometry.expanded(&f).unwrap();
    assert_eq!(shown.children().unwrap()[0].x, x);
}

#[test]
fn ui_double_click_enters_and_overrides_text_then_resets() {
    let (mut h, f, _, label) = ui_with_button();
    h.app.editor_mut().selection = vec![f.clone()];
    h.key(Key::K, CMD_ALT);
    h.app.editor_mut().selection.clear();
    h.step(vec![]);
    // Label's text starts at page (110, 60).
    let p = h.at(113.0, 66.0);
    h.click(p);
    assert_eq!(
        h.ed().selection,
        vec![f.clone()],
        "a click selects the instance"
    );
    h.key(Key::Escape, Modifiers::NONE);
    let label_d = derived(&f, &label);
    h.double_click(p);
    assert_eq!(
        h.ed().selection,
        vec![label_d.clone()],
        "double-click enters"
    );
    // Layers lists the instance's children once it is entered.
    show_tab(&mut h, LeftTab::Layers);
    assert!(h.app.layers().rows().iter().any(|(id, _)| id == &label_d));

    // Its text is an override field in the inspector.
    let key = format!("text:{label_d}");
    h.click(inspector_field(&h, &key));
    h.key(Key::A, Modifiers::COMMAND);
    h.type_text("Cancel");
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
    assert_eq!(h.ed().history.undo_label(), Some("Override text"));
    assert_eq!(
        h.ed().override_of(&label_d),
        Some(Override::Text {
            content: "Cancel".into()
        })
    );
    // Position and size are read-only inside an instance: only the
    // component section is shown.
    assert!(h.app.inspector().field_rect("x").is_none());

    h.click(inspector_field(&h, &format!("reset:{label_d}")));
    assert_eq!(h.ed().override_of(&label_d), None);

    // Go to master from inside.
    h.click(inspector_field(&h, "master"));
    assert!(h.ed().editing_component().is_some());
}

#[test]
fn ui_expose_a_text_prop_then_set_it_on_an_instance() {
    let (mut h, f, _, label) = ui_with_button();
    h.app.editor_mut().selection = vec![f.clone()];
    h.key(Key::K, CMD_ALT);
    let c = h.ed().doc.components[0].id.clone();
    h.app.editor_mut().edit_component(&c);
    h.app.editor_mut().selection = vec![label.clone()];
    h.step(vec![]);
    h.step(vec![]);
    h.click(inspector_field(&h, "expose:text"));
    let props = &component(h.ed().document(), &c).unwrap().props;
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].name, "label");
    assert_eq!(props[0].kind, doc::PropKind::Text);
    h.app.editor_mut().leave_component();
    h.app.editor_mut().selection = vec![f.clone()];
    h.step(vec![]);
    h.click(inspector_field(&h, "prop:label"));
    h.key(Key::A, Modifiers::COMMAND);
    h.type_text("Go");
    h.key(Key::Enter, Modifiers::NONE);
    h.step(vec![]);
    match h.ed().shown_node(&derived(&f, &label)).unwrap().kind {
        NodeKind::Text(t) => assert_eq!(t.content, "Go"),
        _ => panic!("a text"),
    }
}

#[test]
fn agents_and_the_export_window_see_the_real_document_while_editing_a_master() {
    use common::mcp::Session;
    use serde_json::json;

    let (mut h, f, _, _) = ui_with_button();
    h.app.editor_mut().selection = vec![f.clone()];
    h.key(Key::K, CMD_ALT);
    let c = h.ed().doc.components[0].id.clone();
    let real_page = h.ed().page().id.clone();
    let mut a = Session::connect(&mut h, "claude-code");

    assert!(h.app.editor_mut().edit_component(&c));
    h.step(vec![]);
    assert_eq!(h.ed().page().id, app::master_edit::components_page_id());

    // The Export window exports the selected master from the real document.
    h.key(Key::E, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(!h.ed().selection.is_empty(), "the master root is selected");
    let master = h.ed().selection.clone();
    // A selection the view does not have (an instance on the page, as when
    // an agent selects it) still exports: the window reads the real document.
    h.app.editor_mut().selection = vec![f.clone()];
    let files = h.app.export_files().expect("the master is exported");
    let html = files.iter().find(|f| f.path == "index.html").unwrap();
    assert!(format!("{:?}", html.content).contains("OK"), "{html:?}");
    h.app.editor_mut().selection = master;
    assert!(h.app.export_files().is_ok());
    let tokens = app::export_window::tokens_file(h.ed(), export::TokenFormat::Css);
    assert_eq!(
        tokens,
        export::export_tokens(h.ed().document(), export::TokenFormat::Css)
    );
    assert_eq!(
        h.ed().editing_component(),
        Some(&c),
        "the window only reads"
    );

    // get_document and export_code read the real document and geometry.
    let got = a.ok(&mut h, "get_document", json!({}));
    let pages = got["pages"].as_array().expect("pages");
    assert!(
        pages.iter().any(|p| p["id"] == real_page.to_string()),
        "the real pages, not the Components page: {got}"
    );
    assert!(!pages.iter().any(|p| p["name"] == "Components"));
    let code = a.ok(
        &mut h,
        "export_code",
        json!({ "id": f.to_string(), "format": "html" }),
    );
    assert!(code.to_string().contains("OK"), "{code}");
}
