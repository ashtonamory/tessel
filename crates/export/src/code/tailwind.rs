//! HTML + Tailwind: the same elements as the HTML export with utilities
//! instead of classes. A token becomes its theme utility (`bg-primary`,
//! `p-4`, `rounded-md`, `font-sans`); any other value is an arbitrary value
//! (`w-[372px]`), and a property without a fitting utility an arbitrary
//! property (`[box-shadow:…]`). Tokens and text styles become the theme: a
//! v4 `@theme` block, or a v3 `tailwind.config.js`.

use doc::{Document, LineHeight, Node, TextStyle};
use layout::Geometry;

use super::build::{color_lit, family_lit, family_name, text_style_names, Builder};
use super::html::{css_comment, markup, page, Export};
use super::ir::{comment_text, header, num, px, Decl, El, Val};
use super::names::{TokKind, TokenNames};
use super::{CodeOptions, OutFile, TailwindVersion, TokenMode};

/// Font properties a v4 `--text-*` / v3 `fontSize` theme entry sets.
const TEXT_THEME_PROPS: &[&str] = &["font-size", "line-height", "letter-spacing", "font-weight"];

pub(super) struct Tw {
    v4: bool,
    /// Tokens as theme utilities (otherwise their values).
    vars: bool,
}

impl Tw {
    pub fn new(opts: &CodeOptions<'_>) -> Tw {
        Tw {
            v4: opts.tailwind == TailwindVersion::V4,
            vars: opts.tokens == TokenMode::CssVars,
        }
    }

    /// Every utility of an element.
    pub fn classes(&self, el: &El) -> String {
        let mut u = self.place(el);
        u.extend(self.look(el));
        u.join(" ")
    }

    /// Utilities placing the element in its parent.
    pub fn place(&self, el: &El) -> Vec<String> {
        self.utilities(&el.place)
    }

    /// Utilities for the element's own look and layout.
    pub fn look(&self, el: &El) -> Vec<String> {
        let mut u = Vec::new();
        match (&el.text_style, self.vars) {
            (Some(s), true) => {
                u.push(format!("text-{}", s.name));
                let rest: Vec<Decl> = el
                    .font
                    .iter()
                    .filter(|d| !TEXT_THEME_PROPS.contains(&d.prop))
                    .cloned()
                    .collect();
                u.extend(self.utilities(&rest));
            }
            _ => u.extend(self.utilities(&el.font)),
        }
        u.extend(self.utilities(&el.decls));
        u
    }

    pub fn utilities(&self, decls: &[Decl]) -> Vec<String> {
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
                self.padding(&decls[i..i + 4], &mut out);
                i += 4;
                continue;
            }
            out.extend(self.utility(&decls[i]));
            i += 1;
        }
        out
    }

    fn padding(&self, d: &[Decl], out: &mut Vec<String>) {
        let [t, r, b, l] = [&d[0].val, &d[1].val, &d[2].val, &d[3].val];
        let mut side = |p: &str, v: &Val| {
            if v.as_lit() != Some("0") {
                out.push(self.spacing(p, v));
            }
        };
        if t == r && t == b && t == l {
            side("p", t);
        } else if t == b && r == l {
            side("px", r);
            side("py", t);
        } else {
            side("pt", t);
            side("pr", r);
            side("pb", b);
            side("pl", l);
        }
    }

    /// A value inside `[…]`: tokens as theme references, spaces as `_`.
    fn arb(&self, v: &Val) -> String {
        let s = v.render(|kind, _, key, lit| match (self.vars, self.v4) {
            (false, _) => lit.to_string(),
            (true, true) => format!("var(--{}-{key})", kind.v4()),
            (true, false) => format!("theme({}.{key})", kind.v3()),
        });
        s.replace('_', "\\_").replace(' ', "_")
    }

    fn tok<'v>(&self, v: &'v Val, kind: TokKind) -> Option<&'v str> {
        v.single_tok()
            .filter(|(k, _)| self.vars && *k == kind)
            .map(|(_, key)| key)
    }

    fn color(&self, prefix: &str, v: &Val) -> String {
        match self.tok(v, TokKind::Color) {
            Some(k) => format!("{prefix}-{k}"),
            None => format!("{prefix}-[{}]", self.arb(v)),
        }
    }

    fn spacing(&self, prefix: &str, v: &Val) -> String {
        match (self.tok(v, TokKind::Space), v.as_lit()) {
            (Some(k), _) => format!("{prefix}-{k}"),
            (None, Some("0")) => format!("{prefix}-0"),
            _ => format!("{prefix}-[{}]", self.arb(v)),
        }
    }

    fn utility(&self, d: &Decl) -> Option<String> {
        let v = &d.val;
        let lit = v.as_lit();
        let arb = |prefix: &str| format!("{prefix}-[{}]", self.arb(v));
        Some(match (d.prop, lit) {
            ("position", Some(p)) => p.to_string(),
            ("left" | "right" | "top" | "bottom", Some("0")) => format!("{}-0", d.prop),
            ("left" | "right" | "top" | "bottom", _) => arb(d.prop),
            ("width", Some("100%")) => "w-full".into(),
            ("width", Some("fit-content")) => "w-fit".into(),
            ("width", _) => arb("w"),
            ("height", Some("100%")) => "h-full".into(),
            ("height", _) => arb("h"),
            ("min-width", _) => arb("min-w"),
            ("max-width", _) => arb("max-w"),
            ("min-height", _) => arb("min-h"),
            ("max-height", _) => arb("max-h"),
            ("flex", _) => arb("flex"),
            ("flex-shrink", Some("0")) => "shrink-0".into(),
            ("align-self", Some(a)) => format!("self-{}", flex_word(a)),
            ("display", Some("flex")) => "flex".into(),
            ("flex-direction", Some("column")) => "flex-col".into(),
            ("flex-wrap", Some("wrap")) => "flex-wrap".into(),
            ("gap", _) => self.spacing("gap", v),
            ("column-gap", _) => self.spacing("gap-x", v),
            ("row-gap", _) => self.spacing("gap-y", v),
            ("justify-content", Some(j)) => format!("justify-{}", flex_word(j)),
            ("align-items", Some(a)) => format!("items-{}", flex_word(a)),
            ("align-content", Some(a)) => format!("content-{}", flex_word(a)),
            ("overflow", Some(o)) => format!("overflow-{o}"),
            ("background-color", _) => self.color("bg", v),
            ("border-width", _) => arb("border"),
            ("border-style", Some(s)) => format!("border-{s}"),
            ("border-color", _) => self.color("border", v),
            ("border-radius", Some("50%")) => "rounded-full".into(),
            ("border-radius", _) => match self.tok(v, TokKind::Radius) {
                Some(k) => format!("rounded-{k}"),
                None => arb("rounded"),
            },
            ("opacity", _) => arb("opacity"),
            ("transform", Some(t)) if t.starts_with("rotate(") => {
                format!("rotate-[{}]", &t["rotate(".len()..t.len() - 1])
            }
            ("color", Some("transparent")) => "text-transparent".into(),
            ("color", _) => self.color("text", v),
            ("font-family", _) => match self.tok(v, TokKind::Font) {
                Some(k) => format!("font-{k}"),
                None => arb("font"),
            },
            ("font-size", _) => arb("text"),
            ("font-weight", Some(w)) => match w {
                "100" => "font-thin".into(),
                "200" => "font-extralight".into(),
                "300" => "font-light".into(),
                "400" => "font-normal".into(),
                "500" => "font-medium".into(),
                "600" => "font-semibold".into(),
                "700" => "font-bold".into(),
                "800" => "font-extrabold".into(),
                "900" => "font-black".into(),
                _ => arb("font"),
            },
            ("font-style", Some("italic")) => "italic".into(),
            ("line-height", _) => arb("leading"),
            ("letter-spacing", _) => arb("tracking"),
            ("text-transform", Some(t)) => t.to_string(),
            ("text-decoration", Some(t)) => t.to_string(),
            ("text-align", Some(a)) => format!("text-{a}"),
            ("white-space", Some(w)) => format!("whitespace-{w}"),
            ("object-fit", Some(f)) => format!("object-{f}"),
            ("fill", Some("none")) => "fill-none".into(),
            ("fill", _) => self.color("fill", v),
            ("stroke", _) => self.color("stroke", v),
            ("background-clip", Some("text")) => "bg-clip-text".into(),
            // `bg-clip-text` sets the prefixed property too.
            ("-webkit-background-clip", _) => return None,
            _ => format!("[{}:{}]", d.prop, self.arb(v)),
        })
    }
}

/// The Tailwind word for a flexbox keyword.
fn flex_word(v: &str) -> &str {
    match v {
        "flex-start" => "start",
        "flex-end" => "end",
        "space-between" => "between",
        v => v,
    }
}

pub(super) fn export(
    doc: &Document,
    geom: &Geometry,
    node: &Node,
    opts: &CodeOptions<'_>,
) -> Export {
    let mut b = Builder::new(doc, geom, opts);
    let items = b.root(node);
    let tw = Tw::new(opts);
    let head = header(opts.file, node.id.as_str(), opts.revision);
    let mut body = String::new();
    markup(&items, 1, &mut body, &|el| tw.classes(el));
    let mut files = vec![OutFile::text(
        "index.html",
        "html",
        page(&head, &node.name, "output.css", &build_note(opts), &body),
    )];
    files.extend(theme_files(doc, opts, &head));
    (files, std::mem::take(&mut b.assets))
}

fn build_note(opts: &CodeOptions<'_>) -> String {
    format!(
        "<!-- Build output.css with: {} -i styles.css -o output.css -->\n",
        match opts.tailwind {
            TailwindVersion::V4 => "npx @tailwindcss/cli",
            TailwindVersion::V3 => "npx tailwindcss",
        }
    )
}

/// The Tailwind entry stylesheet, plus `tailwind.config.js` for v3.
pub(super) fn theme_files(doc: &Document, opts: &CodeOptions<'_>, head: &str) -> Vec<OutFile> {
    let vars = opts.tokens == TokenMode::CssVars;
    let head_css = format!("/* {} */\n\n", css_comment(head));
    match opts.tailwind {
        TailwindVersion::V4 => {
            let mut css = head_css + "@import \"tailwindcss\";\n";
            let theme = if vars { theme_v4(doc) } else { String::new() };
            if !theme.is_empty() {
                css.push('\n');
                css.push_str(&theme);
            }
            vec![OutFile::text("styles.css", "css", css)]
        }
        TailwindVersion::V3 => vec![
            OutFile::text(
                "styles.css",
                "css",
                head_css + "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
            ),
            OutFile::text(
                "tailwind.config.js",
                "javascript",
                format!("// {head}\n{}", config_v3(doc, vars)),
            ),
        ],
    }
}

/// `theme.css` (§8 token export).
pub(super) fn theme_v4_file(doc: &Document) -> String {
    format!(
        "/* tessel tokens: \"{}\" */\n\n{}",
        css_comment(&doc.meta.title),
        theme_v4(doc)
    )
}

/// `tailwind.config.js` (§8 token export).
pub(super) fn config_v3_file(doc: &Document) -> String {
    format!(
        "// tessel tokens: \"{}\"\n{}",
        comment_text(&doc.meta.title),
        config_v3(doc, true)
    )
}

fn line_height(s: &TextStyle) -> String {
    match s.line_height {
        LineHeight::Auto => "normal".to_string(),
        LineHeight::Px(v) => px(v as f64),
        LineHeight::Percent(p) => format!("{}%", num(p as f64)),
    }
}

/// The `@theme` block, or nothing without tokens and text styles.
fn theme_v4(doc: &Document) -> String {
    let names = TokenNames::new(doc);
    let t = &doc.tokens;
    let mut lines = Vec::new();
    let mut add = |kind: TokKind, id: &doc::TokenId, v: String| {
        if let Some(n) = names.get(kind, id) {
            lines.push(format!("  --{}-{}: {v};", kind.v4(), n.key));
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
    for (_, name, s) in text_style_names(doc) {
        lines.push(format!("  --text-{name}: {};", px(s.font_size as f64)));
        lines.push(format!("  --text-{name}--line-height: {};", line_height(s)));
        if num(s.letter_spacing as f64) != "0" {
            lines.push(format!(
                "  --text-{name}--letter-spacing: {};",
                px(s.letter_spacing as f64)
            ));
        }
        lines.push(format!("  --text-{name}--font-weight: {};", s.font_weight));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("@theme {{\n{}\n}}\n", lines.join("\n"))
}

fn js(s: &str) -> String {
    super::ir::js_str(s)
}

fn config_v3(doc: &Document, vars: bool) -> String {
    let names = TokenNames::new(doc);
    let t = &doc.tokens;
    let mut sections: Vec<(&str, Vec<String>)> = Vec::new();
    if vars {
        let group = |kind: TokKind, entries: Vec<(&doc::TokenId, String)>| {
            entries
                .into_iter()
                .filter_map(|(id, v)| {
                    let n = names.get(kind, id)?;
                    Some(format!("{}: {v},", js(&n.key)))
                })
                .collect::<Vec<_>>()
        };
        sections.push((
            "colors",
            group(
                TokKind::Color,
                t.colors
                    .iter()
                    .map(|(id, c)| (id, js(&color_lit(c))))
                    .collect(),
            ),
        ));
        sections.push((
            "spacing",
            group(
                TokKind::Space,
                t.spacing
                    .iter()
                    .map(|(id, v)| (id, js(&px(v.max(0.0) as f64))))
                    .collect(),
            ),
        ));
        sections.push((
            "borderRadius",
            group(
                TokKind::Radius,
                t.radii
                    .iter()
                    .map(|(id, v)| (id, js(&px(v.max(0.0) as f64))))
                    .collect(),
            ),
        ));
        sections.push((
            "fontFamily",
            group(
                TokKind::Font,
                t.fonts
                    .iter()
                    .map(|(id, f)| (id, format!("[{}, \"sans-serif\"]", js(&family_name(f)))))
                    .collect(),
            ),
        ));
        let sizes = text_style_names(doc)
            .into_iter()
            .map(|(_, name, s)| {
                let mut opts = vec![format!("lineHeight: {}", js(&line_height(s)))];
                if num(s.letter_spacing as f64) != "0" {
                    opts.push(format!(
                        "letterSpacing: {}",
                        js(&px(s.letter_spacing as f64))
                    ));
                }
                opts.push(format!("fontWeight: {}", js(&s.font_weight.to_string())));
                format!(
                    "{}: [{}, {{ {} }}],",
                    js(&name),
                    js(&px(s.font_size as f64)),
                    opts.join(", ")
                )
            })
            .collect();
        sections.push(("fontSize", sizes));
    }
    let mut extend = String::new();
    for (name, entries) in sections.into_iter().filter(|(_, e)| !e.is_empty()) {
        extend.push_str(&format!("      {name}: {{\n"));
        for e in entries {
            extend.push_str(&format!("        {e}\n"));
        }
        extend.push_str("      },\n");
    }
    let extend = if extend.is_empty() {
        "    extend: {},\n".to_string()
    } else {
        format!("    extend: {{\n{extend}    }},\n")
    };
    format!(
        "/** @type {{import('tailwindcss').Config}} */\nmodule.exports = {{\n  content: [\"./*.{{html,tsx}}\"],\n  theme: {{\n{extend}  }},\n}};\n"
    )
}
