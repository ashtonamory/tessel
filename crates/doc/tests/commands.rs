//! Unit tests per command (§5.1–5.2) and for the history (§5.3).

use doc::*;
use serde_json::json;

fn id(s: &str) -> Id {
    Id::from_raw(s)
}

fn rect(i: &str) -> Node {
    Node::new(id(i), "Rect", NodeKind::Rect(Rect::default())).with_geometry(1.0, 2.0, 30.0, 40.0)
}

fn text(i: &str, content: &str) -> Node {
    Node::new(
        id(i),
        "Text",
        NodeKind::Text(Text {
            content: content.into(),
            ..Text::default()
        }),
    )
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
    .with_geometry(0.0, 0.0, 400.0, 300.0)
}

fn group(i: &str, children: Vec<Node>) -> Node {
    Node::new(
        id(i),
        "Group",
        NodeKind::Group(Group {
            children,
            ..Group::default()
        }),
    )
}

const PAGE: &str = "p_0000000001";
const FRAME: &str = "n_00000000f1";
const RECT: &str = "n_00000000r1";
const TEXT: &str = "n_00000000t1";
const GROUP: &str = "n_00000000g1";
const G1: &str = "n_00000000g2";
const G2: &str = "n_00000000g3";

/// page
/// ├── frame  [rect, text]
/// └── group  [g1, g2]
fn sample() -> Document {
    let mut page = Page::new(id(PAGE), "Page 1");
    page.children = vec![
        frame(FRAME, vec![rect(RECT), text(TEXT, "Hello")]),
        group(GROUP, vec![rect(G1), rect(G2)]),
    ];
    let mut d = Document::new("Test", page);
    d.rebuild_index();
    assert!(validate(&d).is_empty(), "{:?}", validate(&d));
    d
}

fn page() -> ParentRef {
    ParentRef::Page(id(PAGE))
}

fn node_parent(i: &str) -> ParentRef {
    ParentRef::Node(id(i))
}

fn child_ids(d: &Document, parent: &ParentRef) -> Vec<String> {
    d.children_of(parent)
        .unwrap()
        .iter()
        .map(|n| n.id.to_string())
        .collect()
}

/// Apply, check the inverse restores the original exactly, and return the
/// document after the command plus the inverse.
fn apply_and_check_inverse(d: &Document, c: Command) -> (Document, Command) {
    let mut after = d.clone();
    let inverse = c.apply(&mut after).expect("command applies");
    let mut back = after.clone();
    inverse.clone().apply(&mut back).expect("inverse applies");
    assert_eq!(&back, d, "inverse did not restore the document");
    assert_eq!(save_string(&back), save_string(d));
    (after, inverse)
}

/// Apply a command that must fail, and check the document is unchanged.
fn apply_err(d: &Document, c: Command) -> CommandError {
    let mut after = d.clone();
    let e = c.apply(&mut after).expect_err("command should fail");
    assert_eq!(&after, d, "failed command changed the document");
    e
}

// --- CreateNode ------------------------------------------------------------

#[test]
fn create_node_appends_or_inserts() {
    let d = sample();
    let (after, inv) = apply_and_check_inverse(
        &d,
        Command::CreateNode {
            parent: page(),
            index: None,
            node: rect("n_0000000new"),
        },
    );
    assert_eq!(
        inv,
        Command::DeleteNode {
            id: id("n_0000000new")
        }
    );
    assert_eq!(child_ids(&after, &page()), [FRAME, GROUP, "n_0000000new"]);

    let (after, _) = apply_and_check_inverse(
        &d,
        Command::CreateNode {
            parent: node_parent(FRAME),
            index: Some(1),
            node: frame("n_0000000new", vec![rect("n_000000new2")]),
        },
    );
    assert_eq!(
        child_ids(&after, &node_parent(FRAME)),
        [RECT, "n_0000000new", TEXT]
    );
    // The subtree is indexed.
    assert!(after.node(&id("n_000000new2")).is_some());
    assert_eq!(
        after.parent_of(&id("n_000000new2")),
        Some((node_parent("n_0000000new"), 0))
    );
}

#[test]
fn create_node_errors() {
    let d = sample();
    let c = |parent, index, node| Command::CreateNode {
        parent,
        index,
        node,
    };
    assert!(matches!(
        apply_err(
            &d,
            c(node_parent("n_000000none"), None, rect("n_0000000new"))
        ),
        CommandError::NotFound(_)
    ));
    assert!(matches!(
        apply_err(&d, c(node_parent(RECT), None, rect("n_0000000new"))),
        CommandError::NotAContainer(_)
    ));
    assert!(matches!(
        apply_err(&d, c(page(), Some(3), rect("n_0000000new"))),
        CommandError::IndexOutOfRange { index: 3, len: 2 }
    ));
    assert!(matches!(
        apply_err(&d, c(page(), None, rect(RECT))),
        CommandError::DuplicateId(_)
    ));
    assert!(matches!(
        apply_err(
            &d,
            c(
                page(),
                None,
                frame("n_0000000new", vec![rect("n_0000000new")])
            )
        ),
        CommandError::DuplicateId(_)
    ));
    assert!(matches!(
        apply_err(&d, c(page(), None, rect(PAGE))),
        CommandError::DuplicateId(_)
    ));
    let mut bad = rect("n_0000000new");
    bad.opacity = 2.0;
    assert!(matches!(
        apply_err(&d, c(page(), None, bad)),
        CommandError::Invariant(_)
    ));
    assert!(matches!(
        apply_err(&d, c(page(), None, rect("not-an-id"))),
        CommandError::Invariant(_)
    ));
    assert!(matches!(
        apply_err(&d, c(page(), None, group("n_0000000new", vec![]))),
        CommandError::Invariant(_)
    ));
}

// --- DeleteNode ------------------------------------------------------------

#[test]
fn delete_node_returns_the_subtree_at_its_index() {
    let d = sample();
    let (after, inv) = apply_and_check_inverse(&d, Command::DeleteNode { id: id(FRAME) });
    assert_eq!(child_ids(&after, &page()), [GROUP]);
    assert!(
        after.node(&id(RECT)).is_none(),
        "descendants are gone from the index"
    );
    let Command::CreateNode {
        parent,
        index,
        node,
    } = inv
    else {
        panic!("inverse of delete is create")
    };
    assert_eq!((parent, index), (page(), Some(0)));
    assert_eq!(node.children().unwrap().len(), 2);
}

#[test]
fn delete_node_errors() {
    let d = sample();
    assert!(matches!(
        apply_err(
            &d,
            Command::DeleteNode {
                id: id("n_000000none")
            }
        ),
        CommandError::NotFound(_)
    ));
    // Deleting one child of a two-child group is fine; the last one is not.
    let (after, _) = apply_and_check_inverse(&d, Command::DeleteNode { id: id(G1) });
    assert!(matches!(
        apply_err(&after, Command::DeleteNode { id: id(G2) }),
        CommandError::Invariant(_)
    ));
}

// --- MoveNode --------------------------------------------------------------

#[test]
fn move_node_reorders_and_reparents() {
    let d = sample();
    let mv = |i: &str, parent, index| Command::MoveNode {
        id: id(i),
        parent,
        index,
    };
    // Reorder within the same parent: index is after removal.
    let (after, inv) = apply_and_check_inverse(&d, mv(RECT, node_parent(FRAME), Some(1)));
    assert_eq!(child_ids(&after, &node_parent(FRAME)), [TEXT, RECT]);
    assert_eq!(inv, mv(RECT, node_parent(FRAME), Some(0)));
    let (after, _) = apply_and_check_inverse(&d, mv(RECT, node_parent(FRAME), None));
    assert_eq!(child_ids(&after, &node_parent(FRAME)), [TEXT, RECT]);

    // Reparent to the page and into a group.
    let (after, _) = apply_and_check_inverse(&d, mv(TEXT, page(), Some(0)));
    assert_eq!(child_ids(&after, &page()), [TEXT, FRAME, GROUP]);
    assert_eq!(after.parent_of(&id(RECT)), Some((node_parent(FRAME), 0)));
    let (after, _) = apply_and_check_inverse(&d, mv(FRAME, node_parent(GROUP), Some(1)));
    assert_eq!(child_ids(&after, &node_parent(GROUP)), [G1, FRAME, G2]);
    assert!(after.node(&id(TEXT)).is_some());
}

#[test]
fn move_node_errors() {
    let d = sample();
    let mv = |i: &str, parent, index| Command::MoveNode {
        id: id(i),
        parent,
        index,
    };
    assert!(matches!(
        apply_err(&d, mv(FRAME, node_parent(FRAME), None)),
        CommandError::Cycle(_)
    ));
    let d2 = {
        let mut d2 = d.clone();
        Command::MoveNode {
            id: id(GROUP),
            parent: node_parent(FRAME),
            index: None,
        }
        .apply(&mut d2)
        .unwrap();
        d2
    };
    assert!(matches!(
        apply_err(&d2, mv(FRAME, node_parent(GROUP), None)),
        CommandError::Cycle(_)
    ));
    assert!(matches!(
        apply_err(&d, mv(RECT, node_parent(FRAME), Some(2))),
        CommandError::IndexOutOfRange { index: 2, len: 1 }
    ));
    assert!(matches!(
        apply_err(&d, mv(RECT, node_parent(TEXT), None)),
        CommandError::NotAContainer(_)
    ));
    assert!(matches!(
        apply_err(&d, mv("n_000000none", page(), None)),
        CommandError::NotFound(_)
    ));
    // Moving the last child out of a group would leave it empty.
    let (one_left, _) = apply_and_check_inverse(&d, mv(G1, page(), None));
    assert!(matches!(
        apply_err(&one_left, mv(G2, page(), None)),
        CommandError::Invariant(_)
    ));
    // Reordering within that group is fine.
    apply_and_check_inverse(&one_left, mv(G2, node_parent(GROUP), Some(0)));
}

// --- SetProps --------------------------------------------------------------

#[test]
fn set_props_inverse_holds_exactly_the_patched_fields() {
    let d = sample();
    let patch = NodePatch::from_json(json!({
        "name": "Renamed",
        "opacity": 0.5,
        "fills": [{ "type": "solid", "color": "#ff0000" }],
        "corner_radius": [1, 2, 3, 4],
    }))
    .unwrap();
    let (after, inv) = apply_and_check_inverse(
        &d,
        Command::SetProps {
            id: id(RECT),
            patch,
        },
    );
    let n = after.node(&id(RECT)).unwrap();
    assert_eq!(n.name, "Renamed");
    assert_eq!(n.opacity, 0.5);
    assert_eq!(
        n.shape().unwrap().corner_radius,
        CornerRadius::Each([1.0, 2.0, 3.0, 4.0].map(RadiusRef::Px))
    );
    assert_eq!(
        inv,
        Command::SetProps {
            id: id(RECT),
            patch: NodePatch {
                name: Some("Rect".into()),
                opacity: Some(1.0),
                fills: Some(vec![]),
                corner_radius: Some(CornerRadius::px(0.0)),
                ..NodePatch::default()
            }
        }
    );
}

#[test]
fn set_props_text_and_nullable_fields() {
    let mut d = sample();
    d.styles.push(Style {
        id: id("s_0000000001"),
        name: "Body".into(),
        kind: StyleKind::Text(TextStyleDef::default()),
    });
    let set = |patch: serde_json::Value| Command::SetProps {
        id: id(TEXT),
        patch: NodePatch::from_json(patch).unwrap(),
    };
    let (after, _) = apply_and_check_inverse(
        &d,
        set(json!({ "content": "Bye", "text_style": "s_0000000001", "semantic": "h1" })),
    );
    let NodeKind::Text(t) = &after.node(&id(TEXT)).unwrap().kind else {
        panic!()
    };
    assert_eq!(t.content, "Bye");
    assert_eq!(t.text_style, Some(id("s_0000000001")));
    // `null` clears an optional prop; its inverse restores the old value.
    let (cleared, inv) = apply_and_check_inverse(&after, set(json!({ "text_style": null })));
    let NodeKind::Text(t) = &cleared.node(&id(TEXT)).unwrap().kind else {
        panic!()
    };
    assert_eq!(t.text_style, None);
    let Command::SetProps { patch, .. } = inv else {
        panic!()
    };
    assert_eq!(patch.text_style, Some(Some(id("s_0000000001"))));
}

#[test]
fn set_props_rejects_unknown_and_inapplicable_fields() {
    let e = NodePatch::from_json(json!({ "nmae": "typo" })).unwrap_err();
    assert!(
        matches!(&e, CommandError::InvalidPatch(m) if m.contains("nmae")),
        "{e}"
    );
    assert!(NodePatch::from_json(json!({ "children": [] })).is_err());
    assert!(NodePatch::from_json(json!({ "id": "n_0000000000" })).is_err());

    let d = sample();
    let e = apply_err(
        &d,
        Command::SetProps {
            id: id(RECT),
            patch: NodePatch::from_json(json!({ "name": "ok", "content": "text only" })).unwrap(),
        },
    );
    assert!(
        matches!(&e, CommandError::InvalidPatch(m) if m.contains("content")),
        "{e}"
    );
    assert!(matches!(
        apply_err(
            &d,
            Command::SetProps {
                id: id(GROUP),
                patch: NodePatch::from_json(json!({ "fills": [] })).unwrap(),
            },
        ),
        CommandError::InvalidPatch(_)
    ));
}

#[test]
fn set_props_rolls_back_invariant_violations() {
    let d = sample();
    for patch in [
        json!({ "name": "x", "opacity": 3 }),
        json!({ "width": -5 }),
        json!({ "fill_style": "s_0000000009" }),
        json!({ "fills": [{ "type": "solid", "color": "{color.missing}" }] }),
        json!({ "stroke_width": -1 }),
    ] {
        let e = apply_err(
            &d,
            Command::SetProps {
                id: id(RECT),
                patch: NodePatch::from_json(patch.clone()).unwrap(),
            },
        );
        assert!(matches!(e, CommandError::Invariant(_)), "{patch}: {e}");
    }
    assert!(matches!(
        apply_err(
            &d,
            Command::SetProps {
                id: id("n_000000none"),
                patch: NodePatch::default(),
            },
        ),
        CommandError::NotFound(_)
    ));
}

#[test]
fn set_props_allows_edits_next_to_existing_problems() {
    // A problem that was already there does not block unrelated edits.
    let mut d = sample();
    d.node_mut(&id(RECT)).unwrap().opacity = 7.0;
    let (after, _) = apply_and_check_inverse(
        &d,
        Command::SetProps {
            id: id(RECT),
            patch: NodePatch::from_json(json!({ "name": "still broken" })).unwrap(),
        },
    );
    assert_eq!(after.node(&id(RECT)).unwrap().name, "still broken");
}

// --- SetGeometry -----------------------------------------------------------

#[test]
fn set_geometry() {
    let d = sample();
    let (after, inv) = apply_and_check_inverse(
        &d,
        Command::SetGeometry {
            id: id(RECT),
            x: 5.0,
            y: 6.0,
            width: 7.0,
            height: 8.0,
            rotation: 45.0,
        },
    );
    let n = after.node(&id(RECT)).unwrap();
    assert_eq!(
        [n.x, n.y, n.width, n.height, n.rotation],
        [5.0, 6.0, 7.0, 8.0, 45.0]
    );
    assert_eq!(
        inv,
        Command::SetGeometry {
            id: id(RECT),
            x: 1.0,
            y: 2.0,
            width: 30.0,
            height: 40.0,
            rotation: 0.0
        }
    );
    for (w, r) in [(-1.0, 0.0), (f32::NAN, 0.0), (1.0, f32::INFINITY)] {
        assert!(matches!(
            apply_err(
                &d,
                Command::SetGeometry {
                    id: id(RECT),
                    x: 0.0,
                    y: 0.0,
                    width: w,
                    height: 1.0,
                    rotation: r,
                },
            ),
            CommandError::Invariant(_)
        ));
    }
}

// --- Pages -----------------------------------------------------------------

#[test]
fn page_commands() {
    let d = sample();
    let mut p = Page::new(id("p_0000000002"), "Second");
    p.children.push(rect("n_0000000new"));
    let (after, inv) = apply_and_check_inverse(
        &d,
        Command::CreatePage {
            index: Some(0),
            page: p,
        },
    );
    assert_eq!(
        inv,
        Command::DeletePage {
            id: id("p_0000000002")
        }
    );
    assert_eq!(after.pages[0].name, "Second");
    assert_eq!(after.path_of(&id(RECT)).unwrap().page, 1);

    apply_and_check_inverse(&after, Command::DeletePage { id: id(PAGE) });
    let (renamed, _) = apply_and_check_inverse(
        &d,
        Command::UpdatePage {
            id: id(PAGE),
            name: Some("Renamed".into()),
            background: Some(Color::new("#000000")),
        },
    );
    assert_eq!(renamed.pages[0].name, "Renamed");

    assert!(matches!(
        apply_err(
            &d,
            Command::CreatePage {
                index: None,
                page: Page::new(id(PAGE), "dup"),
            }
        ),
        CommandError::DuplicateId(_)
    ));
    let mut dup_node = Page::new(id("p_0000000002"), "dup node");
    dup_node.children.push(rect(RECT));
    assert!(matches!(
        apply_err(
            &d,
            Command::CreatePage {
                index: None,
                page: dup_node,
            }
        ),
        CommandError::DuplicateId(_)
    ));
    assert!(matches!(
        apply_err(
            &d,
            Command::UpdatePage {
                id: id(PAGE),
                name: None,
                background: Some(Color::new("red")),
            }
        ),
        CommandError::Invariant(_)
    ));
}

// --- Batch -----------------------------------------------------------------

#[test]
fn batch_applies_in_order_and_inverts_in_reverse() {
    let d = sample();
    let (after, inv) = apply_and_check_inverse(
        &d,
        Command::Batch(vec![
            Command::CreateNode {
                parent: page(),
                index: None,
                node: frame("n_0000000new", vec![]),
            },
            Command::MoveNode {
                id: id(RECT),
                parent: node_parent("n_0000000new"),
                index: None,
            },
            Command::SetProps {
                id: id(RECT),
                patch: NodePatch::from_json(json!({ "name": "Moved" })).unwrap(),
            },
        ]),
    );
    assert_eq!(
        after.parent_of(&id(RECT)),
        Some((node_parent("n_0000000new"), 0))
    );
    let Command::Batch(inv) = inv else { panic!() };
    assert!(matches!(inv[0], Command::SetProps { .. }));
    assert!(matches!(inv[1], Command::MoveNode { .. }));
    assert!(matches!(inv[2], Command::DeleteNode { .. }));
}

#[test]
fn batch_is_atomic() {
    let d = sample();
    let e = apply_err(
        &d,
        Command::Batch(vec![
            Command::DeleteNode { id: id(TEXT) },
            Command::SetProps {
                id: id(RECT),
                patch: NodePatch::from_json(json!({ "name": "changed" })).unwrap(),
            },
            Command::Batch(vec![
                Command::CreateNode {
                    parent: page(),
                    index: None,
                    node: rect("n_0000000new"),
                },
                Command::DeleteNode {
                    id: id("n_000000none"),
                },
            ]),
        ]),
    );
    assert!(matches!(e, CommandError::NotFound(_)));
    assert!(d.node(&id(TEXT)).is_some());
    assert_eq!(d.node(&id(RECT)).unwrap().name, "Rect");
    assert!(d.node(&id("n_0000000new")).is_none());
}

// --- Regressions -----------------------------------------------------------

fn doc_with_component() -> Document {
    let mut d = sample();
    d.components.push(Component {
        id: Id::from_raw("c_0000000001"),
        name: "Button".into(),
        props: vec![],
        extra: Extra::default(),
        root: frame("n_00000000m1", vec![rect("n_00000000m2")]),
    });
    d.rebuild_index();
    assert!(validate(&d).is_empty(), "{:?}", validate(&d));
    d
}

#[test]
fn new_ids_may_not_reuse_component_master_ids() {
    let d = doc_with_component();
    for reused in ["n_00000000m1", "n_00000000m2"] {
        let e = apply_err(
            &d,
            Command::CreateNode {
                parent: page(),
                index: None,
                node: rect(reused),
            },
        );
        assert!(matches!(e, CommandError::DuplicateId(_)), "{e:?}");
    }
    let mut p = Page::new(id("p_0000000002"), "Two");
    p.children = vec![rect("n_00000000m2")];
    let e = apply_err(
        &d,
        Command::CreatePage {
            index: None,
            page: p,
        },
    );
    assert!(matches!(e, CommandError::DuplicateId(_)), "{e:?}");
}

#[test]
fn empty_group_from_a_hand_edited_file_can_be_undone() {
    let mut d = sample();
    d.pages[0].children.push(group("n_00000000g9", vec![]));
    d.rebuild_index();
    let loaded = load_str(&save_string(&d)).expect("loads");
    let mut d = loaded.doc;
    let before = d.clone();

    let mut history = History::default();
    history
        .commit(
            &mut d,
            "add",
            Origin::User,
            vec![Command::CreateNode {
                parent: node_parent("n_00000000g9"),
                index: None,
                node: rect("n_0000000new"),
            }],
            Selection::default(),
            Selection::default(),
        )
        .expect("create into the empty group");
    history.undo(&mut d).expect("undo succeeds");
    assert_eq!(d, before);

    // The same must hold for a batch that rolls back after such an undo.
    let mut after = before.clone();
    let e = Command::Batch(vec![
        Command::CreateNode {
            parent: node_parent("n_00000000g9"),
            index: None,
            node: rect("n_0000000new"),
        },
        Command::DeleteNode {
            id: id("n_000000none"),
        },
    ])
    .apply(&mut after)
    .expect_err("batch fails");
    assert!(matches!(e, CommandError::NotFound(_)));
    assert_eq!(after, before);
}

// --- History ---------------------------------------------------------------

fn rename(i: &str, name: &str) -> Command {
    Command::SetProps {
        id: id(i),
        patch: NodePatch {
            name: Some(name.into()),
            ..NodePatch::default()
        },
    }
}

#[test]
fn history_undo_redo() {
    let original = sample();
    let mut d = original.clone();
    let mut h = History::default();
    assert!(!h.can_undo() && !h.can_redo());
    assert!(h.undo(&mut d).unwrap().is_none());

    let sel = Selection {
        ids: vec![id(RECT)],
    };
    h.commit(
        &mut d,
        "Rename",
        Origin::User,
        vec![rename(RECT, "A"), rename(TEXT, "B")],
        Selection::default(),
        sel.clone(),
    )
    .unwrap();
    let after_first = d.clone();
    h.commit(
        &mut d,
        "Agent: delete text",
        Origin::Agent {
            client: "claude-code".into(),
        },
        vec![Command::DeleteNode { id: id(TEXT) }],
        sel.clone(),
        Selection::default(),
    )
    .unwrap();
    let after_second = d.clone();
    assert_eq!(h.undo_label(), Some("Agent: delete text"));

    let tx = h.undo(&mut d).unwrap().unwrap();
    assert_eq!(
        tx.origin,
        Origin::Agent {
            client: "claude-code".into()
        }
    );
    assert_eq!(tx.selection_before, sel);
    assert_eq!(d, after_first);
    assert_eq!(h.redo_label(), Some("Agent: delete text"));
    h.undo(&mut d).unwrap();
    assert_eq!(d, original);
    assert_eq!(save_string(&d), save_string(&original));
    assert!(!h.can_undo());

    h.redo(&mut d).unwrap();
    assert_eq!(d, after_first);
    h.redo(&mut d).unwrap();
    assert_eq!(d, after_second);
    assert!(!h.can_redo());

    // Undo twice, redo once, then a new transaction clears the redo stack.
    h.undo(&mut d).unwrap();
    h.undo(&mut d).unwrap();
    h.redo(&mut d).unwrap();
    h.commit(
        &mut d,
        "Rename again",
        Origin::User,
        vec![rename(RECT, "C")],
        Selection::default(),
        Selection::default(),
    )
    .unwrap();
    assert!(!h.can_redo());
    assert_eq!(h.transactions().len(), 2);
    h.undo(&mut d).unwrap();
    assert_eq!(d, after_first);
}

#[test]
fn history_failed_commit_changes_nothing() {
    let original = sample();
    let mut d = original.clone();
    let mut h = History::default();
    let e = h
        .commit(
            &mut d,
            "Bad",
            Origin::User,
            vec![
                rename(RECT, "A"),
                Command::DeleteNode {
                    id: id("n_000000none"),
                },
            ],
            Selection::default(),
            Selection::default(),
        )
        .unwrap_err();
    assert!(matches!(e, CommandError::NotFound(_)));
    assert_eq!(d, original);
    assert!(!h.can_undo());
}

#[test]
fn history_limit_drops_oldest() {
    let mut d = sample();
    let mut h = History::with_limit(3);
    for i in 0..5 {
        h.commit(
            &mut d,
            format!("step {i}"),
            Origin::User,
            vec![rename(RECT, &i.to_string())],
            Selection::default(),
            Selection::default(),
        )
        .unwrap();
    }
    let labels: Vec<_> = h.transactions().iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, ["step 2", "step 3", "step 4"]);
    while h.undo(&mut d).unwrap().is_some() {}
    assert_eq!(d.node(&id(RECT)).unwrap().name, "1");
}

// --- touched_ids (§5.5) ----------------------------------------------------

fn ids(list: &[&str]) -> std::collections::BTreeSet<NodeId> {
    list.iter().map(|s| id(s)).collect()
}

#[test]
fn touched_ids_per_command() {
    let create = Command::CreateNode {
        parent: node_parent(FRAME),
        index: None,
        node: frame("n_0000000new", vec![rect("n_000000new2")]),
    };
    assert_eq!(
        create.touched_ids(),
        ids(&[FRAME, "n_0000000new", "n_000000new2"])
    );
    // Page-level parents are not node IDs.
    let create = Command::CreateNode {
        parent: page(),
        index: None,
        node: rect("n_0000000new"),
    };
    assert_eq!(create.touched_ids(), ids(&["n_0000000new"]));

    assert_eq!(
        Command::DeleteNode { id: id(RECT) }.touched_ids(),
        ids(&[RECT])
    );
    assert_eq!(
        Command::MoveNode {
            id: id(RECT),
            parent: node_parent(GROUP),
            index: None,
        }
        .touched_ids(),
        ids(&[RECT, GROUP])
    );
    assert_eq!(rename(TEXT, "x").touched_ids(), ids(&[TEXT]));
    assert!(Command::DeletePage { id: id(PAGE) }
        .touched_ids()
        .is_empty());
}

#[test]
fn touched_ids_cover_batches_and_inverses() {
    let d = sample();
    let batch = Command::Batch(vec![
        rename(RECT, "a"),
        Command::Batch(vec![Command::DeleteNode { id: id(TEXT) }]),
    ]);
    assert_eq!(batch.touched_ids(), ids(&[RECT, TEXT]));

    // The inverse of a delete names the parent too.
    let mut after = d.clone();
    let inverse = Command::DeleteNode { id: id(TEXT) }
        .apply(&mut after)
        .unwrap();
    assert_eq!(inverse.touched_ids(), ids(&[FRAME, TEXT]));
}
