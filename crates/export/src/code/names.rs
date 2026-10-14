//! Names in generated code: CSS classes from layer names (kebab-case),
//! React components (PascalCase) and props (camelCase), CSS custom
//! properties and Tailwind theme keys from token IDs. All ASCII; anything
//! else separates words.

use std::collections::{BTreeMap, HashSet};

use doc::{Document, TokenId};

/// The words of a name: split at non-alphanumerics and at lower-to-upper
/// case changes ("PricingCard 2" -> pricing, card, 2), lowercased.
pub(crate) fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `pricing-card`; `fallback` when the name has no usable characters. A
/// leading digit gets an `n-` prefix (classes may not start with one).
pub(crate) fn kebab(s: &str, fallback: &str) -> String {
    let k = words(s).join("-");
    if k.is_empty() {
        fallback.to_string()
    } else if k.starts_with(|c: char| c.is_ascii_digit()) {
        format!("n-{k}")
    } else {
        k
    }
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
        .unwrap_or_default()
}

/// `ButtonPrimary` from "Button/Primary"; a valid JS identifier.
pub(crate) fn pascal(s: &str, fallback: &str) -> String {
    let p: String = words(s).iter().map(|w| capitalize(w)).collect();
    if p.is_empty() {
        fallback.to_string()
    } else if p.starts_with(|c: char| c.is_ascii_digit()) {
        format!("{fallback}{p}")
    } else {
        p
    }
}

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "className",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "key",
    "new",
    "null",
    "ref",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
];

/// `showIcon` from "Show icon"; a valid, non-reserved JS identifier.
pub(crate) fn camel(s: &str, fallback: &str) -> String {
    let w = words(s);
    let mut c = String::new();
    for (i, w) in w.iter().enumerate() {
        c.push_str(&if i == 0 { w.clone() } else { capitalize(w) });
    }
    if c.is_empty() || RESERVED.contains(&c.as_str()) {
        c = format!("{fallback}{}", capitalize(&c));
    }
    if c.starts_with(|c: char| c.is_ascii_digit()) {
        c = format!("{fallback}{c}");
    }
    c
}

/// Hands out distinct names: the first `base` is `base`, later ones
/// `base-2`, `base-3`, … (`sep` between base and number).
#[derive(Default)]
pub(crate) struct Unique {
    taken: HashSet<String>,
}

impl Unique {
    pub(crate) fn take(&mut self, base: &str, sep: &str) -> String {
        if self.taken.insert(base.to_string()) {
            return base.to_string();
        }
        (2..)
            .map(|i| format!("{base}{sep}{i}"))
            .find(|n| self.taken.insert(n.clone()))
            .expect("an unused name")
    }
}

/// The four token groups of §3.6.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TokKind {
    Color,
    Space,
    Radius,
    Font,
}

impl TokKind {
    /// Tailwind v4 theme namespace (`--color-*`, `--spacing-*`, …).
    pub(crate) fn v4(self) -> &'static str {
        match self {
            TokKind::Color => "color",
            TokKind::Space => "spacing",
            TokKind::Radius => "radius",
            TokKind::Font => "font",
        }
    }

    /// Tailwind v3 theme section.
    pub(crate) fn v3(self) -> &'static str {
        match self {
            TokKind::Color => "colors",
            TokKind::Space => "spacing",
            TokKind::Radius => "borderRadius",
            TokKind::Font => "fontFamily",
        }
    }

    /// Leading token-ID segments that name the group and are dropped from
    /// the Tailwind key ("color.primary" -> `primary`).
    fn prefixes(self) -> &'static [&'static str] {
        match self {
            TokKind::Color => &["color.", "colors."],
            TokKind::Space => &["space.", "spacing."],
            TokKind::Radius => &["radius.", "radii.", "rounded."],
            TokKind::Font => &["font.", "fonts."],
        }
    }
}

/// How one token is named in code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TokName {
    /// The CSS custom property without `--`: "color.primary" -> `color-primary`.
    pub css: String,
    /// The Tailwind theme key: `primary` (`bg-primary`, `--color-primary`).
    pub key: String,
}

/// Names of every token in the document, distinct within their namespace.
#[derive(Default)]
pub(crate) struct TokenNames {
    map: BTreeMap<(TokKind, TokenId), TokName>,
}

impl TokenNames {
    pub(crate) fn new(doc: &Document) -> TokenNames {
        let t = &doc.tokens;
        let groups: [(TokKind, Vec<&TokenId>); 4] = [
            (TokKind::Color, t.colors.keys().collect()),
            (TokKind::Space, t.spacing.keys().collect()),
            (TokKind::Radius, t.radii.keys().collect()),
            (TokKind::Font, t.fonts.keys().collect()),
        ];
        let mut css = Unique::default();
        let mut map = BTreeMap::new();
        for (kind, ids) in groups {
            let mut keys = Unique::default();
            for id in ids {
                let stripped = kind
                    .prefixes()
                    .iter()
                    .find_map(|p| id.0.strip_prefix(p))
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&id.0);
                let key = keys.take(&key_of(stripped), "-");
                let css = css.take(&key_of(&id.0), "-");
                map.insert((kind, id.clone()), TokName { css, key });
            }
        }
        TokenNames { map }
    }

    pub(crate) fn get(&self, kind: TokKind, id: &TokenId) -> Option<&TokName> {
        self.map.get(&(kind, id.clone()))
    }
}

/// `space.4` -> `space-4`; digits may lead (theme keys like `4` are normal).
fn key_of(s: &str) -> String {
    let k = words(s).join("-");
    if k.is_empty() {
        "token".to_string()
    } else {
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_conversions() {
        assert_eq!(kebab("Pricing Card", "x"), "pricing-card");
        assert_eq!(kebab("PricingCard/Header", "x"), "pricing-card-header");
        assert_eq!(kebab("  ", "frame"), "frame");
        assert_eq!(kebab("2 col", "x"), "n-2-col");
        assert_eq!(kebab("Ünïcode", "x"), "n-code");
        assert_eq!(pascal("Button/Primary", "C"), "ButtonPrimary");
        assert_eq!(pascal("icon - star", "C"), "IconStar");
        assert_eq!(pascal("404 page", "Page"), "Page404Page");
        assert_eq!(camel("Show icon", "prop"), "showIcon");
        assert_eq!(camel("class", "prop"), "propClass");
        assert_eq!(camel("", "prop"), "prop");
    }

    #[test]
    fn unique_names() {
        let mut u = Unique::default();
        assert_eq!(u.take("card", "-"), "card");
        assert_eq!(u.take("card", "-"), "card-2");
        assert_eq!(u.take("card-2", "-"), "card-2-2");
        assert_eq!(u.take("card", "-"), "card-3");
    }

    #[test]
    fn token_names() {
        let mut doc = Document::default();
        doc.tokens
            .colors
            .insert(TokenId::new("color.primary"), doc::Color::new("#000000"));
        doc.tokens
            .colors
            .insert(TokenId::new("brand.Blue.500"), doc::Color::new("#000000"));
        doc.tokens.spacing.insert(TokenId::new("space.4"), 16.0);
        doc.tokens.spacing.insert(TokenId::new("space"), 4.0);
        let n = TokenNames::new(&doc);
        let c = n
            .get(TokKind::Color, &TokenId::new("color.primary"))
            .unwrap();
        assert_eq!(
            (c.css.as_str(), c.key.as_str()),
            ("color-primary", "primary")
        );
        let b = n
            .get(TokKind::Color, &TokenId::new("brand.Blue.500"))
            .unwrap();
        assert_eq!(
            (b.css.as_str(), b.key.as_str()),
            ("brand-blue-500", "brand-blue-500")
        );
        let s = n.get(TokKind::Space, &TokenId::new("space.4")).unwrap();
        assert_eq!((s.css.as_str(), s.key.as_str()), ("space-4", "4"));
        let s = n.get(TokKind::Space, &TokenId::new("space")).unwrap();
        assert_eq!((s.css.as_str(), s.key.as_str()), ("space", "space"));
    }
}
