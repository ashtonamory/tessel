//! The element tree every code format is written from: one [`El`] per
//! exported node with its CSS declarations. Values keep their token
//! references ([`Part::Tok`]) so each writer decides how a token is spelled:
//! `var(--color-primary)`, the literal, or a Tailwind theme utility.

use super::names::TokKind;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Part {
    Lit(String),
    Tok {
        kind: TokKind,
        /// CSS custom property name, without `--`.
        css: String,
        /// Tailwind theme key.
        key: String,
        /// The token's value, for inline output.
        lit: String,
    },
}

/// A CSS value: literal text with token references in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Val(pub Vec<Part>);

impl Val {
    pub fn lit(s: impl Into<String>) -> Val {
        Val(vec![Part::Lit(s.into())])
    }

    pub fn push(&mut self, s: &str) -> &mut Val {
        match self.0.last_mut() {
            Some(Part::Lit(l)) => l.push_str(s),
            _ => self.0.push(Part::Lit(s.to_string())),
        }
        self
    }

    pub fn append(&mut self, v: &Val) -> &mut Val {
        for p in &v.0 {
            match p {
                Part::Lit(s) => {
                    self.push(s);
                }
                t => self.0.push(t.clone()),
            }
        }
        self
    }

    /// `parts` joined by `sep`.
    pub fn join(parts: &[Val], sep: &str) -> Val {
        let mut v = Val::default();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                v.push(sep);
            }
            v.append(p);
        }
        v
    }

    /// The value as CSS: tokens as `var(--name)`, or their literal values.
    pub fn css(&self, vars: bool) -> String {
        self.render(|kind, css, key, lit| {
            let _ = (kind, key);
            if vars {
                format!("var(--{css})")
            } else {
                lit.to_string()
            }
        })
    }

    pub fn render(&self, mut tok: impl FnMut(TokKind, &str, &str, &str) -> String) -> String {
        let mut s = String::new();
        for p in &self.0 {
            match p {
                Part::Lit(l) => s.push_str(l),
                Part::Tok {
                    kind,
                    css,
                    key,
                    lit,
                } => s.push_str(&tok(*kind, css, key, lit)),
            }
        }
        s
    }

    /// The token, if the value is exactly one token reference.
    pub fn single_tok(&self) -> Option<(TokKind, &str)> {
        match self.0.as_slice() {
            [Part::Tok { kind, key, .. }] => Some((*kind, key)),
            _ => None,
        }
    }

    /// The value if it is plain literal text.
    pub fn as_lit(&self) -> Option<&str> {
        match self.0.as_slice() {
            [Part::Lit(l)] => Some(l),
            [] => Some(""),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decl {
    pub prop: &'static str,
    pub val: Val,
}

impl Decl {
    pub fn new(prop: &'static str, val: Val) -> Decl {
        Decl { prop, val }
    }

    pub fn lit(prop: &'static str, val: impl Into<String>) -> Decl {
        Decl::new(prop, Val::lit(val))
    }
}

/// A text style applied by reference (§3.6).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextStyleRef {
    /// The class (HTML) and Tailwind `text-*` key it is exported as.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct El {
    pub tag: String,
    /// Class named from the layer (HTML + CSS only).
    pub class: String,
    pub node_id: Option<String>,
    /// How the element sits in its parent: position, size, flex item.
    pub place: Vec<Decl>,
    /// Its own look, and how it lays out its children.
    pub decls: Vec<Decl>,
    /// Font declarations of a text node (also the contents of its text
    /// style, when it has one).
    pub font: Vec<Decl>,
    pub text_style: Option<TextStyleRef>,
    pub attrs: Vec<(String, String)>,
    pub body: Body,
    /// React: rendered only while this boolean prop is true.
    pub cond: Option<String>,
}

impl El {
    pub fn new(tag: &str, class: String) -> El {
        El {
            tag: tag.to_string(),
            class,
            node_id: None,
            place: Vec::new(),
            decls: Vec::new(),
            font: Vec::new(),
            text_style: None,
            attrs: Vec::new(),
            body: Body::Empty,
            cond: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Body {
    Empty,
    Text(String),
    /// React: the value of a string prop (`{label}`).
    Prop(String),
    Children(Vec<Item>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    El(El),
    /// React: a component instance, `<ButtonPrimary label="Buy" />`.
    Call(Call),
    /// React: the value of a swap prop (`{icon}`), shown while the
    /// condition (a boolean prop) is true.
    Slot(String, Option<String>),
}

impl Item {
    pub fn set_cond(&mut self, c: &str) {
        match self {
            Item::El(e) => e.cond = Some(c.to_string()),
            Item::Call(call) => call.cond = Some(c.to_string()),
            Item::Slot(_, cond) => *cond = Some(c.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Call {
    pub component: String,
    pub props: Vec<(String, PropVal)>,
    /// Placement of the instance, passed as `className`.
    pub place: Vec<Decl>,
    pub cond: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PropVal {
    Str(String),
    Bool(bool),
    Node(Vec<Item>),
}

// ---------------------------------------------------------------------------
// Formatting shared by the writers

/// A number with at most 2 decimals and no trailing zeros.
pub(crate) fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 || !r.is_finite() {
        "0".to_string()
    } else {
        format!("{r}")
    }
}

/// `16px`; zero is `0`.
pub(crate) fn px(v: f64) -> String {
    match num(v).as_str() {
        "0" => "0".to_string(),
        n => format!("{n}px"),
    }
}

pub(crate) fn esc_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// A double-quoted JS string literal.
pub(crate) fn js_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Text for a `/* */`, `//` or `<!-- -->` comment: line breaks and other
/// control characters become spaces, and `*`, `<`, `>` are dropped, so
/// nothing in it can end the comment or start code.
pub(crate) fn comment_text(s: &str) -> String {
    one_line(
        &s.chars()
            .filter(|c| !matches!(c, '*' | '<' | '>'))
            .collect::<String>(),
    )
}

/// One line of text: line breaks and other control characters become spaces.
pub(crate) fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// A file name inside `assets/`: ASCII letters, digits, `-`, `_` and `.`;
/// anything else is dropped, and it never starts with `.`.
pub(crate) fn asset_file(name: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if n.is_empty() || n.starts_with('.') {
        format!("_{n}")
    } else {
        n
    }
}

/// SVG path data without anything but its commands, numbers and separators.
pub(crate) fn path_data(d: &str) -> String {
    d.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | ',' | '.' | '-' | '+'))
        .collect()
}

/// The header line every exported file starts with (§8).
pub(crate) fn header(file: &str, node: &str, revision: u64) -> String {
    format!(
        "tessel export: file=\"{}\" node=\"{}\" revision={revision}",
        comment_text(file),
        comment_text(node)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_render_tokens_or_literals() {
        let mut v = Val::lit("0 4px 8px ");
        v.0.push(Part::Tok {
            kind: TokKind::Color,
            css: "color-ink".into(),
            key: "ink".into(),
            lit: "#0f172a".into(),
        });
        assert_eq!(v.css(true), "0 4px 8px var(--color-ink)");
        assert_eq!(v.css(false), "0 4px 8px #0f172a");
        assert_eq!(v.single_tok(), None);
        assert_eq!(px(16.0), "16px");
        assert_eq!(px(0.004), "0");
        assert_eq!(px(-1.5), "-1.5px");
        assert_eq!(js_str("a\"b\nc"), "\"a\\\"b\\nc\"");
    }

    #[test]
    fn comment_text_cannot_end_a_comment_or_a_line() {
        for hostile in [
            "a*/b",
            "a-->b",
            "a--!>b",
            "a<!--b",
            "a\nb\rc\u{2028}d\u{2029}e",
            "**//",
        ] {
            let c = comment_text(hostile);
            assert!(
                !c.contains("*/") && !c.contains('>') && !c.contains('<'),
                "{c:?}"
            );
            assert!(!c
                .chars()
                .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}'));
        }
        assert_eq!(comment_text("Pricing - v2 (final)"), "Pricing - v2 (final)");
        assert_eq!(
            header("a*/\nb.json", "n_1\" -->", 3),
            "tessel export: file=\"a/ b.json\" node=\"n_1\" --\" revision=3"
        );
    }

    #[test]
    fn asset_files_stay_inside_assets() {
        assert_eq!(asset_file("3fa9c1.png"), "3fa9c1.png");
        assert_eq!(asset_file("../../etc/passwd"), "_....etcpasswd");
        assert_eq!(asset_file("a\\b\"c`d$e.png"), "abcde.png");
        assert_eq!(asset_file(""), "_");
        assert_eq!(asset_file(".hidden"), "_.hidden");
    }

    #[test]
    fn path_data_keeps_commands_and_numbers() {
        assert_eq!(path_data("M12 2L15 9H22Z"), "M12 2L15 9H22Z");
        assert_eq!(path_data("M1,2.5-3\"/><script>"), "M1,2.5-3script");
    }
}
