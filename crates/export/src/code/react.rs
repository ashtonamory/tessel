//! React: TSX + Tailwind. The exported node is a page component; with
//! `include_components`, each component of the document is a function
//! component in its own file, named from the component ("Button/Primary"
//! -> `ButtonPrimary.tsx`), with typed props from its `ComponentProp`s
//! (`label?: string`, `showIcon?: boolean`, `icon?: ReactNode`) defaulting
//! to the master's values. Instances whose overrides are all props become
//! calls (`<ButtonPrimary label="Buy" />`); others are expanded in place.
//!
//! A component's root takes a `className` that replaces its placement (its
//! size, or position inside an absolute parent), so each call places the
//! instance where the design has it.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use doc::{Component, Document, Node, NodeKind, PropKind};
use layout::Geometry;

use super::build::{prop_names, Builder};
use super::html::Export;
use super::ir::{comment_text, header, js_str, Body, Item, PropVal};
use super::names::{pascal, Unique};
use super::tailwind::{theme_files, Tw};
use super::{CodeOptions, OutFile};

pub(super) fn export(
    doc: &Document,
    geom: &Geometry,
    node: &Node,
    opts: &CodeOptions<'_>,
) -> Export {
    let mut b = Builder::new(doc, geom, opts);
    let tw = Tw::new(opts);
    let head = header(opts.file, node.id.as_str(), opts.revision);
    let mut names = Unique::default();
    if opts.include_components {
        b.calls = true;
        for c in &doc.components {
            let name = names.take(&pascal(&c.name, "Component"), "");
            b.comp_names.insert(c.id.clone(), name);
        }
    }
    let page_name = names.take(&pascal(&node.name, "Page"), "");

    let items = b.root(node);
    let used = std::mem::take(&mut b.used);
    let mut src = format!("// {head}\n");
    imports(&used, false, &mut src);
    let _ = writeln!(src, "\nexport default function {page_name}() {{");
    body(&tw, &items, None, &mut src);
    src.push_str("}\n");
    let mut files = vec![OutFile::text(format!("{page_name}.tsx"), "tsx", src)];

    if opts.include_components {
        for c in &doc.components {
            let name = b.comp_names[&c.id].clone();
            let src = component(&mut b, &tw, c, &name, &head);
            files.push(OutFile::text(format!("{name}.tsx"), "tsx", src));
        }
    }
    files.extend(theme_files(doc, opts, &head));
    (files, std::mem::take(&mut b.assets))
}

fn imports(used: &BTreeSet<String>, react_node: bool, out: &mut String) {
    if react_node || !used.is_empty() {
        out.push('\n');
    }
    if react_node {
        out.push_str("import type { ReactNode } from \"react\";\n");
    }
    for u in used {
        let _ = writeln!(out, "import {{ {u} }} from \"./{u}\";");
    }
}

/// `return (…);` for `items`; `root_class` replaces the first element's
/// `className` attribute.
fn body(tw: &Tw, items: &[Item], root_class: Option<String>, out: &mut String) {
    match items {
        [] => out.push_str("  return null;\n"),
        [one] => {
            out.push_str("  return (\n");
            item(tw, one, 2, root_class, out);
            out.push_str("  );\n");
        }
        many => {
            out.push_str("  return (\n    <>\n");
            for i in many {
                item(tw, i, 3, None, out);
            }
            out.push_str("    </>\n  );\n");
        }
    }
}

fn component(b: &mut Builder<'_>, tw: &Tw, c: &Component, name: &str, head: &str) -> String {
    let props = prop_names(c);
    b.props.clear();
    for (p, n) in c.props.iter().zip(&props) {
        b.props
            .entry(p.target.clone())
            .or_default()
            .push((p.kind, n.clone()));
    }
    b.slot_defaults.clear();
    let items = b.root(&c.root);
    b.props.clear();
    let mut used = std::mem::take(&mut b.used);
    used.remove(name);

    let target = |id: &doc::NodeId| {
        let mut found = None;
        c.root.walk(&mut |n| {
            if &n.id == id {
                found = Some(n);
            }
        });
        found
    };
    let mut fields = Vec::new();
    let mut params = Vec::new();
    for (p, n) in c.props.iter().zip(&props) {
        let t = target(&p.target);
        let (ty, default) = match p.kind {
            PropKind::Text => (
                "string",
                match t.map(|t| &t.kind) {
                    Some(NodeKind::Text(text)) => Some(js_str(&text.content)),
                    _ => None,
                },
            ),
            PropKind::Visible => ("boolean", t.map(|t| t.visible.to_string())),
            PropKind::Swap => (
                "ReactNode",
                b.slot_defaults.get(n).map(|items| inline(tw, items, 1)),
            ),
        };
        fields.push(format!("  {n}?: {ty};"));
        params.push(match default {
            Some(d) => format!("  {n} = {d},"),
            None => format!("  {n},"),
        });
    }
    fields.push("  className?: string;".to_string());
    params.push("  className,".to_string());

    let swap = c.props.iter().any(|p| p.kind == PropKind::Swap);
    let mut src = format!("// {head}\n// Component \"{}\"\n", comment_text(&c.name));
    imports(&used, swap, &mut src);
    let _ = writeln!(
        src,
        "\nexport type {name}Props = {{\n{}\n}};\n\nexport function {name}({{\n{}\n}}: {name}Props) {{",
        fields.join("\n"),
        params.join("\n")
    );
    let root_class = match items.first() {
        Some(Item::El(el)) => {
            let base = template_text(&tw.look(el).join(" "));
            let place = tw.place(el).join(" ");
            Some(match (base.is_empty(), place.is_empty()) {
                (true, true) => "className={className}".to_string(),
                (true, false) => format!("className={{className ?? {}}}", js_str(&place)),
                (false, true) => format!("className={{`{base} ${{className ?? \"\"}}`.trim()}}"),
                (false, false) => format!(
                    "className={{`{base} ${{className ?? {}}}`}}",
                    js_str(&place)
                ),
            })
        }
        _ => None,
    };
    body(tw, &items, root_class, &mut src);
    src.push_str("}\n");
    src
}

/// Items as one JSX expression (a fragment for several), indented for a
/// position at `depth`.
fn inline(tw: &Tw, items: &[Item], depth: usize) -> String {
    let mut s = String::new();
    match items {
        [] => return "null".to_string(),
        [one] => item(tw, one, depth, None, &mut s),
        many => {
            let _ = writeln!(s, "{}<>", "  ".repeat(depth));
            for i in many {
                item(tw, i, depth + 1, None, &mut s);
            }
            let _ = writeln!(s, "{}</>", "  ".repeat(depth));
        }
    }
    s.trim().to_string()
}

/// Text for the inside of a JS template literal.
fn template_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('$', "\\$")
}

/// A JSX attribute value: a string literal, or an expression when JSX
/// strings cannot hold it.
fn attr(v: &str) -> String {
    if v.contains(['"', '\\', '&', '{', '}']) || v.chars().any(char::is_control) {
        format!("{{{}}}", js_str(v))
    } else {
        format!("\"{v}\"")
    }
}

/// JSX text, or a string expression when JSX would change it (braces,
/// markup, line breaks, runs of spaces).
fn text(t: &str) -> String {
    let plain = !t.is_empty()
        && !t.contains(['{', '}', '<', '>', '&', '\n', '\r', '\t', '"', '\''])
        && !t.contains("  ")
        && t.trim() == t;
    if plain {
        t.to_string()
    } else {
        format!("{{{}}}", js_str(t))
    }
}

fn item(tw: &Tw, it: &Item, depth: usize, root_class: Option<String>, out: &mut String) {
    let ind = "  ".repeat(depth);
    let cond = match it {
        Item::El(el) => el.cond.as_deref(),
        Item::Call(c) => c.cond.as_deref(),
        Item::Slot(_, c) => c.as_deref(),
    };
    if let (Item::Slot(name, Some(c)), _) = (it, cond) {
        let _ = writeln!(out, "{ind}{{{c} && {name}}}");
        return;
    }
    if let Some(c) = cond {
        let _ = writeln!(out, "{ind}{{{c} && (");
        let inner = match it {
            Item::El(el) => Item::El(super::ir::El {
                cond: None,
                ..el.clone()
            }),
            Item::Call(call) => Item::Call(super::ir::Call {
                cond: None,
                ..call.clone()
            }),
            Item::Slot(name, _) => Item::Slot(name.clone(), None),
        };
        item(tw, &inner, depth + 1, root_class, out);
        let _ = writeln!(out, "{ind})}}");
        return;
    }
    match it {
        Item::Slot(name, _) => {
            let _ = writeln!(out, "{ind}{{{name}}}");
        }
        Item::Call(call) => {
            let mut s = format!("<{}", call.component);
            for (k, v) in &call.props {
                let _ = match v {
                    PropVal::Str(t) => write!(s, " {k}={}", attr(t)),
                    PropVal::Bool(b) => write!(s, " {k}={{{b}}}"),
                    PropVal::Node(items) => write!(s, " {k}={{{}}}", inline(tw, items, depth)),
                };
            }
            let cls = tw.utilities(&call.place).join(" ");
            if !cls.is_empty() {
                let _ = write!(s, " className={}", attr(&cls));
            }
            let _ = writeln!(out, "{ind}{s} />");
        }
        Item::El(el) => {
            let mut open = format!("<{}", el.tag);
            match root_class {
                Some(rc) => {
                    let _ = write!(open, " {rc}");
                }
                None => {
                    let cls = tw.classes(el);
                    if !cls.is_empty() {
                        let _ = write!(open, " className={}", attr(&cls));
                    }
                }
            }
            if let Some(id) = &el.node_id {
                let _ = write!(open, " data-node={}", attr(id));
            }
            for (k, v) in &el.attrs {
                let _ = write!(open, " {k}={}", attr(v));
            }
            match &el.body {
                Body::Children(c) => {
                    let _ = writeln!(out, "{ind}{open}>");
                    for i in c {
                        item(tw, i, depth + 1, None, out);
                    }
                    let _ = writeln!(out, "{ind}</{}>", el.tag);
                }
                Body::Text(t) => {
                    let _ = writeln!(out, "{ind}{open}>{}</{}>", text(t), el.tag);
                }
                Body::Prop(p) => {
                    let _ = writeln!(out, "{ind}{open}>{{{p}}}</{}>", el.tag);
                }
                Body::Empty => {
                    let _ = writeln!(out, "{ind}{open} />");
                }
            }
        }
    }
}
