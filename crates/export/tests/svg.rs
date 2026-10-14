//! Golden SVG tests. Regenerate with `UPDATE_GOLDEN=1 cargo test -p export`
//! and review the diff. Text positions depend on the installed fonts, so the
//! text golden masks `y` values (and the test checks only structure).

use std::fs;
use std::path::PathBuf;

use doc::{Assets, Id, IdKind, NodeId};
use export::{export_svg, SvgOptions};
use layout::LayoutEngine;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn node(n: u64) -> NodeId {
    Id::from_entropy(IdKind::Node, n)
}

/// A 2x2 PNG with two colors.
fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 2, 2);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(&[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ])
        .unwrap();
    out
}

fn svg_of(fixture: &str, root: u64) -> String {
    let assets = Assets::new();
    let asset = assets.add(tiny_png(), "png");
    let text = fs::read_to_string(
        dir()
            .join("fixtures")
            .join(format!("{fixture}.tessel.json")),
    )
    .unwrap()
    .replace("ASSET", &asset.0);
    let loaded = doc::load_str(&text).unwrap();
    assert!(
        loaded.problems.is_empty(),
        "{fixture}: {:?}",
        loaded.problems
    );
    let geom = LayoutEngine::new().compute(&loaded.doc);
    export_svg(
        &loaded.doc,
        &geom,
        &node(root),
        &SvgOptions {
            file: "test.tessel.json",
            revision: 7,
            assets: Some(&assets),
        },
    )
    .unwrap()
}

fn check(name: &str, svg: &str) {
    let path = dir().join("golden").join(format!("{name}.svg"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(&path, svg).unwrap();
        return;
    }
    let want = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(svg, want, "{name}: output differs from {}", path.display());
}

fn golden(name: &str, root: u64) {
    let svg = svg_of(name, root);
    // Deterministic: a second export is byte-identical.
    assert_eq!(svg, svg_of(name, root));
    check(name, &svg);
}

#[test]
fn shapes() {
    golden("shapes", 1);
}

#[test]
fn gradients() {
    golden("gradients", 1);
}

#[test]
fn clip_frame() {
    golden("clip_frame", 1);
}

#[test]
fn image() {
    golden("image", 1);
}

#[test]
fn rotated() {
    golden("rotated", 1);
}

#[test]
fn tokens_and_styles() {
    golden("tokens", 1);
}

#[test]
fn nested_auto_layout() {
    golden("auto_layout", 1);
}

#[test]
fn instances_draw_as_their_expansion() {
    golden("code_components", 1);
    let svg = svg_of("code_components", 1);
    // Derived IDs, the overridden fill and the swapped icon all show.
    assert!(svg.contains("id=\"n_0000000002/n_0000000106\""), "{svg}");
    assert!(svg.contains("#dc2626"), "{svg}");
    assert!(!svg.contains("#eeeeee"), "{svg}"); // no instance placeholder
}

#[test]
fn exporting_a_nested_node_uses_its_bounds() {
    let svg = svg_of("auto_layout", 3);
    assert!(svg.contains("node=\"n_0000000003\""), "{svg}");
    assert!(svg.contains("viewBox=\"116 "), "{svg}");
    assert!(!svg.contains("n_0000000002"));
}

#[test]
fn text_uses_layout_lines() {
    let svg = svg_of("text", 1);
    // y depends on the font's metrics: mask it for the golden.
    let masked: String = svg
        .lines()
        .map(|l| match l.find(" y=\"") {
            Some(i) if l.contains("<tspan") => {
                let rest = &l[i + 4..];
                let end = rest.find('"').unwrap();
                format!("{} y=\"#{}\n", &l[..i], &rest[end..])
            }
            _ => format!("{l}\n"),
        })
        .collect();
    assert!(svg.contains("Hello &lt;world&gt;"), "{svg}");
    assert!(svg.contains("Second line"), "{svg}");
    assert_eq!(svg.matches("<tspan").count(), 2);
    check("text", &masked);
}

#[test]
fn unknown_node_is_an_error() {
    let svg = svg_of("rotated", 1);
    assert!(svg.starts_with("<?xml"));
    let loaded =
        doc::load_str(&fs::read_to_string(dir().join("fixtures/rotated.tessel.json")).unwrap())
            .unwrap();
    let geom = LayoutEngine::new().compute(&loaded.doc);
    let opts = SvgOptions {
        file: "f",
        revision: 0,
        assets: None,
    };
    assert!(export_svg(&loaded.doc, &geom, &node(99), &opts).is_err());
}
