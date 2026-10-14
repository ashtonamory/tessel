//! Seeded random property tests (§5.2, §11). No proptest (it would need a
//! fetch); a small SplitMix64 generator drives everything, so failures are
//! reproducible from the seed printed in the assertion message.
//!
//! For random documents and random commands:
//! - a command that applies, followed by its inverse, gives back an equal
//!   document with byte-identical serialization, and the inverse's inverse
//!   re-applies the command exactly;
//! - a command that fails leaves the document unchanged;
//! - the node index always agrees with the tree;
//! - save -> load -> save is byte-identical.

use doc::*;

const SEEDS: u64 = 300;
const STEPS: usize = 25;

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1u32 << 24) as f32;
        lo + unit * (hi - lo)
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn node_id(&mut self) -> Id {
        Id::from_entropy(IdKind::Node, self.next())
    }
}

const COLOR_TOKENS: [&str; 2] = ["color.primary", "color.bg"];
const TEXT_STYLE: &str = "s_00000000t5";
const FILL_STYLE: &str = "s_00000000f5";
const STROKE_STYLE: &str = "s_00000000k5";
const EFFECT_STYLE: &str = "s_00000000e5";
const NEW_STYLE: &str = "s_00000000n5";
const ALL_TOKENS: [&str; 6] = [
    "color.primary",
    "color.bg",
    "color.extra",
    "space.4",
    "font.sans",
    "radius.md",
];

fn color(r: &mut Rng) -> ColorRef {
    if r.chance(0.3) {
        ColorRef::Token(TokenId::new(*r.pick(&COLOR_TOKENS)))
    } else {
        ColorRef::Literal(Color::new(format!("#{:06x}", r.next() & 0xff_ffff)))
    }
}

fn paints(r: &mut Rng) -> Vec<Paint> {
    (0..r.below(3))
        .map(|_| {
            if r.chance(0.8) {
                Paint::solid(color(r))
            } else {
                Paint::LinearGradient(LinearGradient {
                    start: [0.0, r.range(0.0, 1.0)],
                    end: [1.0, r.range(0.0, 1.0)],
                    stops: vec![
                        Stop {
                            offset: 0.0,
                            color: color(r),
                        },
                        Stop {
                            offset: r.range(0.0, 1.0),
                            color: color(r),
                        },
                    ],
                    extra: Extra::new(),
                })
            }
        })
        .collect()
}

fn shape(r: &mut Rng) -> Shape {
    Shape {
        fills: paints(r),
        strokes: if r.chance(0.3) { paints(r) } else { vec![] },
        stroke_width: if r.chance(0.3) {
            r.range(0.0, 8.0)
        } else {
            1.0
        },
        corner_radius: match r.below(3) {
            0 => CornerRadius::px(0.0),
            1 => CornerRadius::px(r.range(0.0, 24.0)),
            _ => CornerRadius::Each(
                [r.range(0.0, 9.0), 0.0, r.range(0.0, 9.0), 1.0].map(RadiusRef::Px),
            ),
        },
        shadows: if r.chance(0.2) {
            vec![shadow(r)]
        } else {
            vec![]
        },
        fill_style: r.chance(0.1).then(|| Id::from_raw(FILL_STYLE)),
        stroke_style: r.chance(0.05).then(|| Id::from_raw(STROKE_STYLE)),
        effect_style: r.chance(0.05).then(|| Id::from_raw(EFFECT_STYLE)),
        ..Shape::default()
    }
}

fn shadow(r: &mut Rng) -> Shadow {
    Shadow {
        x: r.range(-4.0, 4.0),
        y: r.range(-4.0, 4.0),
        blur: r.range(0.0, 20.0),
        spread: 0.0,
        color: color(r),
        extra: Extra::new(),
    }
}

fn text_content(r: &mut Rng) -> String {
    let parts = [
        "Hello", " ", "wörld", "\n", "\"q\"", "\\", "\u{1}", "🎨", "—", "tab\t",
    ];
    (0..r.below(6)).map(|_| *r.pick(&parts)).collect()
}

/// An instance of one of `comps`, with overrides of its master's nodes
/// (and, for nested instances, of theirs).
fn instance_kind(r: &mut Rng, comps: &[Component]) -> NodeKind {
    let c = r.pick(comps);
    let mut keys = Vec::new();
    c.root.walk(&mut |n| {
        keys.push(n.id.to_string());
        if let NodeKind::Instance(i) = &n.kind {
            if let Some(inner) = comps.iter().find(|c| c.id == i.component) {
                let first = inner.root.children().and_then(|k| k.first());
                if let Some(k) = first {
                    keys.push(format!("{}/{}", n.id, k.id));
                }
            }
        }
    });
    let mut overrides = std::collections::BTreeMap::new();
    for _ in 0..r.below(3) {
        let key = r.pick(&keys).clone();
        let o = match r.below(4) {
            0 => Override::Text {
                content: text_content(r),
            },
            1 => Override::Fills { fills: paints(r) },
            2 => Override::Visible {
                visible: r.chance(0.5),
            },
            _ => Override::Swap {
                component: r.pick(comps).id.clone(),
            },
        };
        overrides.insert(key, o);
    }
    NodeKind::Instance(Instance {
        component: c.id.clone(),
        overrides,
        extra: Extra::new(),
    })
}

/// `comps` are the components an instance may use.
fn leaf(r: &mut Rng, comps: &[Component]) -> NodeKind {
    if !comps.is_empty() && r.chance(0.15) {
        return instance_kind(r, comps);
    }
    match r.below(3) {
        0 => NodeKind::Rect(Rect {
            shape: shape(r),
            ..Rect::default()
        }),
        1 => NodeKind::Ellipse(Ellipse {
            shape: shape(r),
            ..Ellipse::default()
        }),
        _ => NodeKind::Text(Text {
            content: text_content(r),
            fills: paints(r),
            text_style: r.chance(0.3).then(|| Id::from_raw(TEXT_STYLE)),
            style: TextStyle {
                font_family: if r.chance(0.2) {
                    "{font.sans}".to_string()
                } else {
                    "Inter".to_string()
                },
                font_size: r.range(8.0, 72.0),
                font_weight: 100 * (1 + r.below(9) as u16),
                italic: r.chance(0.2),
                line_height: *r.pick(&[
                    LineHeight::Auto,
                    LineHeight::Px(24.0),
                    LineHeight::Percent(150.0),
                ]),
                ..TextStyle::default()
            },
            align: *r.pick(&[TextAlign::Left, TextAlign::Center, TextAlign::Justify]),
            semantic: r.chance(0.2).then(|| "p".to_string()),
            ..Text::default()
        }),
    }
}

fn auto_layout(r: &mut Rng) -> AutoLayout {
    AutoLayout {
        direction: *r.pick(&[Direction::Row, Direction::Column]),
        wrap: r.chance(0.2),
        gap: if r.chance(0.5) {
            SpaceRef::Token(TokenId::new("space.4"))
        } else {
            SpaceRef::Px(r.range(0.0, 32.0))
        },
        cross_gap: None,
        padding: [
            SpaceRef::Px(r.range(0.0, 16.0)),
            SpaceRef::Px(0.0),
            SpaceRef::Token(TokenId::new("space.4")),
            SpaceRef::Px(0.0),
        ],
        justify: *r.pick(&[Justify::Start, Justify::SpaceBetween]),
        align: *r.pick(&[Align::Start, Align::Center, Align::Stretch]),
        width: *r.pick(&[Sizing::Fixed, Sizing::Hug, Sizing::Fill]),
        height: *r.pick(&[Sizing::Fixed, Sizing::Hug]),
        extra: Extra::new(),
    }
}

fn node(r: &mut Rng, depth: usize, comps: &[Component]) -> Node {
    let kind = if depth < 3 && r.chance(0.35) {
        let n = if r.chance(0.5) {
            r.below(4)
        } else {
            1 + r.below(3)
        };
        let children = (0..n)
            .map(|_| node(r, depth + 1, comps))
            .collect::<Vec<_>>();
        if !children.is_empty() && r.chance(0.3) {
            NodeKind::Group(Group {
                children,
                ..Group::default()
            })
        } else {
            NodeKind::Frame(Frame {
                shape: shape(r),
                clip: r.chance(0.8),
                layout: r.chance(0.4).then(|| auto_layout(r)),
                semantic: r.chance(0.1).then(|| "section".to_string()),
                children,
                ..Frame::default()
            })
        }
    } else {
        leaf(r, comps)
    };
    let mut n = Node::new(r.node_id(), format!("Layer {}", r.below(100)), kind).with_geometry(
        r.range(-500.0, 1500.0),
        r.range(-500.0, 1500.0),
        r.range(0.0, 800.0),
        r.range(0.0, 600.0),
    );
    if r.chance(0.2) {
        n.rotation = r.range(-180.0, 180.0);
    }
    if r.chance(0.2) {
        n.opacity = r.range(0.0, 1.0);
    }
    n.visible = !r.chance(0.1);
    n.locked = r.chance(0.1);
    if r.chance(0.15) {
        n.layout_child.width = Sizing::Fill;
        n.layout_child.min_width = Some(r.range(0.0, 100.0));
    }
    n
}

fn document(r: &mut Rng) -> Document {
    let mut d = Document::default();
    d.meta.title = "Random".into();
    d.tokens
        .colors
        .insert(TokenId::new("color.primary"), Color::new("#2563eb"));
    d.tokens
        .colors
        .insert(TokenId::new("color.bg"), Color::new("#ffffff"));
    d.tokens.spacing.insert(TokenId::new("space.4"), 16.0);
    d.tokens.radii.insert(TokenId::new("radius.md"), 8.0);
    d.tokens
        .fonts
        .insert(TokenId::new("font.sans"), "Inter".to_string());
    d.styles.push(Style {
        id: Id::from_raw(TEXT_STYLE),
        name: "Body".into(),
        kind: StyleKind::Text(TextStyleDef::default()),
    });
    d.styles.push(Style {
        id: Id::from_raw(FILL_STYLE),
        name: "Card".into(),
        kind: StyleKind::Fill(FillStyle {
            fills: vec![Paint::solid(ColorRef::literal("#eeeeee"))],
            extra: Extra::new(),
        }),
    });
    d.styles.push(Style {
        id: Id::from_raw(STROKE_STYLE),
        name: "Outline".into(),
        kind: StyleKind::Stroke(StrokeStyle {
            strokes: vec![Paint::solid(ColorRef::Token(TokenId::new("color.primary")))],
            width: 2.0,
            extra: Extra::new(),
        }),
    });
    d.styles.push(Style {
        id: Id::from_raw(EFFECT_STYLE),
        name: "Raised".into(),
        kind: StyleKind::Effect(EffectStyle {
            shadows: vec![shadow(r)],
            extra: Extra::new(),
        }),
    });
    // Each component may use the ones before it, so there are no cycles.
    for c in 0..r.below(4) {
        let comp = random_component(r, None, &d.components);
        let mut comp = comp;
        comp.name = format!("Component {c}");
        d.components.push(comp);
    }
    for p in 0..1 + r.below(2) {
        let mut page = Page::new(
            Id::from_entropy(IdKind::Page, r.next()),
            format!("Page {p}"),
        );
        page.children = (0..r.below(5)).map(|_| node(r, 0, &d.components)).collect();
        d.pages.push(page);
    }
    d.rebuild_index();
    d
}

/// A component whose root is a frame with a few children, using `comps`.
fn random_component(r: &mut Rng, id: Option<Id>, comps: &[Component]) -> Component {
    let id = id.unwrap_or_else(|| Id::from_entropy(IdKind::Component, r.next()));
    let children = (0..1 + r.below(3)).map(|_| node(r, 2, comps)).collect();
    let root = Node::new(
        r.node_id(),
        "Root",
        NodeKind::Frame(Frame {
            shape: shape(r),
            layout: r.chance(0.4).then(|| auto_layout(r)),
            children,
            ..Frame::default()
        }),
    )
    .with_geometry(0.0, 0.0, r.range(10.0, 300.0), r.range(10.0, 200.0));
    Component {
        id,
        name: "Component".into(),
        props: vec![],
        extra: Extra::new(),
        root,
    }
}

fn all_node_ids(d: &Document) -> Vec<Id> {
    let mut ids = Vec::new();
    d.walk_nodes(&mut |n| ids.push(n.id.clone()));
    ids
}

fn random_parent(r: &mut Rng, d: &Document, ids: &[Id]) -> ParentRef {
    if ids.is_empty() || r.chance(0.3) {
        ParentRef::Page(r.pick(&d.pages).id.clone())
    } else {
        // Often a non-container, to exercise the error path.
        ParentRef::Node(r.pick(ids).clone())
    }
}

fn random_index(r: &mut Rng, d: &Document, parent: &ParentRef) -> Option<usize> {
    let len = d.children_of(parent).map_or(0, Vec::len);
    match r.below(4) {
        0 => None,
        1 => Some(len + 1), // out of range
        _ => Some(r.below(len + 1)),
    }
}

fn random_id(r: &mut Rng, ids: &[Id]) -> Id {
    if ids.is_empty() || r.chance(0.05) {
        r.node_id() // probably missing
    } else {
        r.pick(ids).clone()
    }
}

fn random_patch(r: &mut Rng) -> NodePatch {
    let mut p = NodePatch::default();
    for _ in 0..1 + r.below(3) {
        match r.below(18) {
            0 => p.name = Some(format!("Renamed {}", r.below(10))),
            1 => p.visible = Some(r.chance(0.5)),
            2 => p.locked = Some(r.chance(0.5)),
            3 => p.x = Some(r.range(-100.0, 100.0)),
            4 => p.width = Some(r.range(-10.0, 300.0)), // sometimes invalid
            5 => p.opacity = Some(r.range(0.0, 1.2)),   // sometimes invalid
            6 => p.rotation = Some(r.range(-90.0, 90.0)),
            7 => p.fills = Some(paints(r)),
            8 => p.strokes = Some(paints(r)),
            9 => p.corner_radius = Some(CornerRadius::px(r.range(0.0, 12.0))),
            10 => p.content = Some(text_content(r)),
            11 => p.text_style = Some(r.chance(0.5).then(|| Id::from_raw(TEXT_STYLE))),
            12 => p.layout = Some(r.chance(0.5).then(|| auto_layout(r))),
            13 => p.semantic = Some(r.chance(0.5).then(|| "nav".to_string())),
            14 => p.fill_style = Some(r.chance(0.5).then(|| Id::from_raw(FILL_STYLE))),
            15 => p.stroke_style = Some(r.chance(0.7).then(|| Id::from_raw(STROKE_STYLE))),
            16 => p.effect_style = Some(r.chance(0.7).then(|| Id::from_raw(EFFECT_STYLE))),
            _ => {
                p.layout_child = Some(LayoutChild {
                    height: Sizing::Hug,
                    ..LayoutChild::default()
                })
            }
        }
    }
    p
}

fn token_name(r: &mut Rng) -> TokenId {
    TokenId::new(*r.pick(&ALL_TOKENS))
}

fn tokens_patch(r: &mut Rng) -> TokensPatch {
    let mut p = TokensPatch::default();
    for _ in 0..1 + r.below(3) {
        let name = token_name(r);
        // `None` removes, which fails while the token is referenced.
        let remove = r.chance(0.3);
        match r.below(4) {
            0 => {
                let v = r
                    .chance(0.9)
                    .then(|| Color::new(format!("#{:06x}", r.next() & 0xff_ffff)));
                p.colors.insert(name, if remove { None } else { v });
            }
            1 => {
                p.spacing
                    .insert(name, (!remove).then(|| r.range(-1.0, 40.0))); // sometimes invalid
            }
            2 => {
                p.radii.insert(name, (!remove).then(|| r.range(0.0, 20.0)));
            }
            _ => {
                p.fonts
                    .insert(name, (!remove).then(|| format!("Font {}", r.below(3))));
            }
        }
    }
    p
}

fn random_style(r: &mut Rng, d: &Document) -> Style {
    let id = if d.styles.is_empty() || r.chance(0.25) {
        Id::from_raw(NEW_STYLE)
    } else {
        r.pick(&d.styles).id.clone()
    };
    let kind = match r.below(4) {
        0 => StyleKind::Fill(FillStyle {
            fills: paints(r),
            extra: Extra::new(),
        }),
        1 => StyleKind::Stroke(StrokeStyle {
            strokes: paints(r),
            width: r.range(0.0, 6.0),
            extra: Extra::new(),
        }),
        2 => StyleKind::Text(TextStyleDef {
            style: TextStyle {
                font_size: r.range(8.0, 40.0),
                ..TextStyle::default()
            },
            extra: Extra::new(),
        }),
        _ => StyleKind::Effect(EffectStyle {
            shadows: (0..r.below(3)).map(|_| shadow(r)).collect(),
            extra: Extra::new(),
        }),
    };
    Style {
        id,
        name: format!("Style {}", r.below(10)),
        kind,
    }
}

fn random_command(r: &mut Rng, d: &Document, depth: usize) -> Command {
    let ids = all_node_ids(d);
    match r.below(if depth == 0 { 17 } else { 16 }) {
        0 => {
            let parent = random_parent(r, d, &ids);
            let index = random_index(r, d, &parent);
            let mut node = node(r, 1, &d.components);
            if !ids.is_empty() && r.chance(0.03) {
                node.id = r.pick(&ids).clone(); // duplicate
            }
            Command::CreateNode {
                parent,
                index,
                node,
            }
        }
        1 => Command::DeleteNode {
            id: random_id(r, &ids),
        },
        2 => {
            let parent = random_parent(r, d, &ids);
            let index = random_index(r, d, &parent);
            Command::MoveNode {
                id: random_id(r, &ids),
                parent,
                index,
            }
        }
        3 | 4 => Command::SetProps {
            id: random_id(r, &ids),
            patch: random_patch(r),
        },
        5 => Command::SetGeometry {
            id: random_id(r, &ids),
            x: r.range(-100.0, 100.0),
            y: r.range(-100.0, 100.0),
            width: r.range(-5.0, 200.0),
            height: r.range(0.0, 200.0),
            rotation: if r.chance(0.5) {
                0.0
            } else {
                r.range(-45.0, 45.0)
            },
        },
        6 => Command::UpsertTokens {
            patch: tokens_patch(r),
        },
        7 => Command::RenameToken {
            from: token_name(r),
            to: if r.chance(0.7) {
                TokenId::new(format!("renamed.{}", r.below(3)))
            } else {
                token_name(r)
            },
        },
        8 => Command::UpsertStyle {
            style: random_style(r, d),
        },
        9 | 10 => Command::DeleteStyle {
            id: if d.styles.is_empty() || r.chance(0.05) {
                Id::from_raw(NEW_STYLE)
            } else {
                r.pick(&d.styles).id.clone()
            },
            detach: r.chance(0.5),
        },
        11 => Command::CreateComponent {
            from: random_id(r, &ids),
        },
        12 => {
            // A new component, or a change to an existing one (which may
            // make a cycle or reuse IDs, to exercise the error path).
            let mut c = random_component(r, None, &d.components);
            if !d.components.is_empty() && r.chance(0.6) {
                let old = r.pick(&d.components);
                c.id = old.id.clone();
                if r.chance(0.5) {
                    c.root.id = old.root.id.clone();
                }
            }
            Command::UpsertComponent { component: c }
        }
        13 => Command::DeleteComponent {
            id: if d.components.is_empty() || r.chance(0.05) {
                Id::from_entropy(IdKind::Component, r.next())
            } else {
                r.pick(&d.components).id.clone()
            },
            detach: r.chance(0.6),
        },
        14 => {
            let instances = instance_ids(d);
            Command::DetachInstance {
                id: if instances.is_empty() || r.chance(0.1) {
                    random_id(r, &ids)
                } else {
                    r.pick(&instances).clone()
                },
            }
        }
        15 => derived_set_props(r, d, &ids),
        _ => {
            // Generate each part against the state the batch will have when
            // that part runs, so most batches are valid.
            let mut sim = d.clone();
            let mut parts = Vec::new();
            for _ in 0..1 + r.below(4) {
                let c = random_command(r, &sim, depth + 1);
                let _ = c.clone().apply(&mut sim);
                parts.push(c);
            }
            Command::Batch(parts)
        }
    }
}

/// Instances on pages and in component masters.
fn instance_ids(d: &Document) -> Vec<Id> {
    let mut out = Vec::new();
    let mut visit = |n: &Node| {
        if let NodeKind::Instance(_) = n.kind {
            out.push(n.id.clone());
        }
    };
    d.walk_nodes(&mut visit);
    for c in &d.components {
        c.root.walk(&mut visit);
    }
    out
}

/// A `SetProps` on a node inside an instance (an override), or of an
/// instance's overrides.
fn derived_set_props(r: &mut Rng, d: &Document, ids: &[Id]) -> Command {
    let instances = instance_ids(d);
    if instances.is_empty() {
        return Command::SetProps {
            id: random_id(r, ids),
            patch: random_patch(r),
        };
    }
    let inst = r.pick(&instances).clone();
    let shown = d.node_or_master(&inst).and_then(|n| expand_instance(d, n));
    let mut derived = Vec::new();
    if let Some(e) = &shown {
        for c in e.children().into_iter().flatten() {
            c.walk(&mut |n| derived.push(n.id.clone()));
        }
    }
    if derived.is_empty() || r.chance(0.3) {
        let key = derived
            .first()
            .map(|id| id.as_str()[inst.as_str().len() + 1..].to_string())
            .unwrap_or_else(|| "n_0000000000".into());
        let value = r.chance(0.7).then(|| Override::Visible {
            visible: r.chance(0.5),
        });
        return Command::SetProps {
            id: inst,
            patch: NodePatch {
                overrides: Some(std::collections::BTreeMap::from([(key, value)])),
                ..NodePatch::default()
            },
        };
    }
    let mut patch = NodePatch::default();
    match r.below(5) {
        0 => patch.content = Some(text_content(r)),
        1 => patch.fills = Some(paints(r)),
        2 => patch.visible = Some(r.chance(0.5)),
        3 if !d.components.is_empty() => patch.component = Some(r.pick(&d.components).id.clone()),
        _ => patch.x = Some(1.0), // read-only
    }
    Command::SetProps {
        id: r.pick(&derived).clone(),
        patch,
    }
}

fn assert_index_consistent(d: &Document, ctx: &str) {
    let mut count = 0;
    d.walk_nodes(&mut |n| {
        count += 1;
        assert_eq!(
            d.node(&n.id).map(|m| &m.id),
            Some(&n.id),
            "{ctx}: index lost {}",
            n.id
        );
    });
    assert_eq!(d.node_ids().count(), count, "{ctx}: stale index entries");
}

#[test]
fn apply_then_inverse_restores_the_document() {
    let (mut applied, mut failed) = (0, 0);
    let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
    for seed in 0..SEEDS {
        let r = &mut Rng(seed);
        let mut d = document(r);
        assert!(validate(&d).is_empty(), "seed {seed}: {:?}", validate(&d));
        for step in 0..STEPS {
            let ctx = format!("seed {seed} step {step}");
            let c = random_command(r, &d, 0);
            let before = d.clone();
            let before_bytes = save_string(&d);
            match c.clone().apply(&mut d) {
                Ok(inverse) => {
                    applied += 1;
                    *kinds.entry(kind_of(&c)).or_default() += 1;
                    assert_index_consistent(&d, &ctx);
                    let after = d.clone();
                    let redo = inverse
                        .clone()
                        .apply(&mut d)
                        .unwrap_or_else(|e| panic!("{ctx}: inverse failed: {e}\n{inverse:?}"));
                    assert_eq!(d, before, "{ctx}: inverse did not restore\n{c:?}");
                    assert_eq!(save_string(&d), before_bytes, "{ctx}: bytes differ");
                    assert_index_consistent(&d, &ctx);
                    redo.apply(&mut d)
                        .unwrap_or_else(|e| panic!("{ctx}: redo failed: {e}"));
                    assert_eq!(d, after, "{ctx}: inverse of inverse did not redo\n{c:?}");
                    assert!(validate(&d).is_empty(), "{ctx}: {:?}", validate(&d));
                }
                Err(_) => {
                    failed += 1;
                    assert_eq!(
                        d, before,
                        "{ctx}: failed command changed the document\n{c:?}"
                    );
                    assert_eq!(save_string(&d), before_bytes, "{ctx}");
                    assert_index_consistent(&d, &ctx);
                }
            }
        }
    }
    // Both paths must be exercised for the test to mean anything.
    let total = applied + failed;
    assert!(
        applied * 3 > total,
        "only {applied}/{total} commands applied"
    );
    assert!(failed * 20 > total, "only {failed}/{total} commands failed");
    for k in [
        "CreateComponent",
        "UpsertComponent",
        "DeleteComponent",
        "DetachInstance",
        "SetProps(derived)",
    ] {
        let n = kinds.get(k).copied().unwrap_or(0);
        assert!(n >= 20, "only {n} {k} applied: {kinds:?}");
    }
}

/// What a command is, for counting.
fn kind_of(c: &Command) -> &'static str {
    match c {
        Command::CreateComponent { .. } => "CreateComponent",
        Command::UpsertComponent { .. } => "UpsertComponent",
        Command::DeleteComponent { .. } => "DeleteComponent",
        Command::DetachInstance { .. } => "DetachInstance",
        Command::SetProps { id, .. } if id.is_derived() => "SetProps(derived)",
        Command::Batch(_) => "Batch",
        _ => "other",
    }
}

#[test]
fn history_undo_all_restores_the_original() {
    for seed in 0..SEEDS / 3 {
        let r = &mut Rng(1_000_000 + seed);
        let original = document(r);
        let mut d = original.clone();
        let mut h = History::default();
        let mut states = vec![save_string(&d)];
        for _ in 0..STEPS {
            let c = random_command(r, &d, 0);
            if h.commit(
                &mut d,
                "step",
                Origin::User,
                vec![c],
                Selection::default(),
                Selection::default(),
            )
            .is_ok()
            {
                states.push(save_string(&d));
            }
        }
        // Undo walks back through every state, redo forward again.
        for expected in states.iter().rev().skip(1) {
            h.undo(&mut d).unwrap().expect("something to undo");
            assert_eq!(&save_string(&d), expected, "seed {seed}");
        }
        assert_eq!(d, original, "seed {seed}");
        for expected in states.iter().skip(1) {
            h.redo(&mut d).unwrap().expect("something to redo");
            assert_eq!(&save_string(&d), expected, "seed {seed}");
        }
    }
}

#[test]
fn save_load_save_is_byte_identical() {
    for seed in 0..SEEDS {
        let r = &mut Rng(2_000_000 + seed);
        let d = document(r);
        let first = save_string(&d);
        let loaded = load_str(&first).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{first}"));
        assert!(
            loaded.problems.is_empty(),
            "seed {seed}: {:?}",
            loaded.problems
        );
        let second = save_string(&loaded.doc);
        assert_eq!(second, first, "seed {seed}");
        // A loaded document is already rounded, so it survives exactly.
        let again = load_str(&second).unwrap().doc;
        assert_eq!(again, loaded.doc, "seed {seed}");
    }
}
