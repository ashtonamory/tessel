//! HTML + CSS: `index.html` with one element per node, each with a class
//! named from its layer, and `styles.css` with the tokens as `:root`
//! custom properties, a class per text style and a rule per element.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use doc::{AssetRef, Document, Node};
use layout::Geometry;

use super::build::{color_lit, family_lit, text_style_names, Builder};
use super::ir::{comment_text, esc_html, header, one_line, px, Body, Decl, El, Item, Val};
use super::names::TokKind;
use super::{CodeOptions, OutFile, TokenMode};

pub(super) type Export = (Vec<OutFile>, BTreeSet<AssetRef>);

/// Resets so elements are plain boxes, as in the design: border-box
/// sizing, no default margins, and no user-agent look on semantic tags.
const RESET: &str = "\
*,
*::before,
*::after {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
  border: 0 solid;
}

img,
svg {
  display: block;
}

button {
  background: none;
  color: inherit;
  font: inherit;
  text-align: inherit;
}

a {
  color: inherit;
  text-decoration: inherit;
}

ol,
ul {
  list-style: none;
}
";

pub(super) fn export(
    doc: &Document,
    geom: &Geometry,
    node: &Node,
    opts: &CodeOptions<'_>,
) -> Export {
    let mut b = Builder::new(doc, geom, opts);
    let items = b.root(node);
    let vars = opts.tokens == TokenMode::CssVars;
    let head = header(opts.file, node.id.as_str(), opts.revision);

    let mut body = String::new();
    markup(&items, 1, &mut body, &|el| {
        let mut c = vec![el.class.clone()];
        if let Some(s) = &el.text_style {
            c.push(s.name.clone());
        }
        c.join(" ")
    });
    let html = page(&head, &node.name, "styles.css", "", &body);

    let mut css = format!("/* {} */\n\n", css_comment(&head));
    if vars {
        css.push_str(&root_vars(&b));
    }
    css.push_str(RESET);
    css.push_str(&text_style_rules(&b, vars));
    rules(&items, vars, &mut css);
    (
        vec![
            OutFile::text("index.html", "html", html),
            OutFile::text("styles.css", "css", css),
        ],
        std::mem::take(&mut b.assets),
    )
}

/// `tokens.css` (§8 token export).
pub(super) fn tokens_css(doc: &Document) -> String {
    let geom = Geometry::default();
    let opts = CodeOptions::default();
    let b = Builder::new(doc, &geom, &opts);
    let mut css = format!(
        "/* tessel tokens: \"{}\" */\n\n",
        css_comment(&doc.meta.title)
    );
    css.push_str(&root_vars(&b));
    css.push_str(text_style_rules(&b, true).trim_start());
    css
}

pub(super) fn css_comment(s: &str) -> String {
    comment_text(s)
}

pub(super) fn html_comment(s: &str) -> String {
    comment_text(s)
}

/// The HTML page around `body`; `note` is extra head content.
pub(super) fn page(head: &str, title: &str, css: &str, note: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<!-- {} -->\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n{note}<link rel=\"stylesheet\" href=\"{css}\">\n</head>\n<body>\n{body}</body>\n</html>\n",
        html_comment(head),
        esc_html(&one_line(title)),
    )
}

/// Elements as HTML; `classes` gives each element's class attribute.
pub(super) fn markup(
    items: &[Item],
    depth: usize,
    out: &mut String,
    classes: &dyn Fn(&El) -> String,
) {
    let ind = "  ".repeat(depth);
    for item in items {
        // Component calls and slots only exist in React output.
        let Item::El(el) = item else { continue };
        let mut open = format!("<{}", el.tag);
        let cls = classes(el);
        if !cls.is_empty() {
            let _ = write!(open, " class=\"{}\"", esc_html(&cls));
        }
        if let Some(id) = &el.node_id {
            let _ = write!(open, " data-node=\"{}\"", esc_html(id));
        }
        for (k, v) in &el.attrs {
            let _ = write!(open, " {k}=\"{}\"", esc_html(v));
        }
        open.push('>');
        match &el.body {
            Body::Children(c) => {
                let _ = writeln!(out, "{ind}{open}");
                markup(c, depth + 1, out, classes);
                let _ = writeln!(out, "{ind}</{}>", el.tag);
            }
            Body::Text(t) => {
                let _ = writeln!(out, "{ind}{open}{}</{}>", esc_html(t), el.tag);
            }
            Body::Empty | Body::Prop(_) if el.tag == "img" => {
                let _ = writeln!(out, "{ind}{open}");
            }
            Body::Empty | Body::Prop(_) => {
                let _ = writeln!(out, "{ind}{open}</{}>", el.tag);
            }
        }
    }
}

/// `prop: value` pairs, with `padding-*` quartets collapsed to `padding`.
pub(super) fn css_decls<'d>(
    decls: impl IntoIterator<Item = &'d Decl>,
    vars: bool,
) -> Vec<(String, String)> {
    let decls: Vec<&Decl> = decls.into_iter().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < decls.len() {
        let sides = [
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ];
        if i + 4 <= decls.len() && (0..4).all(|k| decls[i + k].prop == sides[k]) {
            let v: Vec<&Val> = (0..4).map(|k| &decls[i + k].val).collect();
            out.push(("padding".to_string(), shorthand(&v).css(vars)));
            i += 4;
            continue;
        }
        out.push((decls[i].prop.to_string(), decls[i].val.css(vars)));
        i += 1;
    }
    out
}

/// `[top, right, bottom, left]` as the shortest box shorthand.
pub(super) fn shorthand(v: &[&Val]) -> Val {
    let [t, r, b, l] = [v[0], v[1], v[2], v[3]];
    let parts: Vec<Val> = if t == r && t == b && t == l {
        vec![t.clone()]
    } else if t == b && r == l {
        vec![t.clone(), r.clone()]
    } else if r == l {
        vec![t.clone(), r.clone(), b.clone()]
    } else {
        vec![t.clone(), r.clone(), b.clone(), l.clone()]
    };
    Val::join(&parts, " ")
}

fn rule(selector: &str, decls: &[(String, String)], out: &mut String) {
    if decls.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{selector} {{");
    for (p, v) in decls {
        let _ = writeln!(out, "  {p}: {v};");
    }
    out.push_str("}\n");
}

fn rules(items: &[Item], vars: bool, out: &mut String) {
    for item in items {
        let Item::El(el) = item else { continue };
        let font = if el.text_style.is_some() {
            &[][..]
        } else {
            &el.font[..]
        };
        let decls = css_decls(el.place.iter().chain(font).chain(&el.decls), vars);
        rule(&format!(".{}", el.class), &decls, out);
        if let Body::Children(c) = &el.body {
            rules(c, vars, out);
        }
    }
}

/// `:root` with every token, or nothing when there are none.
fn root_vars(b: &Builder<'_>) -> String {
    let t = &b.doc.tokens;
    let mut lines = Vec::new();
    let mut add = |kind: TokKind, id: &doc::TokenId, v: String| {
        if let Some(n) = b.toks.get(kind, id) {
            lines.push(format!("  --{}: {v};", n.css));
        }
    };
    for (id, c) in &t.colors {
        add(TokKind::Color, id, color_lit(c));
    }
    for (id, v) in &t.spacing {
        add(TokKind::Space, id, px(v.max(0.0) as f64));
    }
    for (id, v) in &t.radii {
        add(TokKind::Radius, id, px(v.max(0.0) as f64));
    }
    for (id, f) in &t.fonts {
        add(TokKind::Font, id, family_lit(f));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(":root {{\n{}\n}}\n\n", lines.join("\n"))
}

fn text_style_rules(b: &Builder<'_>, vars: bool) -> String {
    let mut out = String::new();
    for (_, name, style) in text_style_names(b.doc) {
        rule(
            &format!(".{name}"),
            &css_decls(&b.font_decls(style), vars),
            &mut out,
        );
    }
    out
}
