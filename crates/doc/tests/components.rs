//! Components and instances (§3.7): the component commands, override
//! patches, instance expansion and derived-ID lookup.

use std::collections::BTreeMap;

use doc::*;
use serde_json::json;

fn id(s: &str) -> Id {
    Id::from_raw(s)
}

fn rect(i: &str, x: f32, y: f32) -> Node {
    Node::new(id(i), "Rect", NodeKind::Rect(Rect::default())).with_geometry(x, y, 20.0, 10.0)
}

fn text(i: &str, content: &str) -> Node {
    Node::new(
        id(i),
        "Label",
        NodeKind::Text(Text {
            content: content.into(),
            ..Text::default()
        }),
    )
    .with_geometry(10.0, 10.0, 50.0, 20.0)
}

fn frame(i: &str, children: Vec<Node>) -> Node {
    Node::new(
        id(i),
        "Frame",
        NodeKind::Frame(Frame {
            children,
            ..Frame::default()
        }),
    )
    .with_geometry(0.0, 0.0, 100.0, 40.0)
}

fn instance(i: &str, component: &str) -> Node {
    Node::new(
        id(i),
        "Instance",
        NodeKind::Instance(Instance {
            component: id(component),
            overrides: BTreeMap::new(),
            extra: Extra::new(),
        }),
    )
    .with_geometry(0.0, 0.0, 100.0, 40.0)
}

fn with_overrides(mut n: Node, overrides: &[(&str, Override)]) -> Node {
    if let NodeKind::Instance(i) = &mut n.kind {
        for (k, o) in overrides {
            i.overrides.insert(k.to_string(), o.clone());
        }
    }
    n
}

fn component(i: &str, name: &str, root: Node) -> Component {
    Component {
        id: id(i),
        name: name.into(),
        props: vec![],
        extra: Extra::new(),
        root,
    }
}

fn red() -> Vec<Paint> {
    vec![Paint::solid(ColorRef::literal("#ff0000"))]
}

const PAGE: &str = "p_0000000001";
const BUTTON: &str = "c_00000000b1";
const CARD: &str = "c_00000000c1";
const ICON: &str = "c_00000000k1";
// Button master: root [bg rect, label text]
const B_ROOT: &str = "n_00000000b0";
const B_BG: &str = "n_00000000b2";
const B_LABEL: &str = "n_00000000b3";
// Icon master: root [shape]
const I_ROOT: &str = "n_00000000a0";
const I_SHAPE: &str = "n_00000000a1";
// Card master: root [title text, button instance]
const C_ROOT: &str = "n_00000000c0";
const C_TITLE: &str = "n_00000000c2";
const C_BUTTON: &str = "n_00000000c3";
// Page nodes.
const SCREEN: &str = "n_00000000s1";
const INST: &str = "n_00000000p1";
const CARD_INST: &str = "n_00000000p2";
const PLAIN: &str = "n_00000000p3";

/// Components Button, Icon and Card (which nests a Button); a page with a
/// screen frame holding a Button instance, a Card instance and a frame.
fn sample() -> Document {
    let mut page = Page::new(id(PAGE), "Page 1");
    page.children = vec![frame(
        SCREEN,
        vec![
            instance(INST, BUTTON).with_geometry(10.0, 10.0, 100.0, 40.0),
            instance(CARD_INST, CARD).with_geometry(10.0, 100.0, 200.0, 120.0),
            frame(PLAIN, vec![rect("n_00000000p4", 1.0, 1.0)]),
        ],
    )
    .with_geometry(0.0, 0.0, 800.0, 600.0)];
    let mut d = Document::new("Test", page);
    d.components.push(component(
        BUTTON,
        "Button",
        frame(B_ROOT, vec![rect(B_BG, 0.0, 0.0), text(B_LABEL, "Click")]),
    ));
    d.components.push(component(
        ICON,
        "Icon",
        frame(I_ROOT, vec![rect(I_SHAPE, 2.0, 2.0)]),
    ));
    d.components.push(component(
        CARD,
        "Card",
        frame(
            C_ROOT,
            vec![
                text(C_TITLE, "Title"),
                with_overrides(
                    instance(C_BUTTON, BUTTON).with_geometry(10.0, 60.0, 100.0, 40.0),
                    &[(
                        B_LABEL,
                        Override::Text {
                            content: "Card button".into(),
                        },
                    )],
                ),
            ],
        )
        .with_geometry(0.0, 0.0, 200.0, 120.0),
    ));
    d.rebuild_index();
    assert!(validate(&d).is_empty(), "{:?}", validate(&d));
    d
}

/// Apply, check the inverse restores the document byte for byte, and the
/// inverse's inverse redoes it exactly. Returns the document after `c`.
fn round_trip(d: &Document, c: Command) -> Document {
    let mut d = d.clone();
    let before = d.clone();
    let before_bytes = save_string(&d);
    let inverse = c
        .clone()
        .apply(&mut d)
        .unwrap_or_else(|e| panic!("{e}\n{c:?}"));
    let after = d.clone();
    assert!(validate(&after).is_empty(), "{:?}", validate(&after));
    let redo = inverse.apply(&mut d).expect("inverse applies");
    assert_eq!(d, before);
    assert_eq!(save_string(&d), before_bytes);
    redo.apply(&mut d).expect("redo applies");
    assert_eq!(d, after);
    // The command is deterministic: applying it again from the original
    // state gives the same document (history redo re-applies forward
    // commands).
    let mut again = before.clone();
    c.apply(&mut again).expect("applies again");
    assert_eq!(again, after);
    after
}

fn apply_err(d: &Document, c: Command) -> CommandError {
    let mut d2 = d.clone();
    let e = c.apply(&mut d2).expect_err("should fail");
    assert_eq!(&d2, d, "a failed command changed the document");
    e
}

fn children_of(n: &Node) -> Vec<String> {
    n.children()
        .map(|c| c.iter().map(|c| c.id.to_string()).collect())
        .unwrap_or_default()
}

// --- CreateComponent ------------------------------------------------------

#[test]
fn create_component_leaves_an_instance_in_place() {
    let d = sample();
    let mut plain = d.node(&id(PLAIN)).unwrap().clone();
    plain.x = 300.0;
    plain.y = 20.0;
    plain.name = "Badge".into();
    let mut d = d;
    *d.node_mut(&id(PLAIN)).unwrap() = plain.clone();
    let after = round_trip(&d, Command::CreateComponent { from: id(PLAIN) });
    let inst = after
        .node(&id(PLAIN))
        .expect("instance keeps the frame's ID");
    let NodeKind::Instance(i) = &inst.kind else {
        panic!("not an instance: {inst:?}");
    };
    assert!(i.overrides.is_empty());
    assert_eq!(
        (inst.x, inst.y, inst.width, inst.height, inst.name.as_str()),
        (300.0, 20.0, 100.0, 40.0, "Badge")
    );
    // Same place in the parent.
    let (parent, at) = after.parent_of(&id(PLAIN)).unwrap();
    assert_eq!((parent, at), (ParentRef::Node(id(SCREEN)), 2));
    let comp = after
        .components
        .iter()
        .find(|c| c.id == i.component)
        .unwrap();
    assert_eq!(comp.name, "Badge");
    assert!(comp.id.check(IdKind::Component).is_ok());
    assert!(comp.root.id.check(IdKind::Node).is_ok());
    assert_ne!(comp.root.id, id(PLAIN));
    // The frame's children moved into the master with their IDs.
    assert_eq!(children_of(&comp.root), vec!["n_00000000p4"]);
    assert!(after.node(&id("n_00000000p4")).is_none());
    // The instance shows the same subtree, under derived IDs.
    let shown = expand_instance(&after, inst).unwrap();
    assert_eq!(children_of(&shown), vec!["n_00000000p3/n_00000000p4"]);
}

#[test]
fn create_component_errors() {
    let d = sample();
    let e = apply_err(
        &d,
        Command::CreateComponent {
            from: id("n_00000000p4"),
        },
    );
    assert!(matches!(e, CommandError::Invariant(_)), "{e:?}");
    let e = apply_err(
        &d,
        Command::CreateComponent {
            from: id("n_0000000none"),
        },
    );
    assert!(matches!(e, CommandError::NotFound(_)), "{e:?}");
    // Master nodes are not on a page.
    let e = apply_err(&d, Command::CreateComponent { from: id(B_ROOT) });
    assert!(matches!(e, CommandError::NotFound(_)), "{e:?}");
}

#[test]
fn create_component_twice_from_the_same_id_makes_distinct_components() {
    let mut d = sample();
    Command::CreateComponent { from: id(PLAIN) }
        .apply(&mut d)
        .unwrap();
    Command::DetachInstance { id: id(PLAIN) }
        .apply(&mut d)
        .unwrap();
    Command::CreateComponent { from: id(PLAIN) }
        .apply(&mut d)
        .unwrap();
    assert_eq!(d.components.len(), 5);
    assert!(validate(&d).is_empty(), "{:?}", validate(&d));
}

// --- UpsertComponent / DeleteComponent -------------------------------------

#[test]
fn upsert_component_adds_and_replaces() {
    let d = sample();
    let new = component(
        "c_00000000n1",
        "New",
        frame("n_00000000n0", vec![rect("n_00000000n2", 0.0, 0.0)]),
    );
    let after = round_trip(
        &d,
        Command::UpsertComponent {
            component: new.clone(),
        },
    );
    assert_eq!(after.components.last(), Some(&new));

    let mut changed = d.components[0].clone();
    changed.name = "Button/Primary".into();
    changed
        .root
        .children_mut()
        .unwrap()
        .push(rect("n_00000000b9", 5.0, 5.0));
    let after = round_trip(
        &d,
        Command::UpsertComponent {
            component: changed.clone(),
        },
    );
    assert_eq!(after.components[0], changed);
}

#[test]
fn upsert_component_errors() {
    let d = sample();
    // A root node ID already used on a page.
    let dup = component("c_00000000n1", "Dup", frame(PLAIN, vec![]));
    assert!(matches!(
        apply_err(&d, Command::UpsertComponent { component: dup }),
        CommandError::DuplicateId(_)
    ));
    // A component ID used by a page.
    let dup = component(PAGE, "Dup", frame("n_00000000n0", vec![]));
    assert!(matches!(
        apply_err(&d, Command::UpsertComponent { component: dup }),
        CommandError::Invariant(_) | CommandError::DuplicateId(_)
    ));
    // A root that is not a frame.
    let bad = component("c_00000000n1", "Bad", rect("n_00000000n0", 0.0, 0.0));
    assert!(matches!(
        apply_err(&d, Command::UpsertComponent { component: bad }),
        CommandError::Invariant(_)
    ));
    // Button containing a Card, which contains a Button: a cycle.
    let mut cyclic = d.components[0].clone();
    cyclic
        .root
        .children_mut()
        .unwrap()
        .push(instance("n_00000000b9", CARD));
    let e = apply_err(&d, Command::UpsertComponent { component: cyclic });
    assert!(e.to_string().contains("instance of itself"), "{e}");
    // An instance of a missing component.
    let mut dangling = d.components[1].clone();
    dangling
        .root
        .children_mut()
        .unwrap()
        .push(instance("n_00000000a9", "c_000000none"));
    let e = apply_err(
        &d,
        Command::UpsertComponent {
            component: dangling,
        },
    );
    assert!(e.to_string().contains("unknown component"), "{e}");
}

#[test]
fn delete_component_is_refused_while_used() {
    let d = sample();
    let e = apply_err(
        &d,
        Command::DeleteComponent {
            id: id(BUTTON),
            detach: false,
        },
    );
    assert!(e.to_string().contains(INST), "{e}");
    assert!(e.to_string().contains(C_BUTTON), "{e}");
    // An unused component goes.
    let after = round_trip(
        &d,
        Command::DeleteComponent {
            id: id(ICON),
            detach: false,
        },
    );
    assert_eq!(after.components.len(), 2);
    assert!(matches!(
        apply_err(
            &d,
            Command::DeleteComponent {
                id: id("c_000000none"),
                detach: true
            }
        ),
        CommandError::NotFound(_)
    ));
}

#[test]
fn delete_component_with_detach_detaches_every_use() {
    let mut d = sample();
    // A swap to Button elsewhere: Card instance swapping its button for an
    // Icon is fine; an icon swapped to a Button is a use.
    let mut icon_inst = instance("n_00000000p5", ICON);
    icon_inst = with_overrides(icon_inst, &[]);
    d.pages[0].children.push(icon_inst);
    d.components[2]
        .root
        .children_mut()
        .unwrap()
        .push(with_overrides(
            instance("n_00000000c9", ICON),
            &[("n_00000000c9", Override::Visible { visible: true })],
        ));
    if let NodeKind::Instance(i) = &mut d.node_mut(&id(CARD_INST)).unwrap().kind {
        i.overrides.insert(
            "n_00000000c9".into(),
            Override::Swap {
                component: id(BUTTON),
            },
        );
    }
    d.rebuild_index();
    assert!(validate(&d).is_empty(), "{:?}", validate(&d));

    let after = round_trip(
        &d,
        Command::DeleteComponent {
            id: id(BUTTON),
            detach: true,
        },
    );
    assert!(after.components.iter().all(|c| c.id != id(BUTTON)));
    // The page instance is a frame now, with the instance's ID.
    let n = after.node(&id(INST)).unwrap();
    assert!(matches!(n.kind, NodeKind::Frame(_)));
    assert_eq!(n.children().unwrap().len(), 2);
    // The Card master's button was detached in place, its override baked.
    let card = &after
        .components
        .iter()
        .find(|c| c.id == id(CARD))
        .unwrap()
        .root;
    let b = card
        .children()
        .unwrap()
        .iter()
        .find(|c| c.id == id(C_BUTTON))
        .unwrap();
    assert!(matches!(b.kind, NodeKind::Frame(_)));
    let label = &b.children().unwrap()[1];
    assert!(matches!(&label.kind, NodeKind::Text(t) if t.content == "Card button"));
    // The swap to Button is cleared.
    let NodeKind::Instance(ci) = &after.node(&id(CARD_INST)).unwrap().kind else {
        panic!()
    };
    assert!(!ci.overrides.contains_key("n_00000000c9"));
}

// --- DetachInstance -----------------------------------------------------

#[test]
fn detach_instance_bakes_overrides_with_fresh_ids() {
    let mut d = sample();
    if let NodeKind::Instance(i) = &mut d.node_mut(&id(INST)).unwrap().kind {
        i.overrides.insert(
            B_LABEL.into(),
            Override::Text {
                content: "OK".into(),
            },
        );
        i.overrides
            .insert(B_BG.into(), Override::Fills { fills: red() });
    }
    let after = round_trip(&d, Command::DetachInstance { id: id(INST) });
    let f = after.node(&id(INST)).unwrap();
    let NodeKind::Frame(_) = &f.kind else {
        panic!("{f:?}")
    };
    assert_eq!((f.x, f.y, f.width, f.height), (10.0, 10.0, 100.0, 40.0));
    let kids = f.children().unwrap();
    assert_eq!(kids.len(), 2);
    for k in kids {
        assert!(k.id.check(IdKind::Node).is_ok(), "{}", k.id);
        assert!(!k.id.is_derived());
        assert!(after.node(&k.id).is_some());
    }
    assert_eq!(kids[0].shape().unwrap().fills, red());
    assert!(matches!(&kids[1].kind, NodeKind::Text(t) if t.content == "OK"));
    // Not an instance.
    assert!(matches!(
        apply_err(&d, Command::DetachInstance { id: id(PLAIN) }),
        CommandError::Invariant(_)
    ));
}

#[test]
fn detach_keeps_nested_instances_with_merged_overrides() {
    let mut d = sample();
    let key = format!("{C_BUTTON}/{B_LABEL}");
    if let NodeKind::Instance(i) = &mut d.node_mut(&id(CARD_INST)).unwrap().kind {
        i.overrides.insert(
            key,
            Override::Text {
                content: "Outer".into(),
            },
        );
        i.overrides.insert(
            C_TITLE.into(),
            Override::Text {
                content: "Hi".into(),
            },
        );
    }
    let after = round_trip(&d, Command::DetachInstance { id: id(CARD_INST) });
    let f = after.node(&id(CARD_INST)).unwrap();
    let kids = f.children().unwrap();
    assert!(matches!(&kids[0].kind, NodeKind::Text(t) if t.content == "Hi"));
    let NodeKind::Instance(nested) = &kids[1].kind else {
        panic!("{:?}", kids[1])
    };
    assert_eq!(nested.component, id(BUTTON));
    // The outer override won over the master's own.
    assert_eq!(
        nested.overrides.get(B_LABEL),
        Some(&Override::Text {
            content: "Outer".into()
        })
    );
}

// --- Override patches ---------------------------------------------------

#[test]
fn set_props_sets_and_clears_overrides_per_key() {
    let d = sample();
    let set = NodePatch::from_json(json!({
        "overrides": {
            B_LABEL: { "type": "text", "content": "Buy" },
            B_BG: { "type": "visible", "visible": false }
        }
    }))
    .unwrap();
    let after = round_trip(
        &d,
        Command::SetProps {
            id: id(INST),
            patch: set,
        },
    );
    let NodeKind::Instance(i) = &after.node(&id(INST)).unwrap().kind else {
        panic!()
    };
    assert_eq!(i.overrides.len(), 2);

    let clear = NodePatch::from_json(json!({ "overrides": { B_LABEL: null } })).unwrap();
    let cleared = round_trip(
        &after,
        Command::SetProps {
            id: id(INST),
            patch: clear,
        },
    );
    let NodeKind::Instance(i) = &cleared.node(&id(INST)).unwrap().kind else {
        panic!()
    };
    assert_eq!(i.overrides.keys().collect::<Vec<_>>(), vec![B_BG]);
}

#[test]
fn set_props_on_a_derived_node_sets_an_override() {
    let d = sample();
    let label = Id::derived(&id(INST), &id(B_LABEL));
    let after = round_trip(
        &d,
        Command::SetProps {
            id: label.clone(),
            patch: NodePatch::from_json(json!({ "content": "Go" })).unwrap(),
        },
    );
    let NodeKind::Instance(i) = &after.node(&id(INST)).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        i.overrides.get(B_LABEL),
        Some(&Override::Text {
            content: "Go".into()
        })
    );
    let shown = shown_node(&after, &label).unwrap();
    assert!(matches!(&shown.kind, NodeKind::Text(t) if t.content == "Go"));

    // Fills and visibility, one per node.
    let bg = Id::derived(&id(INST), &id(B_BG));
    round_trip(
        &d,
        Command::SetProps {
            id: bg.clone(),
            patch: NodePatch {
                fills: Some(red()),
                ..NodePatch::default()
            },
        },
    );
    round_trip(
        &d,
        Command::SetProps {
            id: bg.clone(),
            patch: NodePatch::from_json(json!({ "visible": false })).unwrap(),
        },
    );
    // A node inside a nested instance: the key is relative to the outer one.
    let nested_label = Id::from_raw(format!("{CARD_INST}/{C_BUTTON}/{B_LABEL}"));
    let after = round_trip(
        &d,
        Command::SetProps {
            id: nested_label.clone(),
            patch: NodePatch::from_json(json!({ "content": "Deep" })).unwrap(),
        },
    );
    let NodeKind::Instance(i) = &after.node(&id(CARD_INST)).unwrap().kind else {
        panic!()
    };
    assert!(i.overrides.contains_key(&format!("{C_BUTTON}/{B_LABEL}")));
    let shown = shown_node(&after, &nested_label).unwrap();
    assert!(matches!(&shown.kind, NodeKind::Text(t) if t.content == "Deep"));
    // Swapping a nested instance.
    let nested = Id::from_raw(format!("{CARD_INST}/{C_BUTTON}"));
    let after = round_trip(
        &d,
        Command::SetProps {
            id: nested.clone(),
            patch: NodePatch::from_json(json!({ "component": ICON })).unwrap(),
        },
    );
    let shown = shown_node(&after, &nested).unwrap();
    assert!(matches!(&shown.kind, NodeKind::Instance(i) if i.component == id(ICON)));
}

#[test]
fn derived_nodes_are_read_only_otherwise() {
    let d = sample();
    let label = Id::derived(&id(INST), &id(B_LABEL));
    for patch in [
        json!({ "x": 5 }),
        json!({ "name": "x" }),
        json!({ "content": "a", "visible": false }),
    ] {
        let e = apply_err(
            &d,
            Command::SetProps {
                id: label.clone(),
                patch: NodePatch::from_json(patch).unwrap(),
            },
        );
        assert!(matches!(e, CommandError::InvalidPatch(_)), "{e:?}");
    }
    // `content` does not apply to a rect; `component` only to instances.
    let bg = Id::derived(&id(INST), &id(B_BG));
    for patch in [json!({ "content": "a" }), json!({ "component": ICON })] {
        let e = apply_err(
            &d,
            Command::SetProps {
                id: bg.clone(),
                patch: NodePatch::from_json(patch).unwrap(),
            },
        );
        assert!(matches!(e, CommandError::InvalidPatch(_)), "{e:?}");
    }
    for c in [
        Command::DeleteNode { id: label.clone() },
        Command::SetGeometry {
            id: label.clone(),
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            rotation: 0.0,
        },
        Command::SetProps {
            id: Id::derived(&id(INST), &id("n_0000000none")),
            patch: NodePatch::from_json(json!({ "content": "a" })).unwrap(),
        },
    ] {
        assert!(matches!(apply_err(&d, c), CommandError::NotFound(_)));
    }
}

#[test]
fn swaps_that_make_a_cycle_are_rejected() {
    let d = sample();
    // Inside the Button master, an Icon instance swapped to Card (which
    // contains a Button) would be a cycle.
    let mut d2 = d.clone();
    Command::UpsertComponent {
        component: {
            let mut b = d.components[0].clone();
            b.root
                .children_mut()
                .unwrap()
                .push(instance("n_00000000b9", ICON));
            b
        },
    }
    .apply(&mut d2)
    .unwrap();
    let e = apply_err(
        &d2,
        Command::SetProps {
            id: id("n_00000000b9"),
            patch: NodePatch::from_json(json!({ "component": CARD })).unwrap(),
        },
    );
    assert!(e.to_string().contains("instance of itself"), "{e}");
    // A swap to a missing component is a dangling reference.
    let e = apply_err(
        &d,
        Command::SetProps {
            id: id(INST),
            patch: NodePatch::from_json(json!({
                "overrides": { B_BG: { "type": "swap", "component": "c_000000none" } }
            }))
            .unwrap(),
        },
    );
    assert!(e.to_string().contains("unknown component"), "{e}");
}

// --- Expansion ------------------------------------------------------------

#[test]
fn expansion_derives_ids_and_applies_every_override_kind() {
    let mut d = sample();
    if let NodeKind::Instance(i) = &mut d.node_mut(&id(INST)).unwrap().kind {
        i.overrides.insert(
            B_LABEL.into(),
            Override::Text {
                content: "OK".into(),
            },
        );
        i.overrides
            .insert(B_BG.into(), Override::Fills { fills: red() });
        i.overrides
            .insert(B_ROOT.into(), Override::Visible { visible: false });
    }
    let inst = d.node(&id(INST)).unwrap().clone();
    let e = expand_instance(&d, &inst).unwrap();
    assert_eq!(e.id, id(INST));
    assert!(matches!(e.kind, NodeKind::Frame(_)));
    assert!(!e.visible, "visible override on the root");
    assert_eq!(
        children_of(&e),
        vec![format!("{INST}/{B_BG}"), format!("{INST}/{B_LABEL}")]
    );
    let kids = e.children().unwrap();
    assert_eq!(kids[0].shape().unwrap().fills, red());
    assert!(matches!(&kids[1].kind, NodeKind::Text(t) if t.content == "OK"));
    // Overrides that do not fit the node's kind are ignored.
    let mut d2 = d.clone();
    if let NodeKind::Instance(i) = &mut d2.node_mut(&id(INST)).unwrap().kind {
        i.overrides.insert(
            B_BG.into(),
            Override::Text {
                content: "x".into(),
            },
        );
        i.overrides
            .insert("n_0000000none".into(), Override::Visible { visible: false });
    }
    let e2 = expand_instance(&d2, d2.node(&id(INST)).unwrap()).unwrap();
    assert_eq!(
        e2.children().unwrap()[0].kind,
        d.components[0].root.children().unwrap()[0].kind
    );
    // Not an instance, or a missing component: nothing.
    assert!(expand_instance(&d, d.node(&id(PLAIN)).unwrap()).is_none());
    let mut dangling = inst.clone();
    if let NodeKind::Instance(i) = &mut dangling.kind {
        i.component = id("c_000000none");
    }
    assert!(expand_instance(&d, &dangling).is_none());
}

#[test]
fn nested_instances_expand_level_by_level_with_swaps() {
    let mut d = sample();
    if let NodeKind::Instance(i) = &mut d.node_mut(&id(CARD_INST)).unwrap().kind {
        i.overrides.insert(
            format!("{C_BUTTON}/{B_BG}"),
            Override::Visible { visible: false },
        );
    }
    let card = d.node(&id(CARD_INST)).unwrap().clone();
    let e = expand_instance(&d, &card).unwrap();
    let nested = &e.children().unwrap()[1];
    assert_eq!(nested.id.as_str(), format!("{CARD_INST}/{C_BUTTON}"));
    let NodeKind::Instance(ni) = &nested.kind else {
        panic!()
    };
    // The master's own override plus the outer one, re-keyed.
    assert_eq!(ni.overrides.len(), 2);
    let inner = expand_instance(&d, nested).unwrap();
    assert_eq!(
        children_of(&inner),
        vec![
            format!("{CARD_INST}/{C_BUTTON}/{B_BG}"),
            format!("{CARD_INST}/{C_BUTTON}/{B_LABEL}")
        ]
    );
    assert!(!inner.children().unwrap()[0].visible);
    assert!(
        matches!(&inner.children().unwrap()[1].kind, NodeKind::Text(t) if t.content == "Card button")
    );
    // Deep expansion replaces every instance by a frame.
    let deep = expand_deep(&d, &card);
    let mut kinds = Vec::new();
    deep.walk(&mut |n| kinds.push(n.kind.tag()));
    assert_eq!(kinds, vec!["frame", "text", "frame", "rect", "text"]);
    // A swap replaces the nested component.
    if let NodeKind::Instance(i) = &mut d.node_mut(&id(CARD_INST)).unwrap().kind {
        i.overrides.insert(
            C_BUTTON.into(),
            Override::Swap {
                component: id(ICON),
            },
        );
    }
    let e = expand_instance(&d, d.node(&id(CARD_INST)).unwrap()).unwrap();
    let inner = expand_instance(&d, &e.children().unwrap()[1]).unwrap();
    assert_eq!(
        children_of(&inner),
        vec![format!("{CARD_INST}/{C_BUTTON}/{I_SHAPE}")]
    );
}

#[test]
fn instances_use_their_own_size_and_children_follow_constraints() {
    let mut d = sample();
    {
        let b = &mut d.components[0].root;
        let kids = b.children_mut().unwrap();
        kids[0].constraints.horizontal = HConstraint::LeftRight;
        kids[1].constraints.horizontal = HConstraint::Right;
        kids[1].constraints.vertical = VConstraint::Center;
    }
    let inst = d.node_mut(&id(INST)).unwrap();
    inst.width = 160.0;
    inst.height = 60.0;
    let inst = inst.clone();
    let e = expand_instance(&d, &inst).unwrap();
    assert_eq!((e.width, e.height, e.x, e.y), (160.0, 60.0, 10.0, 10.0));
    let k = e.children().unwrap();
    assert_eq!((k[0].x, k[0].width), (0.0, 80.0));
    assert_eq!((k[1].x, k[1].y, k[1].width), (70.0, 20.0, 50.0));
}

#[test]
fn derived_ids_resolve_to_their_expanded_node() {
    let d = sample();
    let r = resolve_derived(
        &d,
        &Id::from_raw(format!("{CARD_INST}/{C_BUTTON}/{B_LABEL}")),
    )
    .unwrap();
    assert_eq!(r.instance, id(CARD_INST));
    assert_eq!(r.key, format!("{C_BUTTON}/{B_LABEL}"));
    assert!(matches!(&r.node.kind, NodeKind::Text(t) if t.content == "Card button"));
    // Inside a frame inside the master.
    let r = resolve_derived(&d, &Id::derived(&id(CARD_INST), &id(C_TITLE))).unwrap();
    assert_eq!(r.node.name, "Label");
    // Instances inside masters resolve too.
    let r = resolve_derived(&d, &Id::derived(&id(C_BUTTON), &id(B_LABEL))).unwrap();
    assert_eq!(r.instance, id(C_BUTTON));
    for bad in [
        INST.to_string(),
        format!("{INST}/n_0000000none"),
        format!("{PLAIN}/{B_LABEL}"),
        format!("{INST}/{B_LABEL}/{B_LABEL}"),
        format!("n_0000000none/{B_LABEL}"),
    ] {
        assert!(
            resolve_derived(&d, &Id::from_raw(bad.clone())).is_none(),
            "{bad}"
        );
    }
    assert_eq!(shown_node(&d, &id(PLAIN)).unwrap().id, id(PLAIN));
}

#[test]
fn cyclic_components_from_a_file_do_not_expand_forever() {
    let mut d = sample();
    // Hand-edited: Icon contains an Icon.
    d.components[1]
        .root
        .children_mut()
        .unwrap()
        .push(instance("n_00000000a9", ICON));
    d.pages[0].children.push(instance("n_00000000p9", ICON));
    d.rebuild_index();
    assert!(!validate(&d).is_empty());
    let deep = expand_deep(&d, d.node(&id("n_00000000p9")).unwrap());
    let mut count = 0;
    deep.walk(&mut |_| count += 1);
    assert!(count < 200, "{count}");
}

/// A page with one instance of `c0`, a chain of `levels` components where
/// each holds `fan` instances of the next, and a rect at the bottom.
fn fan_out_bomb(levels: usize, fan: usize) -> Document {
    let cid = |i: usize| format!("c_{i:010}");
    let mut page = Page::new(id(PAGE), "Page 1");
    page.children = vec![instance("n_00000000p9", &cid(0))];
    let mut d = Document::new("Bomb", page);
    for l in 0..levels {
        let kids = if l + 1 == levels {
            vec![rect("n_000000r000", 0.0, 0.0)]
        } else {
            (0..fan)
                .map(|k| instance(&format!("n_{:03}{:07}", l, k), &cid(l + 1)))
                .collect()
        };
        d.components.push(component(
            &cid(l),
            "C",
            frame(&format!("n_{l:03}r000000"), kids),
        ));
    }
    d.rebuild_index();
    d
}

#[test]
fn fan_out_bomb_is_reported_and_expands_within_budget() {
    let d = fan_out_bomb(20, 10);
    // Through the loader, as an opened file.
    let loaded = load_str(&save_string(&d)).unwrap();
    assert!(
        loaded
            .problems
            .iter()
            .any(|p| p.message.contains("expands to more than")),
        "{:?}",
        loaded.problems
    );
    let t = std::time::Instant::now();
    let inst = d.node(&id("n_00000000p9")).unwrap();
    // Over budget: left unexpanded, however it is asked.
    assert!(expand_instance(&d, inst).is_none());
    assert_eq!(expand_deep(&d, inst), *inst);
    // As the master of the first component is opened.
    let _ = expand_deep(&d, &d.components[0].root);
    assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
}

#[test]
fn big_but_in_budget_documents_still_expand() {
    // 10^3 + 10^2 + 10 + 1 + root: a few thousand nodes.
    let d = fan_out_bomb(4, 10);
    assert!(validate(&d).is_empty(), "{:?}", validate(&d));
    let deep = expand_deep(&d, d.node(&id("n_00000000p9")).unwrap());
    let mut count = 0;
    deep.walk(&mut |_| count += 1);
    assert_eq!(count, 1 + 10 + 100 + 1000 + 1000);
}
