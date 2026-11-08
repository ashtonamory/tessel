//! The preview (§13.2): a top-level frame's HTML + CSS export, served on
//! `127.0.0.1` to a browser window, reloaded after each change.
//!
//! The preview is the export: the page in the browser is exactly what
//! `export_code` returns, so what the user sees is what the agent builds
//! from. A small shell page that Tessel owns wraps it in an `<iframe>` and
//! adds the viewport presets; the export itself is never modified.

mod server;
mod shot;

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use doc::{Assets, Document, NodeId, ParentRef};
use export::{CodeFormat, CodeOptions, FileContent};
use layout::Geometry;

pub use server::Server;
pub use shot::{headless_browser, screenshot, ShotError, ShotOptions, MAX_SIDE};

/// One published export.
#[derive(Clone, Debug, Default)]
pub struct Page {
    /// The document revision it was made from.
    pub revision: u64,
    /// The frame's layer name.
    pub title: String,
    /// The frame's design size, px.
    pub width: f32,
    pub height: f32,
    /// Files by path relative to the export (`index.html`, `styles.css`,
    /// `assets/…`).
    pub files: HashMap<String, Vec<u8>>,
}

/// The top-level node `id` sits under (itself if it is one).
pub fn top_level_of(doc: &Document, id: &NodeId) -> Option<NodeId> {
    let mut cur = id.clone();
    loop {
        match doc.parent_of(&cur)? {
            (ParentRef::Page(_), _) => return Some(cur),
            (ParentRef::Node(parent), _) => cur = parent,
        }
    }
}

/// What to preview: the top-level node holding the first selected node,
/// else the page's first frame, else its first node.
pub fn pick_frame(doc: &Document, page: &doc::Page, selection: &[NodeId]) -> Option<NodeId> {
    if let Some(top) = selection.iter().find_map(|id| top_level_of(doc, id)) {
        return Some(top);
    }
    page.children
        .iter()
        .find(|n| matches!(n.kind, doc::NodeKind::Frame(_)))
        .or(page.children.first())
        .map(|n| n.id.clone())
}

/// Export `id` as the page the preview shows: HTML + CSS with image files,
/// laid out to the viewport's width up to the design width (`responsive`).
pub fn export_page(
    doc: &Document,
    geom: &Geometry,
    assets: &Assets,
    id: &NodeId,
    file: &str,
    revision: u64,
) -> Result<Page, String> {
    let node = doc
        .node_or_master(id)
        .ok_or_else(|| format!("layer {id} is not in the document"))?;
    let opts = CodeOptions {
        file,
        revision,
        responsive: true,
        assets: Some(assets),
        ..Default::default()
    };
    let files = export::export_code(doc, geom, id, CodeFormat::Html, &opts)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|f| {
            let bytes = match f.content {
                FileContent::Text(s) => s.into_bytes(),
                FileContent::Binary(b) => b,
            };
            (f.path, bytes)
        })
        .collect();
    let (width, height) = geom
        .get(id)
        .map(|g| {
            let r = g.local_rect();
            (r.width() as f32, r.height() as f32)
        })
        .unwrap_or((node.width, node.height));
    Ok(Page {
        revision,
        title: node.name.clone(),
        width,
        height,
        files,
    })
}

/// Browsers tried for the preview window, in order. Each is Chromium-based,
/// so `--app` gives a window without tabs or an address bar.
const BROWSERS: [&str; 5] = [
    "chromium",
    "google-chrome-stable",
    "google-chrome",
    "chromium-browser",
    "brave",
];

/// Find `name` on `PATH`.
fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// Open `url` in a window of its own: a Chromium-based browser in app mode,
/// with its own profile in `profile_dir`, else the default browser through
/// `xdg-open`. Returns the program used.
pub fn open_window(url: &str, profile_dir: &Path) -> std::io::Result<String> {
    let (program, mut cmd) = match BROWSERS.iter().find_map(|b| which(b).map(|p| (b, p))) {
        Some((name, path)) => {
            let mut cmd = Command::new(path);
            cmd.arg(format!("--app={url}"))
                .arg(format!("--user-data-dir={}", profile_dir.display()))
                .args(["--no-first-run", "--no-default-browser-check"]);
            (name.to_string(), cmd)
        }
        None => {
            let mut cmd = Command::new("xdg-open");
            cmd.arg(url);
            ("xdg-open".to_string(), cmd)
        }
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reap it when it exits, so it does not linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kitchen_sink() -> Document {
        let text = include_str!("../../doc/tests/fixtures/kitchen_sink.tessel.json");
        doc::io::load_str(text).expect("fixture parses").doc
    }

    #[test]
    fn picks_the_frame_of_the_selection_or_the_first_frame() {
        let doc = kitchen_sink();
        let page = &doc.pages[0];
        let first = pick_frame(&doc, page, &[]).unwrap();
        assert_eq!(first, page.children[0].id);
        // A deep descendant resolves to its top-level frame.
        let mut deep = &page.children[0];
        while let Some(c) = deep.children().and_then(|c| c.first()) {
            deep = c;
        }
        assert_ne!(deep.id, first);
        assert_eq!(pick_frame(&doc, page, &[deep.id.clone()]), Some(first));
    }

    #[test]
    fn exports_the_frame_as_a_page() {
        let doc = kitchen_sink();
        let geom = layout::LayoutEngine::new().compute(&doc);
        let id = pick_frame(&doc, &doc.pages[0], &[]).unwrap();
        let page = export_page(&doc, &geom, &Assets::default(), &id, "k.tessel.json", 7).unwrap();
        assert_eq!(page.revision, 7);
        assert!(page.width > 0.0 && page.height > 0.0);
        let html = String::from_utf8(page.files["index.html"].clone()).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(page.files.contains_key("styles.css"));
    }

    #[test]
    fn screenshots_the_page_at_the_asked_size() {
        if headless_browser().is_none() {
            eprintln!("skipped: no headless Chromium");
            return;
        }
        let doc = kitchen_sink();
        let geom = layout::LayoutEngine::new().compute(&doc);
        let id = pick_frame(&doc, &doc.pages[0], &[]).unwrap();
        let page = export_page(&doc, &geom, &Assets::default(), &id, "k.tessel.json", 1).unwrap();
        let opts = ShotOptions {
            width: 400,
            height: 300,
            ..Default::default()
        };
        let png = screenshot(page, &opts).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((w, h), (400, 300));
    }
}
