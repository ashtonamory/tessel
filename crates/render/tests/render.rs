//! Render tests. The GPU ones skip with a message when no adapter is found
//! (CI and sandboxes may lack Vulkan), per SPEC §11.

use std::sync::{Mutex, MutexGuard, OnceLock};

use doc::{
    ColorRef, Document, Frame, Id, IdKind, Node, NodeId, NodeKind, Page, Paint, Rect, Shape, Text,
};
use kurbo::{Affine, Rect as KRect};
use layout::LayoutEngine;
use render::peniko::Color;
use render::{page_scene, SceneCache, SceneOptions, SceneRenderer};

fn id(n: u64) -> NodeId {
    Id::from_entropy(IdKind::Node, n)
}

fn filled(color: &str) -> Shape {
    Shape {
        fills: vec![Paint::solid(ColorRef::literal(color))],
        ..Default::default()
    }
}

fn red_rect(n: u64, x: f32, y: f32) -> Node {
    Node::new(
        id(n),
        "r",
        NodeKind::Rect(Rect {
            shape: filled("#ff0000"),
            ..Default::default()
        }),
    )
    .with_geometry(x, y, 20.0, 20.0)
}

fn document() -> Document {
    let mut frame = Node::new(
        id(1),
        "frame",
        NodeKind::Frame(Frame {
            shape: filled("#0000ff"),
            ..Default::default()
        }),
    )
    .with_geometry(40.0, 0.0, 40.0, 40.0);
    // Pokes out of the clipping frame on the right.
    frame.children_mut().unwrap().push(red_rect(2, 30.0, 10.0));
    let text = Node::new(
        id(3),
        "text",
        NodeKind::Text(Text {
            content: "Hello".into(),
            fills: vec![Paint::solid(ColorRef::literal("#000000"))],
            ..Default::default()
        }),
    )
    .with_geometry(0.0, 50.0, 0.0, 0.0);
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    page.children = vec![red_rect(4, 0.0, 0.0), frame, text];
    Document::new("t", page)
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: SceneRenderer,
}

/// One device shared by all tests, used one test at a time: creating GL
/// contexts concurrently crashes some drivers (Mesa llvmpipe).
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
    eprintln!("GPU render tests on {:?}", adapter.get_info());
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

#[test]
fn renders_shapes_clipping_and_text() {
    let Some(mut gpu) = gpu() else { return };
    let Gpu {
        device,
        queue,
        renderer,
    } = gpu.as_mut().unwrap();
    let doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let scene = page_scene(&doc, &geom, &doc.pages[0]);
    let (w, h) = (100, 80);
    let rgba = renderer
        .render_to_rgba(device, queue, &scene, w, h, Color::WHITE)
        .expect("render");
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    assert_eq!(pixel(&rgba, w, 10, 10), [255, 0, 0, 255], "red rect");
    assert_eq!(pixel(&rgba, w, 50, 5), [0, 0, 255, 255], "blue frame");
    assert_eq!(
        pixel(&rgba, w, 75, 20),
        [255, 0, 0, 255],
        "child inside the frame"
    );
    assert_eq!(
        pixel(&rgba, w, 85, 20),
        [255, 255, 255, 255],
        "child clipped by the frame"
    );
    assert_eq!(pixel(&rgba, w, 30, 30), [255, 255, 255, 255], "background");
    let text_area_dark = (50..75)
        .flat_map(|y| (0..60).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&rgba, w, x, y)[0] < 128)
        .count();
    if geom.get(&id(3)).unwrap().size.width > 0.0 {
        assert!(
            text_area_dark > 20,
            "text drew {text_area_dark} dark pixels"
        );
    }
}

#[test]
fn view_transform_scales_the_page() {
    let Some(mut gpu) = gpu() else { return };
    let Gpu {
        device,
        queue,
        renderer,
    } = gpu.as_mut().unwrap();
    let doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let mut cache = SceneCache::new();
    let view = Affine::translate((10.0, 10.0)) * Affine::scale(2.0);
    let scene = cache.build(
        &doc,
        &geom,
        &doc.pages[0],
        view,
        KRect::new(-1e3, -1e3, 1e3, 1e3),
        &SceneOptions::default(),
    );
    let rgba = renderer
        .render_to_rgba(device, queue, &scene, 100, 100, Color::WHITE)
        .expect("render");
    // The 20x20 red rect at the origin now covers (10,10)..(50,50).
    assert_eq!(pixel(&rgba, 100, 45, 45), [255, 0, 0, 255]);
    assert_eq!(pixel(&rgba, 100, 5, 5), [255, 255, 255, 255]);
}

#[test]
fn scene_cache_invalidates_by_page_level_node() {
    let doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let mut cache = SceneCache::new();
    let all = KRect::new(-1e3, -1e3, 1e3, 1e3);
    let opts = SceneOptions::default();
    cache.build(&doc, &geom, &doc.pages[0], Affine::IDENTITY, all, &opts);
    assert_eq!(cache.len(), 3);
    // A change inside the frame drops only the frame's scene.
    cache.invalidate(&doc, [&id(2)]);
    assert_eq!(cache.len(), 2);
    cache.build(&doc, &geom, &doc.pages[0], Affine::IDENTITY, all, &opts);
    assert_eq!(cache.len(), 3);
    // Changing options (hiding a node being edited) rebuilds everything.
    let hide = SceneOptions {
        hidden: Some(id(3)),
    };
    cache.build(&doc, &geom, &doc.pages[0], Affine::IDENTITY, all, &hide);
    assert_eq!(cache.len(), 3);
    // Deleted page-level nodes are dropped.
    let mut doc2 = doc.clone();
    doc::Command::DeleteNode { id: id(4) }
        .apply(&mut doc2)
        .unwrap();
    cache.build(&doc2, &geom, &doc2.pages[0], Affine::IDENTITY, all, &hide);
    assert_eq!(cache.len(), 2);
}

#[test]
fn styles_and_tokens_change_the_drawn_color() {
    use doc::{Color as DocColor, Command, FillStyle, Style, StyleKind, TokenId};
    use doc::{StrokeStyle, TokensPatch};

    let Some(mut gpu) = gpu() else { return };
    let Gpu {
        device,
        queue,
        renderer,
    } = gpu.as_mut().unwrap();

    let primary = TokenId::new("color.primary");
    let fill_style = Id::from_entropy(IdKind::Style, 1);
    let stroke_style = Id::from_entropy(IdKind::Style, 2);

    // Left: a fill style (whose own color is a token) with an inline fill
    // that must be ignored. Middle: an inline token fill. Right: a stroke
    // style on an unfilled rect.
    let mut left = red_rect(10, 0.0, 0.0);
    left.shape_mut().unwrap().fill_style = Some(fill_style.clone());
    let mut middle = red_rect(11, 30.0, 0.0);
    middle.shape_mut().unwrap().fills = vec![Paint::solid(ColorRef::Token(primary.clone()))];
    let mut right = red_rect(12, 60.0, 0.0);
    {
        let s = right.shape_mut().unwrap();
        s.fills.clear();
        s.stroke_style = Some(stroke_style.clone());
    }
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    page.children = vec![left, middle, right];
    let mut d = Document::new("t", page);
    d.tokens
        .colors
        .insert(primary.clone(), DocColor::new("#00ff00"));
    d.styles.push(Style {
        id: fill_style.clone(),
        name: "Fill".into(),
        kind: StyleKind::Fill(FillStyle {
            fills: vec![Paint::solid(ColorRef::Token(primary.clone()))],
            ..Default::default()
        }),
    });
    d.styles.push(Style {
        id: stroke_style.clone(),
        name: "Stroke".into(),
        kind: StyleKind::Stroke(StrokeStyle {
            strokes: vec![Paint::solid(ColorRef::literal("#0000ff"))],
            width: 6.0,
            extra: Default::default(),
        }),
    });
    assert_eq!(doc::validate(&d), vec![]);

    let mut render_px = |d: &Document| {
        let geom = LayoutEngine::new().compute(d);
        let scene = page_scene(d, &geom, &d.pages[0]);
        let rgba = renderer
            .render_to_rgba(device, queue, &scene, 90, 30, Color::WHITE)
            .expect("render");
        [
            pixel(&rgba, 90, 10, 10),
            pixel(&rgba, 90, 40, 10),
            pixel(&rgba, 90, 61, 10),
            pixel(&rgba, 90, 70, 10),
        ]
    };
    let [fill, inline, stroke, hole] = render_px(&d);
    assert_eq!(
        fill,
        [0, 255, 0, 255],
        "fill style via token, not inline red"
    );
    assert_eq!(inline, [0, 255, 0, 255], "inline token fill");
    assert_eq!(stroke, [0, 0, 255, 255], "stroke style, 6px inside");
    assert_eq!(hole, [255, 255, 255, 255], "unfilled interior");

    // Changing the token recolors both; changing the style recolors the left.
    let mut tokens = TokensPatch::default();
    tokens
        .colors
        .insert(primary.clone(), Some(DocColor::new("#ff00ff")));
    Command::UpsertTokens { patch: tokens }
        .apply(&mut d)
        .unwrap();
    let [fill, inline, ..] = render_px(&d);
    assert_eq!(fill, [255, 0, 255, 255]);
    assert_eq!(inline, [255, 0, 255, 255]);

    let mut style = d.styles[0].clone();
    style.kind = StyleKind::Fill(FillStyle {
        fills: vec![Paint::solid(ColorRef::literal("#ffff00"))],
        ..Default::default()
    });
    Command::UpsertStyle { style }.apply(&mut d).unwrap();
    let [fill, inline, ..] = render_px(&d);
    assert_eq!(fill, [255, 255, 0, 255]);
    assert_eq!(inline, [255, 0, 255, 255]);
}

#[test]
fn radius_tokens_change_the_drawn_corner() {
    use doc::{Command, CornerRadius, RadiusRef, TokenId, TokensPatch};

    let Some(mut gpu) = gpu() else { return };
    let Gpu {
        device,
        queue,
        renderer,
    } = gpu.as_mut().unwrap();

    let md = TokenId::new("radius.md");
    let mut rect = red_rect(10, 0.0, 0.0);
    rect.shape_mut().unwrap().corner_radius = CornerRadius::All(RadiusRef::Token(md.clone()));
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    page.children = vec![rect];
    let mut d = Document::new("t", page);
    d.tokens.radii.insert(md.clone(), 0.0);
    assert_eq!(doc::validate(&d), vec![]);

    let mut render_px = |d: &Document| {
        let geom = LayoutEngine::new().compute(d);
        let scene = page_scene(d, &geom, &d.pages[0]);
        let rgba = renderer
            .render_to_rgba(device, queue, &scene, 20, 20, Color::WHITE)
            .expect("render");
        [pixel(&rgba, 20, 1, 1), pixel(&rgba, 20, 10, 10)]
    };
    let [corner, centre] = render_px(&d);
    assert_eq!(corner, [255, 0, 0, 255], "square corner");
    assert_eq!(centre, [255, 0, 0, 255]);

    let mut tokens = TokensPatch::default();
    tokens.radii.insert(md, Some(10.0));
    Command::UpsertTokens { patch: tokens }
        .apply(&mut d)
        .unwrap();
    let [corner, centre] = render_px(&d);
    assert_eq!(corner, [255, 255, 255, 255], "rounded away by the token");
    assert_eq!(centre, [255, 0, 0, 255]);
}
