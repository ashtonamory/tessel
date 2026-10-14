//! Layout fixtures (§10 M3, §11): documents under `tests/fixtures/*.json`
//! with the geometry they must lay out to, plus the HTML/CSS twin of each
//! under `tests/fixtures/html/` for the browser parity check
//! (`scripts/layout-parity.sh`).
//!
//! A fixture is `{ description, tokens?, text_measured?, nodes, expected }`:
//! `nodes` are the page's top-level nodes, `expected` maps node IDs to
//! `[x, y, width, height]` page bounds. Text depends on installed fonts, so a
//! fixture with text records the standalone size of each text node in
//! `text_measured`; it is skipped where fonts measure differently.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use doc::{Document, Id, IdKind, Node, Page, Tokens};
use layout::{parity, Geometry, LayoutEngine};
use serde::Deserialize;

const TOLERANCE: f64 = 0.01;

#[derive(Deserialize)]
struct Fixture {
    description: String,
    #[serde(default)]
    tokens: Tokens,
    #[serde(default)]
    text_measured: BTreeMap<String, [f64; 2]>,
    nodes: Vec<Node>,
    expected: BTreeMap<String, [f64; 4]>,
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load() -> Vec<(String, Fixture, Document)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir())
        .expect("fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&p).expect("read fixture");
            let f: Fixture =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: bad fixture: {e}"));
            let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Fixture");
            page.children = f.nodes.clone();
            let mut doc = Document::new(name.clone(), page);
            doc.tokens = f.tokens.clone();
            doc.rebuild_index();
            (name, f, doc)
        })
        .collect()
}

/// Whether this machine's fonts measure the fixture's text as recorded.
fn fonts_match(engine: &mut LayoutEngine, name: &str, f: &Fixture, doc: &Document) -> bool {
    for (id, want) in &f.text_measured {
        let node = doc
            .node(&Id::from_raw(id.as_str()))
            .unwrap_or_else(|| panic!("{name}: text_measured names unknown node {id}"));
        let got = engine.measure_text(doc, node).expect("a text node");
        if (got.width - want[0]).abs() > TOLERANCE || (got.height - want[1]).abs() > TOLERANCE {
            eprintln!(
                "{name}: skipped, fonts measure {id} as {}x{} (recorded {}x{})",
                got.width, got.height, want[0], want[1]
            );
            return false;
        }
    }
    true
}

fn mismatches(name: &str, f: &Fixture, g: &Geometry) -> Vec<String> {
    let mut out = Vec::new();
    for (id, want) in &f.expected {
        let Some(geom) = g.get(&Id::from_raw(id.as_str())) else {
            out.push(format!("{name}: {id}: no geometry"));
            continue;
        };
        let b = geom.bounds;
        let got = [b.x0, b.y0, b.width(), b.height()];
        if got.iter().zip(want).any(|(a, b)| (a - b).abs() > TOLERANCE) {
            out.push(format!("{name}: {id}: want {want:?}, got {got:?}"));
        }
    }
    out
}

#[test]
fn fixtures_lay_out_as_expected() {
    let fixtures = load();
    assert!(fixtures.len() >= 12, "expected at least 12 fixtures");
    let mut failures = Vec::new();
    for (name, f, doc) in &fixtures {
        let mut engine = LayoutEngine::new();
        if !fonts_match(&mut engine, name, f, doc) {
            continue;
        }
        let g = engine.compute(doc);
        failures.extend(mismatches(name, f, &g));
        // Incremental passes give the same result: touching every node
        // re-syncs everything, touching nothing re-syncs nothing.
        let all: BTreeSet<_> = doc.node_ids().cloned().collect();
        failures.extend(mismatches(
            &format!("{name} (update all)"),
            f,
            &engine.update(doc, &all),
        ));
        failures.extend(mismatches(
            &format!("{name} (update none)"),
            f,
            &engine.update(doc, &BTreeSet::new()),
        ));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The committed HTML twins match the fixtures. `LAYOUT_PARITY_WRITE=1`
/// (set by `scripts/layout-parity.sh`) rewrites them instead.
#[test]
fn html_twins_are_current() {
    let html_dir = dir().join("html");
    let write = std::env::var_os("LAYOUT_PARITY_WRITE").is_some();
    if write {
        std::fs::create_dir_all(&html_dir).expect("create html dir");
    }
    let fixtures = load();
    let mut names = Vec::new();
    let mut stale = Vec::new();
    for (name, f, doc) in &fixtures {
        let g = LayoutEngine::new().compute(doc);
        let expected: Vec<_> = f.expected.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let html = parity::fixture_html(name, &f.description, doc, &g, &expected);
        check(
            &html_dir.join(format!("{name}.html")),
            &html,
            write,
            &mut stale,
        );
        names.push(name.clone());
    }
    check(
        &html_dir.join("index.html"),
        &parity::index_html(&names),
        write,
        &mut stale,
    );
    assert!(
        stale.is_empty(),
        "HTML twins are out of date; run scripts/layout-parity.sh: {stale:?}"
    );
}

fn check(path: &Path, html: &str, write: bool, stale: &mut Vec<String>) {
    if write {
        std::fs::write(path, html).expect("write html");
    } else if std::fs::read_to_string(path).ok().as_deref() != Some(html) {
        stale.push(path.display().to_string());
    }
}
