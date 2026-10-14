//! Instances in layout (§3.7, §7.3, §7.5): an instance lays out as the
//! frame it stands for, its derived children get geometry, auto-layout
//! inside masters works, and hit-testing treats it as a unit until entered.

use std::collections::{BTreeMap, BTreeSet};

use doc::*;
use kurbo::{Point, Rect as KRect};
use layout::{ancestors, hit_test, size_writeback, HitOptions, LayoutEngine};

fn id(s: &str) -> Id {
    Id::from_raw(s)
}

fn rect(i: &str, x: f32, y: f32, w: f32, h: f32) -> Node {
    Node::new(id(i), "Rect", NodeKind::Rect(Rect::default())).with_geometry(x, y, w, h)
}

fn frame(i: &str, layout: Option<AutoLayout>, children: Vec<Node>) -> Node {
    Node::new(
        id(i),
        "Frame",
        NodeKind::Frame(Frame {
            layout,
            children,
            ..Frame::default()
        }),
    )
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
}

fn row(gap: f32, padding: f32, width: Sizing, height: Sizing) -> AutoLayout {
    AutoLayout {
        direction: Direction::Row,
        wrap: false,
        gap: SpaceRef::Px(gap),
        cross_gap: None,
        padding: [
            SpaceRef::Px(padding),
            SpaceRef::Px(padding),
            SpaceRef::Px(padding),
            SpaceRef::Px(padding),
        ],
        justify: Justify::Start,
        align: Align::Start,
        width,
        height,
        extra: Extra::new(),
    }
}

const STACK: &str = "c_00000000s1";
const BOX: &str = "c_00000000b1";
const S_ROOT: &str = "n_00000000s0";
const S_A: &str = "n_00000000sa";
const S_B: &str = "n_00000000sb";
const B_ROOT: &str = "n_00000000b0";
const B_BOX: &str = "n_00000000b2";
const B_INNER: &str = "n_00000000b3";
const I_STACK: &str = "n_00000000p1";
const I_BOX: &str = "n_00000000p2";
const ROW: &str = "n_00000000p3";
const I_IN_ROW: &str = "n_00000000p4";

fn d(s: &str) -> Id {
    Id::from_raw(s)
}

/// Stack: an auto-layout row (hug, padding 5, gap 10) of two rects 20x10
/// and 30x10. Box: a fixed 100x50 frame with a rect pinned right and a
/// nested Stack instance.
fn sample() -> Document {
    let stack = frame(
        S_ROOT,
        Some(row(10.0, 5.0, Sizing::Hug, Sizing::Hug)),
        vec![
            rect(S_A, 0.0, 0.0, 20.0, 10.0),
            rect(S_B, 0.0, 0.0, 30.0, 10.0),
        ],
    )
    .with_geometry(0.0, 0.0, 1.0, 1.0);
    let mut pinned = rect(B_BOX, 70.0, 10.0, 20.0, 20.0);
    pinned.constraints.horizontal = HConstraint::Right;
    let boxc = frame(
        B_ROOT,
        None,
        vec![
            pinned,
            instance(B_INNER, STACK).with_geometry(5.0, 30.0, 70.0, 20.0),
        ],
    )
    .with_geometry(0.0, 0.0, 100.0, 50.0);
    let mut page = Page::new(Id::from_raw("p_0000000001"), "Page");
    page.children = vec![
        instance(I_STACK, STACK).with_geometry(100.0, 100.0, 1.0, 1.0),
        instance(I_BOX, BOX).with_geometry(300.0, 100.0, 200.0, 50.0),
        frame(
            ROW,
            Some(row(4.0, 0.0, Sizing::Hug, Sizing::Hug)),
            vec![
                rect("n_00000000r1", 0.0, 0.0, 10.0, 10.0),
                instance(I_IN_ROW, STACK).with_geometry(0.0, 0.0, 1.0, 1.0),
            ],
        )
        .with_geometry(0.0, 300.0, 1.0, 1.0),
    ];
    let mut doc = Document::new("t", page);
    for (cid, name, root) in [(STACK, "Stack", stack), (BOX, "Box", boxc)] {
        doc.components.push(Component {
            id: id(cid),
            name: name.into(),
            props: vec![],
            extra: Extra::new(),
            root,
        });
    }
    doc.rebuild_index();
    assert!(validate(&doc).is_empty(), "{:?}", validate(&doc));
    doc
}

fn bounds(g: &layout::Geometry, i: &str) -> KRect {
    g.get(&d(i))
        .unwrap_or_else(|| panic!("no geometry for {i}"))
        .bounds
}

fn approx(a: KRect, b: KRect) {
    let ok = (a.x0 - b.x0).abs() < 1e-3
        && (a.y0 - b.y0).abs() < 1e-3
        && (a.x1 - b.x1).abs() < 1e-3
        && (a.y1 - b.y1).abs() < 1e-3;
    assert!(ok, "{a:?} != {b:?}");
}

#[test]
fn auto_layout_inside_a_master_places_derived_children() {
    let doc = sample();
    let g = LayoutEngine::new().compute(&doc);
    // Hug: 5 + 20 + 10 + 30 + 5 wide, 5 + 10 + 5 high.
    approx(bounds(&g, I_STACK), KRect::new(100.0, 100.0, 170.0, 120.0));
    approx(
        bounds(&g, &format!("{I_STACK}/{S_A}")),
        KRect::new(105.0, 105.0, 125.0, 115.0),
    );
    approx(
        bounds(&g, &format!("{I_STACK}/{S_B}")),
        KRect::new(135.0, 105.0, 165.0, 115.0),
    );
    assert_eq!(
        g.get(&d(&format!("{I_STACK}/{S_A}"))).unwrap().parent,
        Some(d(I_STACK))
    );
    let shown = g.expanded(&d(I_STACK)).expect("expansion kept");
    assert!(matches!(shown.kind, NodeKind::Frame(_)));
    // The hug size is written back to the instance.
    let writes = size_writeback(&doc, &g);
    assert!(
        writes
            .iter()
            .any(|w| w.id == d(I_STACK) && w.width == 70.0 && w.height == 20.0),
        "{writes:?}"
    );
}

#[test]
fn instances_take_their_own_size_and_nest() {
    let doc = sample();
    let g = LayoutEngine::new().compute(&doc);
    approx(bounds(&g, I_BOX), KRect::new(300.0, 100.0, 500.0, 150.0));
    // Pinned right: 10 from the right edge of the 200-wide instance.
    approx(
        bounds(&g, &format!("{I_BOX}/{B_BOX}")),
        KRect::new(470.0, 110.0, 490.0, 130.0),
    );
    // The nested Stack instance and its own derived children.
    let nested = format!("{I_BOX}/{B_INNER}");
    approx(bounds(&g, &nested), KRect::new(305.0, 130.0, 375.0, 150.0));
    approx(
        bounds(&g, &format!("{nested}/{S_B}")),
        KRect::new(340.0, 135.0, 370.0, 145.0),
    );
    assert!(g.expanded(&d(&nested)).is_some());
}

#[test]
fn instances_are_flow_children_of_auto_layout_frames() {
    let doc = sample();
    let g = LayoutEngine::new().compute(&doc);
    // After the 10px rect and a 4px gap, hugging its own content.
    approx(bounds(&g, I_IN_ROW), KRect::new(14.0, 300.0, 84.0, 320.0));
    approx(bounds(&g, ROW), KRect::new(0.0, 300.0, 84.0, 320.0));
    approx(
        bounds(&g, &format!("{I_IN_ROW}/{S_A}")),
        KRect::new(19.0, 305.0, 39.0, 315.0),
    );
}

#[test]
fn a_master_change_relayouts_every_instance() {
    let mut doc = sample();
    let mut engine = LayoutEngine::new();
    engine.compute(&doc);
    // Widen rect A in the master by 10: every Stack grows.
    let mut stack = doc.components[0].clone();
    stack.root.children_mut().unwrap()[0].width = 30.0;
    let c = Command::UpsertComponent { component: stack };
    let touched = c.touched_ids();
    c.apply(&mut doc).unwrap();
    let g = engine.update(&doc, &touched);
    approx(bounds(&g, I_STACK), KRect::new(100.0, 100.0, 180.0, 120.0));
    approx(bounds(&g, ROW), KRect::new(0.0, 300.0, 94.0, 320.0));
    let nested = format!("{I_BOX}/{B_INNER}/{S_B}");
    approx(bounds(&g, &nested), KRect::new(350.0, 135.0, 380.0, 145.0));
    // An override (a derived SetProps) hides a child: the row reflows.
    let c = Command::SetProps {
        id: d(&format!("{I_STACK}/{S_A}")),
        patch: NodePatch {
            visible: Some(false),
            ..NodePatch::default()
        },
    };
    let touched: BTreeSet<NodeId> = c.touched_ids();
    c.apply(&mut doc).unwrap();
    let g = engine.update(&doc, &touched);
    approx(bounds(&g, I_STACK), KRect::new(100.0, 100.0, 140.0, 120.0));
}

#[test]
fn a_missing_component_leaves_a_plain_box() {
    let mut doc = sample();
    doc.pages[0]
        .children
        .push(instance("n_00000000p9", "c_00000000zz").with_geometry(0.0, 0.0, 40.0, 30.0));
    doc.rebuild_index();
    let g = LayoutEngine::new().compute(&doc);
    approx(bounds(&g, "n_00000000p9"), KRect::new(0.0, 0.0, 40.0, 30.0));
    assert!(g.expanded(&d("n_00000000p9")).is_none());
}

#[test]
fn hit_testing_selects_an_instance_whole_until_entered() {
    let doc = sample();
    let g = LayoutEngine::new().compute(&doc);
    let page = &doc.pages[0];
    let p = Point::new(110.0, 110.0); // over Stack's rect A
    assert_eq!(
        hit_test(&g, page, p, HitOptions::default()),
        Some(d(I_STACK))
    );
    let sel = [d(I_STACK)];
    let entered = HitOptions {
        selected: &sel,
        ..Default::default()
    };
    let a = d(&format!("{I_STACK}/{S_A}"));
    assert_eq!(hit_test(&g, page, p, entered), Some(a.clone()));
    // Selecting a derived node keeps the instance entered.
    let sel = [d(&format!("{I_STACK}/{S_B}"))];
    let entered = HitOptions {
        selected: &sel,
        ..Default::default()
    };
    assert_eq!(hit_test(&g, page, p, entered), Some(a.clone()));
    // Padding shows the instance frame itself.
    assert_eq!(
        hit_test(&g, page, Point::new(102.0, 102.0), entered),
        Some(d(I_STACK))
    );
    // A nested instance is a unit inside its entered parent.
    let q = Point::new(310.0, 140.0);
    let sel = [d(I_BOX)];
    let opts = HitOptions {
        selected: &sel,
        ..Default::default()
    };
    let nested = d(&format!("{I_BOX}/{B_INNER}"));
    assert_eq!(hit_test(&g, page, q, opts), Some(nested.clone()));
    let deep = HitOptions {
        deep: true,
        ..Default::default()
    };
    let leaf = d(&format!("{I_BOX}/{B_INNER}/{S_A}"));
    assert_eq!(hit_test(&g, page, q, deep), Some(leaf.clone()));
    // Not a prefix match on IDs that merely share characters.
    let sel = [d(&format!("{I_BOX}x"))];
    let opts = HitOptions {
        selected: &sel,
        ..Default::default()
    };
    assert_eq!(hit_test(&g, page, q, opts), Some(d(I_BOX)));
    assert_eq!(ancestors(&doc, &leaf), vec![nested, d(I_BOX)]);
}
