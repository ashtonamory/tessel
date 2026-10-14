//! PNG export tests. They need a GPU adapter and skip with a message when
//! there is none (CI and sandboxes may lack Vulkan), per SPEC §11.

use doc::{
    ColorRef, CornerRadius, Document, Frame, Id, IdKind, Node, NodeId, NodeKind, Page, Paint, Rect,
    Shape,
};
use export::{export_png, GpuContext, PngOptions};
use layout::LayoutEngine;
use render::{ImageCache, SceneRenderer};

fn id(n: u64) -> NodeId {
    Id::from_entropy(IdKind::Node, n)
}

fn filled(color: &str) -> Shape {
    Shape {
        fills: vec![Paint::solid(ColorRef::literal(color))],
        ..Default::default()
    }
}

/// A 100x50 rounded blue frame holding a red 20x20 rect at (10, 10).
fn document() -> Document {
    let mut frame = Node::new(
        id(1),
        "frame",
        NodeKind::Frame(Frame {
            shape: Shape {
                corner_radius: CornerRadius::px(20.0),
                ..filled("#0000ff")
            },
            ..Default::default()
        }),
    )
    .with_geometry(300.0, 200.0, 100.0, 50.0);
    frame.children_mut().unwrap().push(
        Node::new(
            id(2),
            "red",
            NodeKind::Rect(Rect {
                shape: filled("#ff0000"),
                ..Default::default()
            }),
        )
        .with_geometry(30.0, 10.0, 20.0, 20.0),
    );
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    page.children = vec![frame];
    Document::new("t", page)
}

fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    (info.width, info.height, buf[..info.buffer_size()].to_vec())
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn near(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
}

#[test]
fn exports_scaled_png_with_and_without_background() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("skipping PNG export test: no GPU adapter");
        return;
    };
    let Ok((device, queue)) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
    else {
        eprintln!("skipping PNG export test: no device");
        return;
    };
    let Ok(mut renderer) = SceneRenderer::new(&device) else {
        eprintln!("skipping PNG export test: vello cannot run here");
        return;
    };

    let doc = document();
    let geom = LayoutEngine::new().compute(&doc);
    let images = ImageCache::default();
    let mut gpu = GpuContext {
        device: &device,
        queue: &queue,
        renderer: &mut renderer,
        images: &images,
    };

    // 2x, transparent: 200x100, rounded corner empty, body blue, rect red.
    let bytes = export_png(
        &doc,
        &geom,
        &id(1),
        &PngOptions {
            scale: 2.0,
            background: None,
        },
        &mut gpu,
    )
    .unwrap();
    let (w, h, px) = decode(&bytes);
    assert_eq!((w, h), (200, 100));
    assert_eq!(pixel(&px, w, 0, 0)[3], 0, "corner outside the radius");
    assert!(near(pixel(&px, w, 100, 90), [0, 0, 255, 255]));
    assert!(near(pixel(&px, w, 80, 40), [255, 0, 0, 255]));

    // 1x with a background shows through the rounded corner.
    let bytes = export_png(
        &doc,
        &geom,
        &id(1),
        &PngOptions {
            scale: 1.0,
            background: Some("#00ff00".into()),
        },
        &mut gpu,
    )
    .unwrap();
    let (w, h, px) = decode(&bytes);
    assert_eq!((w, h), (100, 50));
    assert!(near(pixel(&px, w, 0, 0), [0, 255, 0, 255]));
    assert!(near(pixel(&px, w, 50, 40), [0, 0, 255, 255]));

    // Fractional scale, and bad input.
    let bytes = export_png(
        &doc,
        &geom,
        &id(1),
        &PngOptions {
            scale: 1.5,
            background: None,
        },
        &mut gpu,
    )
    .unwrap();
    let (w, h, _) = decode(&bytes);
    assert_eq!((w, h), (150, 75));
    let bad = PngOptions {
        scale: 1.0,
        background: Some("green".into()),
    };
    assert!(export_png(&doc, &geom, &id(1), &bad, &mut gpu).is_err());
    assert!(export_png(&doc, &geom, &id(9), &PngOptions::default(), &mut gpu).is_err());
}
