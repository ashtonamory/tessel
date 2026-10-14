//! Exported code from documents whose strings are hostile: nothing from the
//! document may break out of the string, comment or markup it lands in.

use std::fs;
use std::path::PathBuf;

use doc::{Assets, Id, IdKind, NodeId};
use export::{export_code, export_tokens, CodeFormat, CodeOptions, OutFile, TokenFormat};
use layout::LayoutEngine;

/// Template literal, comment, markup and line-break breakouts in one string.
const HOSTILE: &str = "x`+alert(1)+`${evil}\n*/ INJECTED_LINE --> --!> <script>alert(2)</script>\u{2028}\r INJECTED_LINE2 \\ ' \" <b>";

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn node(n: u64) -> NodeId {
    Id::from_entropy(IdKind::Node, n)
}

fn json(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn load(fixture: &str, replace: &[(&str, String)]) -> doc::Document {
    let mut text = fs::read_to_string(
        dir()
            .join("fixtures")
            .join(format!("{fixture}.tessel.json")),
    )
    .unwrap();
    for (from, to) in replace {
        assert!(text.contains(from), "{fixture}: {from}");
        text = text.replace(from, to);
    }
    doc::load_str(&text).unwrap().doc
}

/// JSON string contents, without the quotes.
fn inner(s: &str) -> String {
    let j = json(s);
    j[1..j.len() - 1].to_string()
}

/// The text without its comments (`/* */`, `<!-- -->` and `//` lines) and
/// the (escaped) text of `<title>`.
fn without_comments(path: &str, text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let kinds: &[(&str, &str)] = match path.rsplit_once('.') {
            Some((_, "css")) => &[("/*", "*/")],
            Some((_, "html")) => &[("<!--", "-->"), ("<title>", "</title>")],
            _ => &[],
        };
        let next = kinds
            .iter()
            .filter_map(|(open, close)| rest.find(open).map(|i| (i, *open, *close)))
            .min();
        match next {
            Some((i, open, close)) => {
                out.push_str(&rest[..i]);
                let after = &rest[i + open.len()..];
                let end = after
                    .find(close)
                    .unwrap_or_else(|| panic!("comment is not closed: {after}"));
                rest = &after[end + close.len()..];
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn texts(files: &[OutFile]) -> Vec<(&str, &str)> {
    files
        .iter()
        .filter_map(|f| Some((f.path.as_str(), f.as_text()?)))
        .collect()
}

/// What no output may contain, whatever the format.
fn assert_contained(what: &str, path: &str, text: &str) {
    let at = format!("{what} {path}");
    let code = without_comments(path, text);
    for bad in [
        "alert(1)", "alert(2)", "<script", "<b>", "${evil}", "`+", "\u{2028}", "\r", "../",
    ] {
        assert!(!code.contains(bad), "{at}: contains {bad:?}\n{text}");
    }
    for line in text.lines() {
        assert!(
            !line.trim_start().starts_with("INJECTED_LINE"),
            "{at}: injected line {line:?}"
        );
    }
    assert!(!text.contains("\u{2028}") && !text.contains('\r'), "{at}");
    // Comments close only where the exporter closes them.
    if path.ends_with(".css") {
        assert_eq!(
            text.matches("/*").count(),
            text.matches("*/").count(),
            "{at}"
        );
    }
    if path.ends_with(".html") {
        assert_eq!(
            text.matches("<!--").count(),
            text.matches("-->").count(),
            "{at}"
        );
        assert!(!text.contains("--!>"), "{at}");
    }
    // `${` only where a template literal of the exporter's own opens one.
    if path.ends_with(".tsx") {
        for l in text.lines().filter(|l| l.contains("className={`")) {
            assert!(l.matches('`').count() == 2, "{at}: {l}");
        }
    }
}

fn export_all(d: &doc::Document, root: u64, what: &str) {
    let geom = LayoutEngine::new().compute(d);
    let assets = Assets::new();
    for format in [CodeFormat::Html, CodeFormat::Tailwind, CodeFormat::React] {
        for (tokens, components, v3) in [
            (export::TokenMode::CssVars, true, false),
            (export::TokenMode::Inline, false, true),
        ] {
            let opts = CodeOptions {
                file: HOSTILE,
                revision: 1,
                tokens,
                include_components: components,
                tailwind: if v3 {
                    export::TailwindVersion::V3
                } else {
                    export::TailwindVersion::V4
                },
                assets: Some(&assets),
                node_ids: true,
                ..Default::default()
            };
            let files = export_code(d, &geom, &node(root), format, &opts).unwrap();
            for (path, text) in texts(&files) {
                assert_contained(&format!("{what} {format:?} v3={v3}"), path, text);
            }
            for f in &files {
                assert!(!f.path.contains(".."), "{}", f.path);
            }
        }
    }
    for format in [
        TokenFormat::Css,
        TokenFormat::TailwindV4,
        TokenFormat::TailwindV3,
    ] {
        let f = export_tokens(d, format);
        assert_contained(what, &f.path, f.as_text().unwrap());
    }
}

#[test]
fn hostile_strings_in_a_card_do_not_break_out() {
    let token = HOSTILE.replace(['{', '}'], "");
    let d = load(
        "code_card",
        &[
            ("\"Pricing\"", json(HOSTILE)),
            ("\"Inter\"", json(HOSTILE)),
            ("\"Pricing Card\"", json(HOSTILE)),
            ("\"Title\"", json(HOSTILE)),
            ("\"Heading/H1\"", json(HOSTILE)),
            ("color.primary", inner(&token)),
            ("space.4", inner(&token)),
            ("ASSET", inner(&format!("../../{HOSTILE}.png"))),
            ("M12 2L15 9H22", inner(&format!("M12 2{HOSTILE}"))),
        ],
    );
    export_all(&d, 1, "card");
}

#[test]
fn hostile_component_names_do_not_break_out() {
    let d = load(
        "code_components",
        &[
            ("\"Button/Primary\"", json(HOSTILE)),
            ("\"Icon/Star\"", json(HOSTILE)),
            ("\"Show icon\"", json(HOSTILE)),
        ],
    );
    export_all(&d, 1, "components");
}
