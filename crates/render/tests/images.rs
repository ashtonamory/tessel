//! Image rendering: every fit mode, shape clipping, image fills and missing
//! assets. GPU tests skip with a message when no adapter is found (SPEC §11).

use std::sync::{Mutex, MutexGuard, OnceLock};

use doc::{
    AssetRef, Assets, CornerRadius, Document, Id, IdKind, Image, ImageFit, ImagePaint, Node,
    NodeKind, Page, Paint, Rect, Shape,
};
use layout::LayoutEngine;
use render::peniko::Color;
use render::{page_scene_with_images, ImageCache, SceneRenderer};

const SIZE: u32 = 40;

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
                eprintln!("skipping GPU image tests: no adapter ({e})");
                return None;
            }
        };
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skipping GPU image tests: no device ({e})");
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
            eprintln!("skipping GPU image tests: vello cannot run here ({e})");
            None
        }
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A 4x2 PNG with four colored columns (aspect 2:1).
fn columns_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(4, 2, |x, _| {
        image::Rgba([RED, GREEN, BLUE, YELLOW][x as usize])
    });
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn nid(n: u64) -> doc::NodeId {
    Id::from_entropy(IdKind::Node, n)
}

fn page_doc(node: Node) -> Document {
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    page.children.push(node);
    Document::new("t", page)
}

fn image_node(asset: &AssetRef, fit: ImageFit, radius: CornerRadius) -> Node {
    Node::new(
        nid(1),
        "img",
        NodeKind::Image(Image {
            asset: asset.clone(),
            fit,
            corner_radius: radius,
            extra: Default::default(),
        }),
    )
    .with_geometry(0.0, 0.0, SIZE as f32, SIZE as f32)
}

fn render(gpu: &mut Gpu, doc: &Document, images: &ImageCache) -> Vec<u8> {
    let geom = LayoutEngine::new().compute(doc);
    let scene = page_scene_with_images(doc, &geom, &doc.pages[0], images);
    gpu.renderer
        .render_to_rgba(&gpu.device, &gpu.queue, &scene, SIZE, SIZE, Color::WHITE)
        .expect("render")
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[track_caller]
/// Loose: smooth upscaling blends neighbouring columns.
fn near(got: [u8; 4], want: [u8; 4], what: &str) {
    let ok = got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 70);
    assert!(ok, "{what}: got {got:?}, want {want:?}");
}

fn setup() -> (Assets, AssetRef, ImageCache) {
    let assets = Assets::new();
    let a = assets.add(columns_png(), "png");
    let cache = ImageCache::new(assets.clone());
    (assets, a, cache)
}

#[test]
fn fill_stretches_the_image_over_the_box() {
    let Some(mut g) = gpu() else { return };
    let (_, a, cache) = setup();
    let doc = page_doc(image_node(&a, ImageFit::Fill, CornerRadius::px(0.0)));
    let rgba = render(g.as_mut().unwrap(), &doc, &cache);
    for (i, c) in [RED, GREEN, BLUE, YELLOW].into_iter().enumerate() {
        let x = i as u32 * 10 + 5;
        near(px(&rgba, x, 3), c, "top");
        near(px(&rgba, x, 36), c, "bottom");
    }
}

#[test]
fn fit_letterboxes_and_keeps_aspect() {
    let Some(mut g) = gpu() else { return };
    let (_, a, cache) = setup();
    let doc = page_doc(image_node(&a, ImageFit::Fit, CornerRadius::px(0.0)));
    let rgba = render(g.as_mut().unwrap(), &doc, &cache);
    near(px(&rgba, 5, 20), RED, "red column");
    near(px(&rgba, 35, 20), YELLOW, "yellow column");
    near(px(&rgba, 5, 5), WHITE, "above the image");
    near(px(&rgba, 35, 35), WHITE, "below the image");
}

#[test]
fn crop_covers_the_box_centered() {
    let Some(mut g) = gpu() else { return };
    let (_, a, cache) = setup();
    let doc = page_doc(image_node(&a, ImageFit::Crop, CornerRadius::px(0.0)));
    let rgba = render(g.as_mut().unwrap(), &doc, &cache);
    // The outer columns are cropped away: green on the left half, blue right.
    near(px(&rgba, 5, 3), GREEN, "left top");
    near(px(&rgba, 35, 36), BLUE, "right bottom");
    near(px(&rgba, 5, 20), GREEN, "left middle");
}

#[test]
fn tile_repeats_at_natural_size() {
    let Some(mut g) = gpu() else { return };
    let (_, a, cache) = setup();
    let doc = page_doc(image_node(&a, ImageFit::Tile, CornerRadius::px(0.0)));
    let rgba = render(g.as_mut().unwrap(), &doc, &cache);
    for (x, c) in [
        (0, RED),
        (1, GREEN),
        (2, BLUE),
        (3, YELLOW),
        (4, RED),
        (39, YELLOW),
    ] {
        near(px(&rgba, x, 20), c, &format!("tile column {x}"));
    }
    near(px(&rgba, 38, 39), BLUE, "bottom row repeats too");
}

#[test]
fn clips_to_the_corner_radius() {
    let Some(mut g) = gpu() else { return };
    let (_, a, cache) = setup();
    let doc = page_doc(image_node(&a, ImageFit::Fill, CornerRadius::px(20.0)));
    let rgba = render(g.as_mut().unwrap(), &doc, &cache);
    near(px(&rgba, 0, 0), WHITE, "corner is clipped away");
    near(px(&rgba, 39, 39), WHITE, "opposite corner");
    near(px(&rgba, 15, 20), GREEN, "center is drawn");
}

#[test]
fn image_paint_fills_a_shape() {
    let Some(mut g) = gpu() else { return };
    let (_, a, cache) = setup();
    let node = Node::new(
        nid(1),
        "r",
        NodeKind::Rect(Rect {
            shape: Shape {
                fills: vec![Paint::Image(ImagePaint {
                    asset: a,
                    fit: ImageFit::Fill,
                    extra: Default::default(),
                })],
                ..Default::default()
            },
            ..Default::default()
        }),
    )
    .with_geometry(0.0, 0.0, SIZE as f32, SIZE as f32);
    let doc = page_doc(node);
    let rgba = render(g.as_mut().unwrap(), &doc, &cache);
    near(px(&rgba, 5, 20), RED, "first column");
    near(px(&rgba, 25, 20), BLUE, "third column");
}

#[test]
fn missing_and_broken_assets_draw_a_placeholder() {
    let Some(mut g) = gpu() else { return };
    let (assets, _, cache) = setup();
    let missing = AssetRef("0123456789ab.png".into());
    let broken = assets.add(b"definitely not a png".to_vec(), "png");
    for a in [missing, broken] {
        let doc = page_doc(image_node(&a, ImageFit::Fill, CornerRadius::px(0.0)));
        let rgba = render(g.as_mut().unwrap(), &doc, &cache);
        // Pale red box, off the cross diagonals.
        near(
            px(&rgba, 20, 5),
            [0xfd, 0xe8, 0xe8, 0xff],
            "placeholder body",
        );
        let dark_red = (0..SIZE).any(|i| px(&rgba, i, i)[0] > 200 && px(&rgba, i, i)[1] < 100);
        assert!(dark_red, "placeholder cross is visible");
    }
}

#[test]
fn each_asset_is_decoded_once() {
    let (_, a, cache) = setup();
    let doc = page_doc(image_node(&a, ImageFit::Fill, CornerRadius::px(0.0)));
    let geom = LayoutEngine::new().compute(&doc);
    let first = page_scene_with_images(&doc, &geom, &doc.pages[0], &cache);
    let second = page_scene_with_images(&doc, &geom, &doc.pages[0], &cache);
    drop((first, second));
    assert_eq!(cache.len(), 1);
}
