//! `tessel render` on a real GPU: skipped where none can render, like the
//! other GPU tests.

use headless::{default_node, load, render, Format, Gpu, RenderOptions};

const DOC: &str = r##"{
  "format": { "name": "tessel", "version": 1 },
  "meta": { "title": "Post" },
  "pages": [{ "id": "p_page000001", "name": "Page", "children": [
    { "id": "n_post000001", "name": "Design", "kind": "frame",
      "x": 0, "y": 0, "width": 100, "height": 80,
      "fills": [{ "type": "solid", "color": "#2563eb" }],
      "children": [
        { "id": "n_dot0000001", "name": "Dot", "kind": "rect",
          "x": 0, "y": 0, "width": 50, "height": 40,
          "fills": [{ "type": "solid", "color": "#ff0000" }] }
      ]}
  ]}]
}"##;

#[test]
fn renders_png_and_jpeg_with_a_matching_layer_map() {
    let dir = std::env::temp_dir().join(format!("tessel-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("post.tessel.json");
    std::fs::write(&file, DOC).unwrap();
    let (doc, assets) = load(&file).unwrap();
    assert_eq!(default_node(&doc).unwrap().to_string(), "n_post000001");

    // SVG needs no GPU.
    let svg = render(
        &doc,
        &assets,
        &RenderOptions {
            format: Format::Svg,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(String::from_utf8(svg.bytes).unwrap().contains("<svg"));

    let Ok(mut gpu) = Gpu::open() else {
        eprintln!("skipping: no GPU can render here");
        return;
    };
    let png = render(
        &doc,
        &assets,
        &RenderOptions {
            scale: 2.0,
            ..Default::default()
        },
        Some(&mut gpu),
    )
    .unwrap();
    let img = image::load_from_memory(&png.bytes).unwrap().to_rgba8();
    assert_eq!(
        (img.width(), img.height()),
        (png.layers.width, png.layers.height)
    );
    assert_eq!((img.width(), img.height()), (200, 160));
    assert_eq!(
        img.get_pixel(20, 20).0,
        [255, 0, 0, 255],
        "the dot, top left"
    );
    assert_eq!(
        img.get_pixel(180, 140).0,
        [37, 99, 235, 255],
        "the frame's blue"
    );
    let dot = &png.layers.layers[1];
    assert_eq!(
        (dot.x, dot.y, dot.width, dot.height),
        (0.0, 0.0, 100.0, 80.0)
    );

    let jpeg = render(
        &doc,
        &assets,
        &RenderOptions {
            format: Format::Jpeg,
            ..Default::default()
        },
        Some(&mut gpu),
    )
    .unwrap();
    let img = image::load_from_memory_with_format(&jpeg.bytes, image::ImageFormat::Jpeg).unwrap();
    assert_eq!((img.width(), img.height()), (100, 80));
}
