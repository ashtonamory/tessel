//! Golden tests for code and token export (§8, §10 M5). Regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p export --test code` and review the diff.
//! Each golden is every text file of one export, concatenated.

use std::fs;
use std::path::PathBuf;

use doc::{Assets, Document, Id, IdKind, NodeId, NodeKind};
use export::{
    export_code, export_tokens, instance_children, CodeFormat, CodeOptions, FileContent, ImageMode,
    OutFile, TailwindVersion, TokenFormat, TokenMode,
};
use layout::LayoutEngine;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn node(n: u64) -> NodeId {
    Id::from_entropy(IdKind::Node, n)
}

fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 1, 1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(&[255, 0, 0, 255])
        .unwrap();
    out
}

fn load(fixture: &str) -> (Document, Assets) {
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
    (loaded.doc, assets)
}

fn concat(files: &[OutFile]) -> String {
    let mut s = String::new();
    for f in files {
        s.push_str(&format!("=== {} ({}) ===\n", f.path, f.language));
        match &f.content {
            FileContent::Text(t) => s.push_str(t),
            FileContent::Binary(b) => s.push_str(&format!("<{} bytes>\n", b.len())),
        }
    }
    s
}

fn check(name: &str, text: &str) {
    let path = dir()
        .join("golden")
        .join("code")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        return;
    }
    let want = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(text, want, "{name}: output differs from {}", path.display());
}

fn export(
    fixture: &str,
    root: u64,
    format: CodeFormat,
    tweak: impl Fn(&mut CodeOptions),
) -> Vec<OutFile> {
    let (doc, assets) = load(fixture);
    let geom = LayoutEngine::new().compute(&doc);
    let mut opts = CodeOptions {
        file: "test.tessel.json",
        revision: 7,
        assets: Some(&assets),
        ..Default::default()
    };
    tweak(&mut opts);
    let files = export_code(&doc, &geom, &node(root), format, &opts).unwrap();
    // Deterministic: a second export is identical.
    assert_eq!(
        files,
        export_code(&doc, &geom, &node(root), format, &opts).unwrap()
    );
    files
}

fn golden(
    name: &str,
    fixture: &str,
    root: u64,
    format: CodeFormat,
    tweak: impl Fn(&mut CodeOptions),
) {
    check(name, &concat(&export(fixture, root, format, tweak)));
}

#[test]
fn card_html() {
    golden("card_html", "code_card", 1, CodeFormat::Html, |_| {});
}

#[test]
fn card_html_inline() {
    golden("card_html_inline", "code_card", 1, CodeFormat::Html, |o| {
        o.tokens = TokenMode::Inline;
        o.images = ImageMode::DataUri;
        o.node_ids = true;
    });
}

#[test]
fn card_tailwind_v4() {
    golden(
        "card_tailwind_v4",
        "code_card",
        1,
        CodeFormat::Tailwind,
        |_| {},
    );
}

#[test]
fn card_tailwind_v3() {
    golden(
        "card_tailwind_v3",
        "code_card",
        1,
        CodeFormat::Tailwind,
        |o| {
            o.tailwind = TailwindVersion::V3;
        },
    );
}

#[test]
fn card_tailwind_inline() {
    golden(
        "card_tailwind_inline",
        "code_card",
        1,
        CodeFormat::Tailwind,
        |o| {
            o.tokens = TokenMode::Inline;
        },
    );
}

#[test]
fn card_react() {
    golden("card_react", "code_card", 1, CodeFormat::React, |_| {});
}

#[test]
fn constraints_html() {
    golden(
        "constraints_html",
        "code_constraints",
        1,
        CodeFormat::Html,
        |_| {},
    );
}

#[test]
fn constraints_tailwind() {
    golden(
        "constraints_tailwind",
        "code_constraints",
        1,
        CodeFormat::Tailwind,
        |_| {},
    );
}

#[test]
fn constraints_react_responsive() {
    golden(
        "constraints_react_responsive",
        "code_constraints",
        1,
        CodeFormat::React,
        |o| {
            o.responsive = true;
        },
    );
}

#[test]
fn components_html() {
    golden(
        "components_html",
        "code_components",
        1,
        CodeFormat::Html,
        |_| {},
    );
}

#[test]
fn components_tailwind() {
    golden(
        "components_tailwind",
        "code_components",
        1,
        CodeFormat::Tailwind,
        |_| {},
    );
}

#[test]
fn components_react() {
    golden(
        "components_react",
        "code_components",
        1,
        CodeFormat::React,
        |_| {},
    );
}

#[test]
fn components_react_expanded() {
    golden(
        "components_react_expanded",
        "code_components",
        1,
        CodeFormat::React,
        |o| {
            o.include_components = false;
        },
    );
}

#[test]
fn tokens() {
    let (doc, _) = load("code_card");
    for (name, format) in [
        ("tokens_css", TokenFormat::Css),
        ("tokens_tailwind_v4", TokenFormat::TailwindV4),
        ("tokens_tailwind_v3", TokenFormat::TailwindV3),
    ] {
        check(name, &concat(&[export_tokens(&doc, format)]));
    }
}

#[test]
fn image_files_are_written_once() {
    let files = export("code_card", 1, CodeFormat::Html, |_| {});
    let images: Vec<&OutFile> = files
        .iter()
        .filter(|f| f.path.starts_with("assets/"))
        .collect();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].language, "png");
    assert_eq!(images[0].content, FileContent::Binary(tiny_png()));
    let html = files[0].as_text().unwrap();
    assert!(
        html.contains(&format!("src=\"{}\"", images[0].path)),
        "{html}"
    );

    let inline = export("code_card", 1, CodeFormat::Html, |o| {
        o.images = ImageMode::DataUri
    });
    assert!(inline.iter().all(|f| !f.path.starts_with("assets/")));
    assert!(inline[0]
        .as_text()
        .unwrap()
        .contains("src=\"data:image/png;base64,"));
}

#[test]
fn unknown_node_is_an_error() {
    let (doc, _) = load("code_card");
    let geom = LayoutEngine::new().compute(&doc);
    let err = export_code(
        &doc,
        &geom,
        &node(999),
        CodeFormat::Html,
        &CodeOptions::default(),
    );
    assert!(err.is_err());
}

#[test]
fn a_component_master_can_be_exported() {
    let (doc, _) = load("code_components");
    let geom = LayoutEngine::new().compute(&doc);
    let master = Id::from_raw("n_0000000105");
    let files = export_code(
        &doc,
        &geom,
        &master,
        CodeFormat::Html,
        &CodeOptions::default(),
    )
    .unwrap();
    let html = files[0].as_text().unwrap();
    assert!(html.contains("<button class=\"button\">"), "{html}");
}

#[test]
fn instance_children_derive_ids_and_apply_overrides() {
    let (doc, _) = load("code_components");
    let save = doc.node(&node(3)).unwrap();
    let children = instance_children(&doc, save).unwrap();
    let ids: Vec<&str> = children.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "n_0000000003/n_0000000107",
            "n_0000000003/n_0000000106",
            "n_0000000003/n_0000000108"
        ]
    );
    match &children[0].kind {
        NodeKind::Instance(i) => assert_eq!(i.component.as_str(), "c_0000000002"),
        k => panic!("{k:?}"),
    }
    match &children[1].kind {
        NodeKind::Text(t) => assert_eq!(t.content, "Save"),
        k => panic!("{k:?}"),
    }
    assert!(children[2].visible);
    // Not an instance.
    assert!(instance_children(&doc, doc.node(&node(1)).unwrap()).is_none());
}
