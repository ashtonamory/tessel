//! The Export window, headless: open it with its shortcut, check the file
//! list for the selected node and save to a folder.

mod common;

use app::export_window::Kind;
use app::{DesignApp, Editor};
use common::Harness;
use doc::{Id, IdKind};
use egui::{Key, Modifiers};
use export::{TailwindVersion, TokenFormat};

fn node(n: u64) -> doc::NodeId {
    Id::from_entropy(IdKind::Node, n)
}

fn components_harness() -> Harness {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../export/tests/fixtures/code_components.tessel.json");
    let ed = Editor::open(&path).expect("fixture opens");
    Harness::with_app(eframe::Frame::_new_kittest(), DesignApp::new(ed))
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("god-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shortcut_opens_the_window_and_lists_the_files() {
    let mut h = components_harness();
    assert!(!h.app.export_window().open);
    h.key(Key::E, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(h.app.export_window().open);

    // Nothing selected: nothing to export.
    assert!(h.app.export_files().is_err());

    h.app.editor_mut().selection = vec![node(2)];
    h.step(vec![]);
    let files = h.app.export_files().unwrap();
    let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(paths, ["index.html", "styles.css"]);

    for (kind, want) in [
        (
            Kind::Tailwind(TailwindVersion::V3),
            vec!["index.html", "styles.css", "tailwind.config.js"],
        ),
        (Kind::Svg, vec!["pay.svg"]),
        (Kind::Png(2), vec!["pay@2x.png"]),
    ] {
        h.app.export_window_mut().kind = kind;
        let paths: Vec<_> = h
            .app
            .export_files()
            .unwrap()
            .iter()
            .map(|f| f.path.clone())
            .collect();
        assert_eq!(paths, want, "{kind:?}");
    }

    // The shortcut again closes it.
    h.key(Key::E, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(!h.app.export_window().open);
}

#[test]
fn saving_writes_every_file_with_instances_expanded() {
    let mut h = components_harness();
    h.key(Key::E, Modifiers::COMMAND | Modifiers::SHIFT);
    h.app.editor_mut().selection = vec![node(1)];
    h.step(vec![]);

    let dir = temp_dir("html");
    let Harness { app, frame, .. } = &mut h;
    app.export_save_to(frame, &dir);
    let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
    assert!(html.contains("Save"), "{html}");
    assert!(html.contains("Delete"), "{html}");
    assert!(dir.join("styles.css").is_file());
    let status = app.editor().status.clone().unwrap();
    assert!(status.starts_with("Exported 2 file(s)"), "{status}");

    // SVG draws the same instances, not placeholders.
    app.export_window_mut().kind = Kind::Svg;
    let dir = temp_dir("svg");
    app.export_save_to(frame, &dir);
    let svg = std::fs::read_to_string(dir.join("checkout.svg")).unwrap_or_else(|_| {
        let f = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap();
        std::fs::read_to_string(f.path()).unwrap()
    });
    assert!(svg.contains("n_0000000003/n_0000000106"), "{svg}");

    // React with components writes a file per component.
    app.export_window_mut().kind = Kind::React;
    let dir = temp_dir("react");
    app.export_save_to(frame, &dir);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n.ends_with(".tsx")), "{names:?}");
}

#[test]
fn png_without_a_gpu_reports_the_failure() {
    let mut h = components_harness();
    h.app.editor_mut().selection = vec![node(1)];
    h.app.export_window_mut().kind = Kind::Png(2);
    let dir = temp_dir("png");
    let Harness { app, frame, .. } = &mut h;
    app.export_save_to(frame, &dir);
    assert!(app
        .editor()
        .status
        .as_deref()
        .unwrap()
        .starts_with("Export failed"));
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
}

#[test]
fn exporting_tokens_writes_the_chosen_format() {
    let mut h = components_harness();
    let dir = temp_dir("tokens");
    for (format, name, needle) in [
        (TokenFormat::Css, "tokens.css", "--color-primary"),
        (TokenFormat::TailwindV4, "theme.css", "@theme"),
        (TokenFormat::TailwindV3, "tailwind.config.js", "primary"),
    ] {
        h.app.export_tokens_to(format, &dir.join(name));
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        assert!(text.contains(needle), "{name}: {text}");
    }
}
