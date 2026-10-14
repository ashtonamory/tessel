//! Browser parity of the HTML export (§10 M5 exit criteria): every layout
//! fixture (`crates/layout/tests/fixtures`, plus this crate's code
//! fixtures) is exported as HTML + CSS, opened in headless Chromium, and
//! each element's box is compared with the geometry `layout` computes for
//! its node.
//!
//! Opt-in (§11: a headless browser is a property of a machine, not of the
//! project): plain `cargo test` prints a skip line and passes. Run it with
//! `scripts/code-parity.sh`, or `CODE_PARITY=1 cargo test --offline -p
//! export --test code_parity -- --nocapture`. Once opted in it is strict:
//! no Chromium, a page Chromium produced no output for, zero boxes checked
//! or any mismatch fails.
//!
//! The pages are served by this test (a small HTTP server on a loopback
//! port) and loaded with `chromium --headless --dump-dom`. Chromium is
//! `$CHROMIUM`, `chromium`, `chromium-browser` or `google-chrome`. Set
//! `CODE_PARITY_KEEP=1` to keep the pages (the path is printed) for a look
//! in a normal browser.
//!
//! Text boxes depend on the browser's and parley's font metrics, so text
//! nodes get a looser tolerance; boxes placed after text in a flow move
//! with it, so a fixture whose text the browser measures differently only
//! reports its mismatches.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use doc::{Document, Id, IdKind, Node, NodeKind, Page, Tokens};
use export::{export_code, CodeFormat, CodeOptions};
use layout::{Geometry, LayoutEngine};
use serde::Deserialize;

const TOLERANCE: f64 = 0.5;
const TEXT_TOLERANCE: f64 = 3.0;

#[derive(Deserialize)]
struct Fixture {
    #[serde(default)]
    tokens: Tokens,
    nodes: Vec<Node>,
}

fn layout_fixtures() -> Vec<(String, Document)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../layout/tests/fixtures");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("layout fixtures")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let f: Fixture = serde_json::from_str(&fs::read_to_string(&p).unwrap())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Fixture");
            page.children = f.nodes;
            let mut doc = Document::new(name.clone(), page);
            doc.tokens = f.tokens;
            doc.rebuild_index();
            (name, doc)
        })
        .collect()
}

fn code_fixtures() -> Vec<(String, Document)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    ["code_card", "code_constraints"]
        .into_iter()
        .map(|name| {
            // Images are not part of the comparison; any asset name will do.
            let text = fs::read_to_string(dir.join(format!("{name}.tessel.json")))
                .unwrap()
                .replace("ASSET", "000000000000.png");
            (name.to_string(), doc::load_str(&text).unwrap().doc)
        })
        .collect()
}

fn chromium() -> Option<String> {
    let mut candidates: Vec<String> = std::env::var("CHROMIUM").into_iter().collect();
    candidates.extend(["chromium", "chromium-browser", "google-chrome"].map(String::from));
    candidates.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Serve `dir` on a loopback port for as long as the test runs.
fn serve(dir: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/");
            let file = dir.join(path.trim_start_matches('/'));
            let resp = match fs::read(&file) {
                Ok(body) if !path.contains("..") => {
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    r.extend(body);
                    r
                }
                _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            };
            let _ = stream.write_all(&resp);
        }
    });
    port
}

/// The exported page with its stylesheet inlined and a script that writes
/// `FAIL <id> want … got …` / `TEXT <id> …` lines into `<pre id="parity">`.
fn page(doc: &Document, geom: &Geometry, root: &Node) -> Option<String> {
    let opts = CodeOptions {
        file: "parity",
        node_ids: true,
        ..Default::default()
    };
    let files = export_code(doc, geom, &root.id, CodeFormat::Html, &opts).unwrap();
    let html = files.iter().find(|f| f.path == "index.html")?.as_text()?;
    let css = files.iter().find(|f| f.path == "styles.css")?.as_text()?;
    let origin = geom.get(&root.id)?.bounds;
    let mut expected = BTreeMap::new();
    let mut text = Vec::new();
    root.walk(&mut |n| {
        if let Some(g) = geom.get(&n.id) {
            let b = g.bounds;
            expected.insert(
                n.id.to_string(),
                [b.x0 - origin.x0, b.y0 - origin.y0, b.width(), b.height()],
            );
        }
        if matches!(n.kind, NodeKind::Text(_)) {
            text.push(n.id.to_string());
        }
    });
    let expected: Vec<String> = expected
        .iter()
        .map(|(id, b)| format!("[{id:?}, [{}, {}, {}, {}]]", b[0], b[1], b[2], b[3]))
        .collect();
    let script = format!(
        r#"<pre id="parity"></pre>
<script>
const EXPECTED = [{expected}];
const TEXT = new Set({text:?});
const root = document.querySelector('[data-node="{root}"]').getBoundingClientRect();
const lines = [];
for (const [id, want] of EXPECTED) {{
  const el = document.querySelector(`[data-node="${{id}}"]`);
  if (!el) continue; // flattened groups have no element
  const r = el.getBoundingClientRect();
  const got = [r.left - root.left, r.top - root.top, r.width, r.height];
  const tol = TEXT.has(id) ? {text_tol} : {tol};
  if (!got.every((v, i) => Math.abs(v - want[i]) <= tol)) {{
    lines.push(`${{TEXT.has(id) ? "TEXT" : "FAIL"}} ${{id}} want ${{want.map((v) => +v.toFixed(2))}} got ${{got.map((v) => +v.toFixed(2))}}`);
  }}
  lines.push(`CHECKED ${{id}}`);
}}
document.getElementById("parity").textContent = lines.join("\n");
</script>
"#,
        expected = expected.join(", "),
        root = root.id,
        tol = TOLERANCE,
        text_tol = TEXT_TOLERANCE,
    );
    Some(
        html.replace(
            "<link rel=\"stylesheet\" href=\"styles.css\">",
            &format!("<style>\n{css}</style>"),
        )
        .replace("</body>", &format!("{script}</body>")),
    )
}

#[test]
fn html_export_matches_layout_in_a_browser() {
    if std::env::var_os("CODE_PARITY").is_none_or(|v| v.is_empty() || v == "0") {
        eprintln!(
            "browser parity check skipped; run scripts/code-parity.sh (CODE_PARITY=1) to enable it"
        );
        return;
    }
    let chromium = chromium().expect(
        "CODE_PARITY is set but no Chromium was found ($CHROMIUM, chromium, chromium-browser, google-chrome)",
    );
    let dir = std::env::temp_dir().join(format!("god-code-parity-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let port = serve(dir.clone());

    let mut fixtures = layout_fixtures();
    fixtures.extend(code_fixtures());
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, doc) in &fixtures {
        let geom = LayoutEngine::new().compute(doc);
        for (i, root) in doc.pages[0].children.iter().enumerate() {
            let Some(html) = page(doc, &geom, root) else {
                continue;
            };
            let file = format!("{name}-{i}.html");
            fs::write(dir.join(&file), html).unwrap();
            let out = Command::new(&chromium)
                .args([
                    "--headless",
                    "--no-sandbox",
                    "--disable-gpu",
                    "--hide-scrollbars",
                    "--window-size=1600,1200",
                    "--dump-dom",
                ])
                .arg(format!("http://127.0.0.1:{port}/{file}"))
                .output()
                .expect("run chromium");
            let dom = String::from_utf8_lossy(&out.stdout);
            let Some(result) = dom
                .split("<pre id=\"parity\">")
                .nth(1)
                .and_then(|s| s.split("</pre>").next())
            else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
                failures.push(format!(
                    "{file}: Chromium produced no result ({}; {} bytes of DOM). It may be unable to render here (sandboxed?). Last stderr lines:\n    {}",
                    out.status,
                    out.stdout.len(),
                    tail.into_iter().rev().collect::<Vec<_>>().join("\n    ")
                ));
                continue;
            };
            let text_off = result.lines().any(|l| l.starts_with("TEXT"));
            for l in result.lines() {
                if l.starts_with("CHECKED") {
                    checked += 1;
                } else if l.starts_with("FAIL") && !text_off {
                    failures.push(format!("{file}: {l}"));
                } else if !l.is_empty() {
                    eprintln!("{file}: {l} (text measured differently; not counted)");
                }
            }
        }
    }
    if std::env::var_os("CODE_PARITY_KEEP").is_some() {
        eprintln!("pages kept in {}", dir.display());
    } else {
        let _ = fs::remove_dir_all(&dir);
    }
    eprintln!("checked {checked} boxes");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    assert!(checked > 50, "only {checked} boxes were checked");
}
