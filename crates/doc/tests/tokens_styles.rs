//! Token and style commands (§3.6, §5.1–5.2) and the resolution helpers.

use doc::resolve::*;
use doc::*;

fn id(s: &str) -> Id {
    Id::from_raw(s)
}

fn tok(s: &str) -> TokenId {
    TokenId::new(s)
}

const CARD: &str = "s_00000000c1";
const OUTLINE: &str = "s_00000000a1";
const BODY: &str = "s_00000000b1";
const GLOW: &str = "s_00000000g1";
const BOX: &str = "n_00000000b1";
const LABEL: &str = "n_00000000t1";
const FRAME: &str = "n_00000000f1";
const MASTER: &str = "n_00000000m1";
const COMP: &str = "c_00000000c1";

fn token_fill(name: &str) -> Vec<Paint> {
    vec![Paint::solid(ColorRef::Token(tok(name)))]
}

fn shadow(color: ColorRef) -> Shadow {
    Shadow {
        x: 0.0,
        y: 2.0,
        blur: 4.0,
        spread: 0.0,
        color,
        extra: Extra::new(),
    }
}

/// A document where `color.primary`, `space.4` and `font.sans` are used by
/// a node, a style and a component master, and every style kind is used.
fn sample() -> Document {
    let mut d = Document::default();
    d.tokens
        .colors
        .insert(tok("color.primary"), Color::new("#2563eb"));
    d.tokens
        .colors
        .insert(tok("color.unused"), Color::new("#000000"));
    d.tokens.spacing.insert(tok("space.4"), 16.0);
    d.tokens.radii.insert(tok("radius.md"), 8.0);
    d.tokens.fonts.insert(tok("font.sans"), "Inter".into());
    d.styles = vec![
        Style {
            id: id(CARD),
            name: "Surface/Card".into(),
            kind: StyleKind::Fill(FillStyle {
                fills: token_fill("color.primary"),
                extra: Extra::new(),
            }),
        },
        Style {
            id: id(OUTLINE),
            name: "Outline".into(),
            kind: StyleKind::Stroke(StrokeStyle {
                strokes: token_fill("color.primary"),
                width: 3.0,
                extra: Extra::new(),
            }),
        },
        Style {
            id: id(BODY),
            name: "Body".into(),
            kind: StyleKind::Text(TextStyleDef {
                style: TextStyle {
                    font_family: "{font.sans}".into(),
                    font_size: 20.0,
                    ..TextStyle::default()
                },
                extra: Extra::new(),
            }),
        },
        Style {
            id: id(GLOW),
            name: "Glow".into(),
            kind: StyleKind::Effect(EffectStyle {
                shadows: vec![shadow(ColorRef::Token(tok("color.primary")))],
                extra: Extra::new(),
            }),
        },
    ];

    let mut boxed = Node::new(
        id(BOX),
        "Box",
        NodeKind::Rect(Rect {
            shape: Shape {
                fills: token_fill("color.primary"),
                fill_style: Some(id(CARD)),
                stroke_style: Some(id(OUTLINE)),
                effect_style: Some(id(GLOW)),
                corner_radius: CornerRadius::Each([
                    RadiusRef::Token(tok("radius.md")),
                    RadiusRef::Px(1.0),
                    RadiusRef::Token(tok("radius.md")),
                    RadiusRef::Px(2.0),
                ]),
                ..Shape::default()
            },
            ..Rect::default()
        }),
    )
    .with_geometry(0.0, 0.0, 50.0, 50.0);
    boxed.opacity = 0.5;
    let label = Node::new(
        id(LABEL),
        "Label",
        NodeKind::Text(Text {
            content: "Hi".into(),
            text_style: Some(id(BODY)),
            style: TextStyle {
                font_family: "{font.sans}".into(),
                font_size: 11.0,
                ..TextStyle::default()
            },
            fills: token_fill("color.primary"),
            ..Text::default()
        }),
    );
    let frame = Node::new(
        id(FRAME),
        "Frame",
        NodeKind::Frame(Frame {
            layout: Some(AutoLayout {
                direction: Direction::Row,
                wrap: false,
                gap: SpaceRef::Token(tok("space.4")),
                cross_gap: Some(SpaceRef::Token(tok("space.4"))),
                padding: [
                    SpaceRef::Token(tok("space.4")),
                    SpaceRef::Px(1.0),
                    SpaceRef::Px(2.0),
                    SpaceRef::Px(3.0),
                ],
                justify: Justify::Start,
                align: Align::Start,
                width: Sizing::Hug,
                height: Sizing::Hug,
                extra: Extra::new(),
            }),
            children: vec![boxed, label],
            ..Frame::default()
        }),
    )
    .with_geometry(0.0, 0.0, 200.0, 100.0);

    // A component master using the same tokens and styles.
    let mut master_shape = Shape {
        fills: token_fill("color.primary"),
        fill_style: Some(id(CARD)),
        corner_radius: CornerRadius::All(RadiusRef::Token(tok("radius.md"))),
        ..Shape::default()
    };
    master_shape.shadows = vec![shadow(ColorRef::Token(tok("color.primary")))];
    let master = Node::new(
        id(MASTER),
        "Master",
        NodeKind::Frame(Frame {
            shape: master_shape,
            ..Frame::default()
        }),
    )
    .with_geometry(0.0, 0.0, 10.0, 10.0);
    d.components.push(Component {
        id: id(COMP),
        name: "Chip".into(),
        props: Vec::new(),
        extra: Extra::new(),
        root: master,
    });

    let mut page = Page::new(id("p_0000000001"), "Page");
    page.children = vec![frame];
    d.pages.push(page);
    d.rebuild_index();
    assert_eq!(validate(&d), vec![]);
    d
}

fn apply_and_check_inverse(d: &Document, c: Command) -> (Document, Command) {
    let mut after = d.clone();
    let inverse = c.clone().apply(&mut after).expect("command applies");
    assert_eq!(validate(&after), vec![], "after {c:?}");
    let mut back = after.clone();
    let redo = inverse.clone().apply(&mut back).expect("inverse applies");
    assert_eq!(&back, d, "inverse did not restore the document");
    assert_eq!(save_string(&back), save_string(d), "bytes differ");
    redo.apply(&mut back).expect("redo applies");
    assert_eq!(back, after, "the inverse's inverse did not redo");
    (after, inverse)
}

fn apply_err(d: &Document, c: Command) -> CommandError {
    let mut after = d.clone();
    let e = c.apply(&mut after).expect_err("command should fail");
    assert_eq!(&after, d, "failed command changed the document");
    assert_eq!(save_string(&after), save_string(d));
    e
}

fn node<'a>(d: &'a Document, i: &str) -> &'a Node {
    d.node(&id(i)).unwrap()
}

fn color_of(p: &Paint) -> &ColorRef {
    match p {
        Paint::Solid(s) => &s.color,
        _ => panic!("not solid"),
    }
}

// --- UpsertTokens -----------------------------------------------------------

#[test]
fn upsert_tokens_adds_changes_and_removes() {
    let d = sample();
    let mut patch = TokensPatch::default();
    patch
        .colors
        .insert(tok("color.primary"), Some(Color::new("#ff0000")));
    patch
        .colors
        .insert(tok("color.new"), Some(Color::new("#00ff00")));
    patch.colors.insert(tok("color.unused"), None);
    patch.spacing.insert(tok("space.8"), Some(32.0));
    patch.radii.insert(tok("radius.lg"), Some(16.0));
    patch
        .fonts
        .insert(tok("font.mono"), Some("Fira Code".into()));
    let (after, inverse) = apply_and_check_inverse(&d, Command::UpsertTokens { patch });
    assert_eq!(after.tokens.colors[&tok("color.primary")].0, "#ff0000");
    assert_eq!(after.tokens.colors[&tok("color.new")].0, "#00ff00");
    assert!(!after.tokens.colors.contains_key(&tok("color.unused")));
    assert_eq!(after.tokens.spacing[&tok("space.8")], 32.0);
    assert_eq!(after.tokens.radii[&tok("radius.lg")], 16.0);
    assert_eq!(after.tokens.fonts[&tok("font.mono")], "Fira Code");
    // The inverse holds the previous state of exactly the patched keys.
    let Command::UpsertTokens { patch: inv } = inverse else {
        panic!("inverse is not UpsertTokens")
    };
    assert_eq!(inv.colors[&tok("color.new")], None);
    assert_eq!(
        inv.colors[&tok("color.unused")],
        Some(Color::new("#000000"))
    );
    assert!(!inv.spacing.contains_key(&tok("space.4")));
}

#[test]
fn removing_a_referenced_token_errors() {
    let d = sample();
    for (name, kind) in [
        ("color.primary", "colors"),
        ("space.4", "spacing"),
        ("radius.md", "radii"),
        ("font.sans", "fonts"),
    ] {
        let mut patch = TokensPatch::default();
        match kind {
            "colors" => drop(patch.colors.insert(tok(name), None)),
            "spacing" => drop(patch.spacing.insert(tok(name), None)),
            "radii" => drop(patch.radii.insert(tok(name), None)),
            _ => drop(patch.fonts.insert(tok(name), None)),
        }
        let e = apply_err(&d, Command::UpsertTokens { patch });
        assert!(matches!(e, CommandError::Invariant(_)), "{name}: {e}");
        assert!(e.to_string().contains(name), "{e}");
    }
}

#[test]
fn removing_a_token_works_once_it_is_detached() {
    let d = sample();
    let mut patch = TokensPatch::default();
    patch.colors.insert(tok("color.primary"), None);
    let mut cmds = Vec::new();
    // Detach every reference: styles first, then the nodes' own paints.
    let flat = Style {
        kind: StyleKind::Fill(FillStyle {
            fills: vec![Paint::solid(ColorRef::literal("#2563eb"))],
            extra: Extra::new(),
        }),
        ..d.styles[0].clone()
    };
    cmds.push(Command::UpsertStyle { style: flat });
    cmds.push(Command::UpsertStyle {
        style: Style {
            kind: StyleKind::Stroke(StrokeStyle {
                strokes: vec![Paint::solid(ColorRef::literal("#2563eb"))],
                width: 3.0,
                extra: Extra::new(),
            }),
            ..d.styles[1].clone()
        },
    });
    cmds.push(Command::UpsertStyle {
        style: Style {
            kind: StyleKind::Effect(EffectStyle {
                shadows: vec![shadow(ColorRef::literal("#2563eb"))],
                extra: Extra::new(),
            }),
            ..d.styles[3].clone()
        },
    });
    for n in [BOX, LABEL] {
        cmds.push(Command::SetProps {
            id: id(n),
            patch: NodePatch {
                fills: Some(vec![Paint::solid(ColorRef::literal("#2563eb"))]),
                ..NodePatch::default()
            },
        });
    }
    // The component master still uses it, so the batch fails as a whole.
    cmds.push(Command::UpsertTokens {
        patch: patch.clone(),
    });
    let e = apply_err(&d, Command::Batch(cmds.clone()));
    assert!(e.to_string().contains("color.primary"), "{e}");

    // Without the master (a document with no component) it goes through.
    let mut d2 = d.clone();
    d2.components.clear();
    apply_and_check_inverse(&d2, Command::Batch(cmds));
}

#[test]
fn invalid_token_values_and_names_error() {
    let d = sample();
    let mut patch = TokensPatch::default();
    patch
        .colors
        .insert(tok("color.bad"), Some(Color::new("red")));
    assert!(matches!(
        apply_err(&d, Command::UpsertTokens { patch }),
        CommandError::Invariant(_)
    ));
    let mut patch = TokensPatch::default();
    patch.spacing.insert(tok("space.neg"), Some(-4.0));
    apply_err(&d, Command::UpsertTokens { patch });
    let mut patch = TokensPatch::default();
    patch.spacing.insert(tok("has space"), Some(4.0));
    apply_err(&d, Command::UpsertTokens { patch });
}

#[test]
fn tokens_patch_parses_from_json() {
    let p: TokensPatch = serde_json::from_value(serde_json::json!({
        "colors": { "color.a": "#112233", "color.b": null },
        "spacing": { "space.1": 4 }
    }))
    .unwrap();
    assert_eq!(p.colors[&tok("color.a")], Some(Color::new("#112233")));
    assert_eq!(p.colors[&tok("color.b")], None);
    assert!(serde_json::from_value::<TokensPatch>(serde_json::json!({ "colours": {} })).is_err());
}

// --- RenameToken --------------------------------------------------------------

#[test]
fn rename_rewrites_nodes_styles_and_components() {
    let d = sample();
    let (after, inverse) = apply_and_check_inverse(
        &d,
        Command::RenameToken {
            from: tok("color.primary"),
            to: tok("color.brand"),
        },
    );
    assert_eq!(
        inverse.clone(),
        Command::RenameToken {
            from: tok("color.brand"),
            to: tok("color.primary"),
        }
    );
    assert!(after.tokens.colors.contains_key(&tok("color.brand")));
    assert!(!after.tokens.colors.contains_key(&tok("color.primary")));
    let brand = ColorRef::Token(tok("color.brand"));
    // Node fills, and the text node's fills.
    assert_eq!(
        color_of(&node(&after, BOX).shape().unwrap().fills[0]),
        &brand
    );
    let NodeKind::Text(t) = &node(&after, LABEL).kind else {
        panic!()
    };
    assert_eq!(color_of(&t.fills[0]), &brand);
    // Styles: fill, stroke and effect.
    let StyleKind::Fill(f) = &after.styles[0].kind else {
        panic!()
    };
    assert_eq!(color_of(&f.fills[0]), &brand);
    let StyleKind::Stroke(s) = &after.styles[1].kind else {
        panic!()
    };
    assert_eq!(color_of(&s.strokes[0]), &brand);
    let StyleKind::Effect(e) = &after.styles[3].kind else {
        panic!()
    };
    assert_eq!(e.shadows[0].color, brand);
    // The component master.
    let master = &after.components[0].root;
    assert_eq!(color_of(&master.shape().unwrap().fills[0]), &brand);
    assert_eq!(master.shape().unwrap().shadows[0].color, brand);
    assert!(!save_string(&after).contains("color.primary"));
}

#[test]
fn rename_rewrites_space_and_font_references() {
    let d = sample();
    let (after, _) = apply_and_check_inverse(
        &d,
        Command::RenameToken {
            from: tok("space.4"),
            to: tok("space.md"),
        },
    );
    let NodeKind::Frame(f) = &node(&after, FRAME).kind else {
        panic!()
    };
    let l = f.layout.as_ref().unwrap();
    let md = SpaceRef::Token(tok("space.md"));
    assert_eq!(l.gap, md);
    assert_eq!(l.cross_gap, Some(md.clone()));
    assert_eq!(l.padding[0], md);
    assert_eq!(l.padding[1], SpaceRef::Px(1.0));

    let (after, _) = apply_and_check_inverse(
        &d,
        Command::RenameToken {
            from: tok("font.sans"),
            to: tok("font.body"),
        },
    );
    let NodeKind::Text(t) = &node(&after, LABEL).kind else {
        panic!()
    };
    assert_eq!(t.style.font_family, "{font.body}");
    let StyleKind::Text(def) = &after.styles[2].kind else {
        panic!()
    };
    assert_eq!(def.style.font_family, "{font.body}");

    // Radii have no references; only the key changes.
    let (after, _) = apply_and_check_inverse(
        &d,
        Command::RenameToken {
            from: tok("radius.md"),
            to: tok("radius.medium"),
        },
    );
    assert_eq!(after.tokens.radii[&tok("radius.medium")], 8.0);
}

#[test]
fn rename_errors() {
    let d = sample();
    let e = apply_err(
        &d,
        Command::RenameToken {
            from: tok("color.primary"),
            to: tok("color.unused"),
        },
    );
    assert!(matches!(e, CommandError::Invariant(_)), "{e}");
    // `to` exists in another category too.
    apply_err(
        &d,
        Command::RenameToken {
            from: tok("color.primary"),
            to: tok("space.4"),
        },
    );
    assert!(matches!(
        apply_err(
            &d,
            Command::RenameToken {
                from: tok("nope"),
                to: tok("other"),
            },
        ),
        CommandError::NotFound(_)
    ));
    apply_err(
        &d,
        Command::RenameToken {
            from: tok("color.primary"),
            to: tok("{braces}"),
        },
    );
}

// --- UpsertStyle ----------------------------------------------------------------

#[test]
fn upsert_style_adds_and_replaces() {
    let d = sample();
    let new = Style {
        id: id("s_00000000n1"),
        name: "New".into(),
        kind: StyleKind::Fill(FillStyle::default()),
    };
    let (after, inverse) = apply_and_check_inverse(&d, Command::UpsertStyle { style: new });
    assert_eq!(after.styles.len(), 5);
    assert_eq!(after.styles[4].name, "New");
    assert_eq!(
        inverse,
        Command::DeleteStyle {
            id: id("s_00000000n1"),
            detach: false
        }
    );

    // Replacing keeps the position, and the inverse is the old style.
    let mut edited = d.styles[1].clone();
    edited.name = "Outline 2".into();
    if let StyleKind::Stroke(s) = &mut edited.kind {
        s.width = 5.0;
    }
    let (after, inverse) = apply_and_check_inverse(
        &d,
        Command::UpsertStyle {
            style: edited.clone(),
        },
    );
    assert_eq!(after.styles[1], edited);
    assert_eq!(
        inverse,
        Command::UpsertStyle {
            style: d.styles[1].clone()
        }
    );
}

#[test]
fn upsert_style_errors() {
    let d = sample();
    // The ID is a node's.
    let clash = Style {
        id: id(BOX),
        name: "x".into(),
        kind: StyleKind::Fill(FillStyle::default()),
    };
    apply_err(&d, Command::UpsertStyle { style: clash });
    // Changing the kind of a style that is in use in a slot of the old kind.
    let wrong = Style {
        id: id(CARD),
        name: "x".into(),
        kind: StyleKind::Effect(EffectStyle::default()),
    };
    let e = apply_err(&d, Command::UpsertStyle { style: wrong });
    assert!(e.to_string().contains("expected fill"), "{e}");
    // An unknown token in the style.
    let bad = Style {
        id: id(CARD),
        name: "x".into(),
        kind: StyleKind::Fill(FillStyle {
            fills: token_fill("color.missing"),
            extra: Extra::new(),
        }),
    };
    apply_err(&d, Command::UpsertStyle { style: bad });
}

// --- DeleteStyle -----------------------------------------------------------------

#[test]
fn delete_style_without_detach_errors_while_referenced() {
    let d = sample();
    for s in [CARD, OUTLINE, BODY, GLOW] {
        let e = apply_err(
            &d,
            Command::DeleteStyle {
                id: id(s),
                detach: false,
            },
        );
        assert!(e.to_string().contains("still used"), "{s}: {e}");
    }
    assert!(matches!(
        apply_err(
            &d,
            Command::DeleteStyle {
                id: id("s_00000000z9"),
                detach: true,
            },
        ),
        CommandError::NotFound(_)
    ));
}

#[test]
fn delete_unreferenced_style() {
    let mut d = sample();
    // Detach the style everywhere first.
    d.components[0].root.shape_mut().unwrap().fill_style = None;
    d.node_mut(&id(BOX))
        .unwrap()
        .shape_mut()
        .unwrap()
        .fill_style = None;
    let (after, inverse) = apply_and_check_inverse(
        &d,
        Command::DeleteStyle {
            id: id(CARD),
            detach: false,
        },
    );
    assert_eq!(after.styles.len(), 3);
    // Undo puts it back at its old index.
    assert!(matches!(inverse, Command::InsertStyle { index: 0, .. }));
}

#[test]
fn delete_style_with_detach_copies_values_onto_nodes() {
    let d = sample();
    for (style, check) in [(CARD, 0), (OUTLINE, 1), (GLOW, 2), (BODY, 3)] {
        let (after, inverse) = apply_and_check_inverse(
            &d,
            Command::DeleteStyle {
                id: id(style),
                detach: true,
            },
        );
        assert!(after.styles.iter().all(|s| s.id != id(style)));
        assert!(matches!(inverse, Command::Batch(_)));
        let b = node(&after, BOX).shape().unwrap();
        let before = node(&d, BOX).shape().unwrap();
        match check {
            0 => {
                assert_eq!(b.fill_style, None);
                assert_eq!(b.fills, token_fill("color.primary"));
                // The component master was detached too.
                let m = after.components[0].root.shape().unwrap();
                assert_eq!(m.fill_style, None);
                assert_eq!(m.fills, token_fill("color.primary"));
            }
            1 => {
                assert_eq!(b.stroke_style, None);
                assert_eq!(b.strokes, token_fill("color.primary"));
                assert_eq!(b.stroke_width, 3.0);
                assert_eq!(b.fill_style, before.fill_style, "other slots keep theirs");
            }
            2 => {
                assert_eq!(b.effect_style, None);
                assert_eq!(b.shadows.len(), 1);
            }
            _ => {
                let NodeKind::Text(t) = &node(&after, LABEL).kind else {
                    panic!()
                };
                assert_eq!(t.text_style, None);
                assert_eq!(t.style.font_size, 20.0);
            }
        }
    }
}

#[test]
fn deleting_a_detached_style_leaves_the_look_unchanged() {
    let d = sample();
    let before = node(&d, BOX);
    let (fills, strokes, shadows, width) = (
        effective_fills(&d, before).to_vec(),
        effective_strokes(&d, before).to_vec(),
        effective_shadows(&d, before).to_vec(),
        effective_stroke_width(&d, before),
    );
    let mut after = d.clone();
    for s in [CARD, OUTLINE, GLOW, BODY] {
        Command::DeleteStyle {
            id: id(s),
            detach: true,
        }
        .apply(&mut after)
        .unwrap();
    }
    let n = node(&after, BOX);
    assert_eq!(effective_fills(&after, n), fills);
    assert_eq!(effective_strokes(&after, n), strokes);
    assert_eq!(effective_shadows(&after, n), shadows);
    assert_eq!(effective_stroke_width(&after, n), width);
    assert_eq!(
        effective_text_style(&after, node(&after, LABEL)),
        effective_text_style(&d, node(&d, LABEL))
    );
}

// --- Detaching one node --------------------------------------------------------------

#[test]
fn detach_style_patch_detaches_one_node_through_set_props() {
    let d = sample();
    let patch = detach_style_patch(&d, node(&d, BOX), None).expect("has styles");
    let (after, inverse) = apply_and_check_inverse(&d, Command::SetProps { id: id(BOX), patch });
    let b = node(&after, BOX).shape().unwrap();
    assert_eq!(
        (
            b.fill_style.clone(),
            b.stroke_style.clone(),
            b.effect_style.clone()
        ),
        (None, None, None)
    );
    // The style itself, and other nodes using it, are untouched.
    assert_eq!(after.styles, d.styles);
    assert_eq!(
        after.components[0].root.shape().unwrap().fill_style,
        Some(id(CARD))
    );
    assert!(matches!(inverse, Command::SetProps { .. }));
    // Nothing left to detach.
    assert_eq!(detach_style_patch(&after, node(&after, BOX), None), None);
    // Only one style.
    let p = detach_style_patch(&d, node(&d, BOX), Some(&id(OUTLINE))).unwrap();
    assert_eq!(p.fill_style, None);
    assert_eq!(p.stroke_style, Some(None));
}

// --- Resolution -------------------------------------------------------------------------

#[test]
fn resolution_applies_styles_and_tokens() {
    let d = sample();
    let b = node(&d, BOX);
    // The fill style wins over the inline fills.
    assert_eq!(effective_fills(&d, b), token_fill("color.primary"));
    assert_eq!(effective_stroke_width(&d, b), 3.0);
    assert_eq!(effective_strokes(&d, b).len(), 1);
    assert_eq!(effective_shadows(&d, b).len(), 1);
    let t = effective_text_style(&d, node(&d, LABEL)).unwrap();
    assert_eq!(t.font_size, 20.0);
    assert_eq!(effective_text_style(&d, b), None);

    // Without the reference, the inline values apply.
    let mut e = d.clone();
    let shape = e.node_mut(&id(BOX)).unwrap().shape_mut().unwrap();
    shape.fill_style = None;
    shape.fills = vec![Paint::solid(ColorRef::literal("#123456"))];
    shape.stroke_style = None;
    shape.stroke_width = 7.0;
    assert_eq!(effective_stroke_width(&e, node(&e, BOX)), 7.0);
    assert_eq!(
        color_of(&effective_fills(&e, node(&e, BOX))[0]),
        &ColorRef::literal("#123456")
    );
    // A text node's own fills.
    assert_eq!(effective_fills(&d, node(&d, LABEL)).len(), 1);
    assert!(effective_fills(&d, node(&d, FRAME)).is_empty());

    let primary = ColorRef::Token(tok("color.primary"));
    assert_eq!(resolve_color(&d, &primary), Some(&Color::new("#2563eb")));
    assert_eq!(resolve_color(&d, &ColorRef::Token(tok("nope"))), None);
    assert_eq!(
        resolve_color(&d, &ColorRef::literal("#fff000")),
        Some(&Color::new("#fff000"))
    );
    assert_eq!(
        resolve_space(&d, &SpaceRef::Token(tok("space.4"))),
        Some(16.0)
    );
    assert_eq!(resolve_space(&d, &SpaceRef::Px(3.0)), Some(3.0));
    assert_eq!(resolve_space(&d, &SpaceRef::Token(tok("nope"))), None);
    assert_eq!(resolve_font(&d, "{font.sans}"), "Inter");
    assert_eq!(resolve_font(&d, "Georgia"), "Georgia");
    assert_eq!(resolve_font(&d, "{font.nope}"), "{font.nope}");
}

#[test]
fn undo_and_redo_through_history() {
    let mut d = sample();
    let original = d.clone();
    let mut h = History::default();
    let mut patch = TokensPatch::default();
    patch
        .colors
        .insert(tok("color.primary"), Some(Color::new("#010203")));
    for (label, c) in [
        ("tokens", Command::UpsertTokens { patch }),
        (
            "rename",
            Command::RenameToken {
                from: tok("space.4"),
                to: tok("space.md"),
            },
        ),
        (
            "delete",
            Command::DeleteStyle {
                id: id(CARD),
                detach: true,
            },
        ),
    ] {
        h.commit(
            &mut d,
            label,
            Origin::User,
            vec![c],
            Selection::default(),
            Selection::default(),
        )
        .unwrap();
    }
    let edited = d.clone();
    while h.undo(&mut d).unwrap().is_some() {}
    assert_eq!(d, original);
    assert_eq!(save_string(&d), save_string(&original));
    while h.redo(&mut d).unwrap().is_some() {}
    assert_eq!(d, edited);
}

// --- Radius references ----------------------------------------------------------

#[test]
fn rename_rewrites_radius_references() {
    let d = sample();
    let (after, _) = apply_and_check_inverse(
        &d,
        Command::RenameToken {
            from: tok("radius.md"),
            to: tok("radius.card"),
        },
    );
    let r = RadiusRef::Token(tok("radius.card"));
    assert_eq!(
        node(&after, BOX).shape().unwrap().corner_radius,
        CornerRadius::Each([r.clone(), RadiusRef::Px(1.0), r.clone(), RadiusRef::Px(2.0)])
    );
    assert_eq!(
        after.components[0].root.shape().unwrap().corner_radius,
        CornerRadius::All(r)
    );
}

#[test]
fn radius_references_are_canonical_and_round_trip() {
    let d = sample();
    let text = save_string(&d);
    assert!(text.contains("\"{radius.md}\""), "{text}");
    // Plain numbers stay plain numbers.
    assert!(!text.contains("\"1\"") && !text.contains("\"2\""));
    let loaded = load_str(&text).unwrap();
    assert_eq!(save_string(&loaded.doc), text);
    assert_eq!(loaded.doc.pages, d.pages);
}

#[test]
fn an_unknown_radius_token_is_a_problem() {
    let mut d = sample();
    d.tokens.radii.clear();
    let problems = validate(&d);
    assert!(
        problems
            .iter()
            .any(|p| p.message.contains("unknown radius token radius.md")),
        "{problems:?}"
    );
}

#[test]
fn set_props_links_and_unlinks_a_radius() {
    let d = sample();
    let link = CornerRadius::All(RadiusRef::Token(tok("radius.md")));
    let (after, inv) = apply_and_check_inverse(
        &d,
        Command::SetProps {
            id: id(BOX),
            patch: NodePatch {
                corner_radius: Some(link.clone()),
                ..NodePatch::default()
            },
        },
    );
    assert_eq!(node(&after, BOX).shape().unwrap().corner_radius, link);
    assert_eq!(effective_corner_radius(&after, node(&after, BOX)), [8.0; 4]);
    // An unknown token is refused.
    let e = apply_err(
        &d,
        Command::SetProps {
            id: id(BOX),
            patch: NodePatch {
                corner_radius: Some(CornerRadius::All(RadiusRef::Token(tok("radius.nope")))),
                ..NodePatch::default()
            },
        },
    );
    assert!(e.to_string().contains("radius.nope"), "{e}");
    drop(inv);
    // Resolution of the sample's mixed radius.
    assert_eq!(
        effective_corner_radius(&d, node(&d, BOX)),
        [8.0, 1.0, 8.0, 2.0]
    );
}

#[test]
fn changing_a_radius_token_changes_the_effective_radius() {
    let d = sample();
    let mut patch = TokensPatch::default();
    patch.radii.insert(tok("radius.md"), Some(20.0));
    let (after, _) = apply_and_check_inverse(&d, Command::UpsertTokens { patch });
    assert_eq!(
        effective_corner_radius(&after, node(&after, BOX)),
        [20.0, 1.0, 20.0, 2.0]
    );
}
