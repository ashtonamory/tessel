//! Golden tests for the canonical file format (§4.3).
//!
//! Each fixture `X` has a hand-written `X.input.json` and its canonical form
//! `X.tessel.json`. To regenerate a canonical file after an intended format
//! change, run
//! `cargo run -p doc --example canon -- tests/fixtures/X.input.json > tests/fixtures/X.tessel.json`
//! (from `crates/doc`) and review the diff.

use std::fs;
use std::path::{Path, PathBuf};

const FIXTURES: [&str; 2] = ["spec_example", "kitchen_sink"];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str) -> String {
    fs::read_to_string(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn load_clean(text: &str, name: &str) -> doc::Document {
    let loaded = doc::load_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(loaded.problems.is_empty(), "{name}: {:?}", loaded.problems);
    loaded.doc
}

#[test]
fn canonical_files_round_trip_byte_identical() {
    for f in FIXTURES {
        let name = format!("{f}.tessel.json");
        let canonical = read(&name);
        let saved = doc::save_string(&load_clean(&canonical, &name));
        assert_eq!(saved, canonical, "{name}: save(load(file)) != file");
    }
}

#[test]
fn inputs_canonicalize_to_golden_and_stay_stable() {
    for f in FIXTURES {
        let input = format!("{f}.input.json");
        let first = doc::save_string(&load_clean(&read(&input), &input));
        assert_eq!(first, read(&format!("{f}.tessel.json")), "{input}");
        // save -> load -> save is byte-identical.
        let second = doc::save_string(&load_clean(&first, &input));
        assert_eq!(second, first, "{input}: second save differs");
    }
}

#[test]
fn loaded_input_equals_loaded_canonical() {
    for f in FIXTURES {
        let a = load_clean(&read(&format!("{f}.input.json")), f);
        let b = load_clean(&read(&format!("{f}.tessel.json")), f);
        // Inputs are written with values that survive 2-decimal rounding,
        // except the kitchen sink, which tests rounding on purpose.
        if f == "spec_example" {
            assert_eq!(a, b, "{f}");
        } else {
            assert_eq!(doc::save_string(&a), doc::save_string(&b), "{f}");
        }
    }
}

#[test]
fn spec_example_matches_the_spec() {
    let spec = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/SPEC.md"))
        .expect("docs/SPEC.md");
    let section = &spec[spec.find("### 4.2 Example").expect("§4.2 heading")..];
    let start = section.find("```json\n").expect("json block") + "```json\n".len();
    let end = start + section[start..].find("```\n").expect("end of block");
    assert_eq!(
        &section[start..end],
        read("spec_example.tessel.json"),
        "the §4.2 example in docs/SPEC.md must be in canonical form"
    );
}

#[test]
fn files_are_lf_with_trailing_newline_and_short_lines() {
    for f in FIXTURES {
        let text = read(&format!("{f}.tessel.json"));
        assert!(text.ends_with("}\n") && !text.ends_with("\n\n"), "{f}");
        assert!(!text.contains('\r'), "{f}");
        for line in text.lines() {
            assert!(!line.ends_with(' '), "{f}: trailing space: {line:?}");
            assert!(line.chars().count() <= 100, "{f}: long line: {line:?}");
        }
    }
}

#[test]
fn unknown_keys_survive_round_trip() {
    let d = load_clean(&read("kitchen_sink.tessel.json"), "kitchen_sink");
    assert!(d.extra.contains_key("future_top_level"));
    assert!(d.format.extra.contains_key("minor"));
    assert!(d.pages[0].extra.contains_key("page_future"));
    let frame = d.node(&doc::Id::from_raw("n_0000000001")).unwrap();
    let doc::NodeKind::Frame(f) = &frame.kind else {
        panic!("not a frame")
    };
    assert!(f.extra.contains_key("future_node_key"));
    assert!(f
        .layout
        .as_ref()
        .unwrap()
        .extra
        .contains_key("future_layout"));
}

#[test]
fn defaults_are_filled_in_on_read() {
    let d = load_clean(&read("spec_example.tessel.json"), "spec_example");
    let title = d.node(&doc::Id::from_raw("n_5t6y7v8h9r")).unwrap();
    assert!(title.visible);
    assert!(!title.locked);
    assert_eq!(title.opacity, 1.0);
    assert_eq!(title.rotation, 0.0);
    let doc::NodeKind::Text(t) = &title.kind else {
        panic!("not text")
    };
    assert_eq!(t.style, doc::TextStyle::default());
}

#[test]
fn invalid_documents_load_with_problems() {
    let text = r##"{
      "format": { "name": "tessel", "version": 1 },
      "pages": [
        { "id": "p_0000000001", "name": "P", "background": "#fff", "children": [
          { "id": "n_0000000001", "name": "A", "kind": "rect", "opacity": 2, "width": -1,
            "fill_style": "s_0000000009",
            "fills": [{ "type": "solid", "color": "{color.nope}" }],
            "children": [] },
          { "id": "n_0000000001", "name": "Dup", "kind": "group" },
          { "id": "bad", "name": "Bad", "kind": "instance", "component": "c_0000000009" }
        ] }
      ],
      "components": [
        { "id": "c_0000000001", "name": "Loop", "root": { "id": "n_00000000c1", "name": "R", "kind": "frame",
          "children": [{ "id": "n_00000000c2", "name": "I", "kind": "instance", "component": "c_0000000001" }] } }
      ]
    }"##;
    let loaded = doc::load_str(text).unwrap();
    let messages: Vec<String> = loaded.problems.iter().map(|p| p.to_string()).collect();
    let all = messages.join("\n");
    for expected in [
        "background \"#fff\" is not a #rrggbb[aa] color",
        "opacity 2 not in 0..=1",
        "width must be a finite number >= 0",
        "fill_style: unknown style s_0000000009",
        "unknown color token color.nope",
        "a rect node cannot have children",
        "duplicate ID n_0000000001",
        "a group must have at least one child",
        "bad: ID \"bad\" has an unknown prefix",
        "unknown component c_0000000009",
        "c_0000000001: component contains an instance of itself",
    ] {
        assert!(all.contains(expected), "missing {expected:?} in:\n{all}");
    }
}

#[test]
fn values_that_round_to_a_default_are_omitted() {
    // Regression: these used to be written as the rounded default ("opacity":
    // 1) and then omitted on the next save, so save -> load -> save differed.
    let mut d = load_clean(&read("spec_example.tessel.json"), "spec_example");
    let title = d.node_mut(&doc::Id::from_raw("n_5t6y7v8h9r")).unwrap();
    title.opacity = 0.999;
    title.rotation = -0.001;
    let doc::NodeKind::Text(t) = &mut title.kind else {
        panic!("not text")
    };
    t.style.font_size = 16.001;
    t.style.letter_spacing = 0.004;
    let first = doc::save_string(&d);
    assert_eq!(first, read("spec_example.tessel.json"));
    assert_eq!(doc::save_string(&load_clean(&first, "rounded")), first);
}
