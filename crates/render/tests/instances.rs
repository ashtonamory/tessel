//! Instances in render (§3.7, §7.4): drawn as the frames they stand for,
//! with overrides; the scene cache follows component changes. The GPU tests
//! skip with a message when no adapter is found.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use doc::{
    ColorRef, Command, Component, Document, Extra, Frame, Id, IdKind, Instance, Node, NodeKind,
    Override, Page, Paint, Rect, Shape,
};
use kurbo::{Affine, Rect as KRect};
use layout::LayoutEngine;
use render::peniko::Color;
use render::{
    node_scene, page_scene, top_level_of, ImageCache, SceneCache, SceneOptions, SceneRenderer,
};

fn id(s: &str) -> Id {
    Id::from_raw(s)
}

fn filled(color: &str) -> Shape {
    Shape {
        fills: vec![Paint::solid(ColorRef::literal(color))],
        ..Default::default()
    }
}

fn rect(i: &str, color: &str, x: f32, y: f32, w: f32, h: f32) -> Node {
    Node::new(
        id(i),
        "r",
        NodeKind::Rect(Rect {
            shape: filled(color),
            ..Default::default()
        }),
    )
    .with_geometry(x, y, w, h)
}

fn frame(i: &str, color: &str, children: Vec<Node>) -> Node {
    Node::new(
        id(i),
        "f",
        NodeKind::Frame(Frame {
            shape: filled(color),
            children,
            ..Default::default()
        }),
    )
}

fn instance(i: &str, component: &str, overrides: &[(&str, Override)]) -> Node {
    Node::new(
        id(i),
        "i",
        NodeKind::Instance(Instance {
            component: id(component),
            overrides: overrides
                .iter()
                .map(|(k, o)| (k.to_string(), o.clone()))
                .collect::<BTreeMap<_, _>>(),
            extra: Extra::new(),
        }),
    )
}

fn component(i: &str, root: Node) -> Component {
    Component {
        id: id(i),
        name: "C".into(),
        props: vec![],
        extra: Extra::new(),
        root,
    }
}

const TILE: &str = "c_00000000t1";
const PAIR: &str = "c_00000000p1";
const T_ROOT: &str = "n_00000000t0";
const T_DOT: &str = "n_00000000t2";
const P_ROOT: &str = "n_00000000p0";
const P_TILE: &str = "n_00000000p2";
const I1: &str = "n_00000000a1";
const I2: &str = "n_00000000a2";
const I3: &str = "n_00000000a3";
const PLAIN: &str = "n_00000000a4";
const BROKEN: &str = "n_00000000a5";

/// Tile: a 20x20 blue frame with a 10x10 red dot at (5,5). Pair: a 40x20
/// white frame holding a Tile at (20,0). On the page: a Tile at (0,0), a
/// Tile at (30,0) with its dot green, a Pair at (0,30), a plain rect, and an
/// instance of a missing component.
fn document() -> Document {
    let tile = frame(
        T_ROOT,
        "#0000ff",
        vec![rect(T_DOT, "#ff0000", 5.0, 5.0, 10.0, 10.0)],
    )
    .with_geometry(0.0, 0.0, 20.0, 20.0);
    let pair = frame(
        P_ROOT,
        "#ffffff",
        vec![instance(P_TILE, TILE, &[]).with_geometry(20.0, 0.0, 20.0, 20.0)],
    )
    .with_geometry(0.0, 0.0, 40.0, 20.0);
    let green = Override::Fills {
        fills: vec![Paint::solid(ColorRef::literal("#00ff00"))],
    };
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    page.children = vec![
        instance(I1, TILE, &[]).with_geometry(0.0, 0.0, 20.0, 20.0),
        instance(I2, TILE, &[(T_DOT, green)]).with_geometry(30.0, 0.0, 20.0, 20.0),
        instance(I3, PAIR, &[]).with_geometry(0.0, 30.0, 40.0, 20.0),
        rect(PLAIN, "#000000", 60.0, 0.0, 10.0, 10.0),
        instance(BROKEN, "c_00000000zz", &[]).with_geometry(60.0, 30.0, 20.0, 20.0),
    ];
    let mut doc = Document::new("t", page);
    doc.components = vec![component(TILE, tile), component(PAIR, pair)];
    doc.rebuild_index();
    doc
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: SceneRenderer,
}

fn gpu() -> Option<MutexGuard<'static, Option<Gpu>>> {
    static GPU: OnceLock<Mutex<Option<Gpu>>> = OnceLock::new();
    let guard = GPU
        .get_or_init(|| Mutex::new(create_gpu()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.is_some().then_some(guard)
}

fn create_gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("skipping GPU render tests: no adapter ({e})");
                return None;
            }
        };
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skipping GPU render tests: no device ({e})");
                return None;
            }
        };
    match SceneRenderer::new(&device) {
        Ok(renderer) => Some(Gpu {
            device,
            queue,
            renderer,
        }),
        Err(e) => {
            eprintln!("skipping GPU render tests: vello cannot run here ({e})");
            None
        }
    }
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const GREY: [u8; 4] = [0xee, 0xee, 0xee, 255];

#[test]
fn instances_draw_their_component_with_overrides() {
    let Some(mut gpu) = gpu() else { return };
    let Gpu {
        device,
        queue,
        renderer,
    } = gpu.as_mut().unwrap();
    let doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let scene = page_scene(&doc, &geom, &doc.pages[0]);
    let (w, h) = (90, 60);
    let rgba = renderer
        .render_to_rgba(device, queue, &scene, w, h, Color::from_rgba8(0, 0, 0, 0))
        .expect("render");
    assert_eq!(pixel(&rgba, w, 2, 2), BLUE, "tile frame");
    assert_eq!(pixel(&rgba, w, 10, 10), RED, "tile dot");
    assert_eq!(pixel(&rgba, w, 32, 2), BLUE, "second tile frame");
    assert_eq!(pixel(&rgba, w, 40, 10), GREEN, "overridden dot");
    assert_eq!(pixel(&rgba, w, 5, 40), WHITE, "pair frame");
    assert_eq!(pixel(&rgba, w, 22, 32), BLUE, "nested tile frame");
    assert_eq!(pixel(&rgba, w, 30, 40), RED, "nested tile dot");
    assert_eq!(
        pixel(&rgba, w, 70, 35),
        GREY,
        "missing component placeholder"
    );
}

#[test]
fn derived_nodes_render_alone() {
    let Some(mut gpu) = gpu() else { return };
    let Gpu {
        device,
        queue,
        renderer,
    } = gpu.as_mut().unwrap();
    let doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let dot = Id::from_raw(format!("{I3}/{P_TILE}/{T_DOT}"));
    let images = ImageCache::default();
    let (scene, bounds) = node_scene(&doc, &geom, &dot, &images).expect("derived node scene");
    assert_eq!(bounds, KRect::new(25.0, 35.0, 35.0, 45.0));
    let view = Affine::translate((-bounds.x0, -bounds.y0));
    let mut moved = render::vello::Scene::new();
    moved.append(&scene, Some(view));
    let rgba = renderer
        .render_to_rgba(device, queue, &moved, 10, 10, Color::WHITE)
        .expect("render");
    assert_eq!(pixel(&rgba, 10, 5, 5), RED);
    assert!(node_scene(
        &doc,
        &geom,
        &Id::from_raw(format!("{I3}/n_00000000zz")),
        &images
    )
    .is_none());
}

#[test]
fn scene_cache_follows_component_changes() {
    let mut doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let mut cache = SceneCache::new();
    let all = KRect::new(-1e3, -1e3, 1e3, 1e3);
    let opts = SceneOptions::default();
    let page = doc.pages[0].clone();
    cache.build(&doc, &geom, &page, Affine::IDENTITY, all, &opts);
    assert_eq!(cache.len(), 5);

    // A derived ID invalidates its instance's scene.
    assert_eq!(
        top_level_of(&doc, &Id::from_raw(format!("{I3}/{P_TILE}/{T_DOT}"))),
        Some(id(I3))
    );
    cache.invalidate(&doc, [&Id::from_raw(format!("{I2}/{T_DOT}"))]);
    assert_eq!(cache.len(), 4);
    cache.build(&doc, &geom, &page, Affine::IDENTITY, all, &opts);
    assert_eq!(cache.len(), 5);

    // Editing the Tile master (a SetProps on a master node touches no page
    // node) drops every scene showing a Tile, nested ones included, on the
    // next build, and nothing else.
    Command::SetProps {
        id: id(T_DOT),
        patch: doc::NodePatch {
            fills: Some(vec![]),
            ..Default::default()
        },
    }
    .apply(&mut doc)
    .unwrap();
    cache.sync_components(&doc);
    assert_eq!(
        cache.len(),
        2,
        "only the plain rect and the broken instance stay"
    );
    // A new component (used by nothing yet) drops nothing.
    let geom = LayoutEngine::new().compute(&doc);
    cache.build(&doc, &geom, &page, Affine::IDENTITY, all, &opts);
    assert_eq!(cache.len(), 5);
    doc.components.push(component(
        "c_00000000n1",
        frame("n_00000000n0", "#000000", vec![]),
    ));
    cache.sync_components(&doc);
    assert_eq!(cache.len(), 5);
}
