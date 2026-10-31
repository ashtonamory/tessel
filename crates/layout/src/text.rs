//! Text measurement and shaping with parley (§7.3). Layouts are cached by
//! (content, style, alignment, width constraint) so text is shaped once per
//! change, not per frame; `render` draws glyph runs from the same layouts.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use doc::{Document, LineHeight, Node, TextAlign, TextStyle, TextTransform};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamilyName, FontStyle, FontWeight, GenericFamily,
    LayoutContext, StyleProperty,
};

/// A shaped, line-broken and aligned text layout. Brushes are not used: a
/// text node has one style, and `render` paints it with the node's fills.
pub type TextLayout = parley::Layout<()>;

/// The text style a text node renders with (`doc::resolve`): its text style
/// reference if that resolves, else its inline style. `None` for other kinds.
pub fn effective_text_style<'a>(doc: &'a Document, node: &'a Node) -> Option<&'a TextStyle> {
    doc::resolve::effective_text_style(doc, node)
}

/// The font family a text style names, with a font token resolved.
pub fn resolve_family(doc: &Document, style: &TextStyle) -> String {
    doc::resolve::resolve_font(doc, &style.font_family).into_owned()
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    content: String,
    family: String,
    style: String,
    align: u8,
    width: Option<u32>,
}

/// Owns the parley font and layout contexts and the layout cache.
pub struct TextMeasurer {
    fcx: FontContext,
    lcx: LayoutContext<()>,
    cache: HashMap<Key, Arc<TextLayout>>,
    used: HashSet<Key>,
    missing_fonts: HashSet<String>,
}

impl Default for TextMeasurer {
    fn default() -> Self {
        TextMeasurer::new()
    }
}

/// Font files added with [`add_fonts`], registered in every measurer made
/// after. Held as blobs so each font context can register the same bytes.
static EXTRA_FONTS: std::sync::RwLock<Vec<parley::fontique::Blob<u8>>> =
    std::sync::RwLock::new(Vec::new());

/// Make the fonts in `dir` (`.ttf`, `.otf`, `.ttc`; not its subfolders)
/// available to text laid out from now on, as if installed. Returns the
/// families they add, sorted. Measurers made earlier do not see them.
pub fn add_fonts(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| {
                    matches!(
                        e.to_string_lossy().to_ascii_lowercase().as_str(),
                        "ttf" | "otf" | "ttc"
                    )
                })
        })
        .collect();
    files.sort();
    let mut probe = FontContext::new();
    let mut families = std::collections::BTreeSet::new();
    let mut blobs = Vec::new();
    for file in files {
        let blob = parley::fontique::Blob::new(Arc::new(std::fs::read(&file)?));
        let added = probe.collection.register_fonts(blob.clone(), None);
        if added.is_empty() {
            tracing::warn!("{}: not a font file", file.display());
            continue;
        }
        for (id, _) in added {
            if let Some(name) = probe.collection.family_name(id) {
                families.insert(name.to_string());
            }
        }
        blobs.push(blob);
    }
    EXTRA_FONTS.write().unwrap().extend(blobs);
    Ok(families.into_iter().collect())
}

impl TextMeasurer {
    /// Discovers system fonts through fontique (fontconfig on Linux), plus
    /// any added with [`add_fonts`].
    pub fn new() -> TextMeasurer {
        let mut fcx = FontContext::new();
        for blob in EXTRA_FONTS.read().unwrap().iter() {
            fcx.collection.register_fonts(blob.clone(), None);
        }
        TextMeasurer {
            fcx,
            lcx: LayoutContext::new(),
            cache: HashMap::new(),
            used: HashSet::new(),
            missing_fonts: HashSet::new(),
        }
    }

    /// Whether a font family is installed.
    pub fn has_family(&mut self, family: &str) -> bool {
        self.fcx.collection.family_id(family).is_some()
    }

    /// Families that were requested but are not installed; they fall back to
    /// the default sans-serif.
    pub fn missing_fonts(&self) -> &HashSet<String> {
        &self.missing_fonts
    }

    /// Shape `content` with `style`. `width` is the line-break width; `None`
    /// lays the text out on as few lines as its explicit breaks allow.
    pub fn layout(
        &mut self,
        content: &str,
        family: &str,
        style: &TextStyle,
        align: TextAlign,
        width: Option<f32>,
    ) -> Arc<TextLayout> {
        let key = Key {
            content: content.to_string(),
            family: family.to_string(),
            style: format!("{style:?}"),
            align: align as u8,
            width: width.map(|w| w.max(0.0).to_bits()),
        };
        self.used.insert(key.clone());
        if let Some(l) = self.cache.get(&key) {
            return l.clone();
        }
        if !self.missing_fonts.contains(family) && !self.has_family(family) {
            tracing::warn!("font family {family:?} is not installed; using sans-serif");
            self.missing_fonts.insert(family.to_string());
        }

        let transformed = apply_transform(content, style.text_transform);
        // An empty layout has no lines and so no height; shape a space instead
        // so empty text still has one line's height.
        let shaped: &str = if transformed.is_empty() {
            " "
        } else {
            &transformed
        };
        let families = [
            FontFamilyName::Named(Cow::Borrowed(family)),
            FontFamilyName::Generic(GenericFamily::SansSerif),
        ];
        let mut builder = self.lcx.ranged_builder(&mut self.fcx, shaped, 1.0, false);
        builder.push_default(StyleProperty::FontFamily((&families[..]).into()));
        builder.push_default(StyleProperty::FontSize(style.font_size.max(0.0)));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(
            style.font_weight as f32,
        )));
        builder.push_default(StyleProperty::FontStyle(if style.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        }));
        builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
        builder.push_default(StyleProperty::LineHeight(match style.line_height {
            LineHeight::Auto => parley::LineHeight::MetricsRelative(1.0),
            LineHeight::Px(px) => parley::LineHeight::Absolute(px),
            LineHeight::Percent(p) => parley::LineHeight::FontSizeRelative(p / 100.0),
        }));
        match style.decoration {
            doc::TextDecoration::None => {}
            doc::TextDecoration::Underline => {
                builder.push_default(StyleProperty::Underline(true));
            }
            doc::TextDecoration::Strike => {
                builder.push_default(StyleProperty::Strikethrough(true));
            }
        }
        let mut layout = builder.build(shaped);
        layout.break_all_lines(width);
        layout.align(
            match align {
                TextAlign::Left => Alignment::Left,
                TextAlign::Center => Alignment::Center,
                TextAlign::Right => Alignment::Right,
                TextAlign::Justify => Alignment::Justify,
            },
            AlignmentOptions::default(),
        );
        let layout = Arc::new(layout);
        self.cache.insert(key, layout.clone());
        layout
    }

    /// Drop cached layouts not used since the last call.
    pub fn end_pass(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.cache.retain(|k, _| used.contains(k));
    }
}

/// `s` with the text transform applied; what the layout is shaped from.
pub fn apply_transform(s: &str, t: TextTransform) -> String {
    match t {
        TextTransform::None => s.to_string(),
        TextTransform::Upper => s.to_uppercase(),
        TextTransform::Lower => s.to_lowercase(),
        TextTransform::Title => {
            let mut out = String::with_capacity(s.len());
            let mut start = true;
            for c in s.chars() {
                if start {
                    out.extend(c.to_uppercase());
                } else {
                    out.push(c);
                }
                start = c.is_whitespace();
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case() {
        assert_eq!(
            apply_transform("hello big world", TextTransform::Title),
            "Hello Big World"
        );
        assert_eq!(apply_transform("Abc", TextTransform::Upper), "ABC");
    }

    #[test]
    fn layouts_are_cached_and_pruned() {
        let mut m = TextMeasurer::new();
        let s = TextStyle::default();
        let a = m.layout("hi", "Inter", &s, TextAlign::Left, None);
        let b = m.layout("hi", "Inter", &s, TextAlign::Left, None);
        assert!(Arc::ptr_eq(&a, &b));
        m.end_pass();
        m.end_pass();
        let c = m.layout("hi", "Inter", &s, TextAlign::Left, None);
        assert!(!Arc::ptr_eq(&a, &c));
    }
}
