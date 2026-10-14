//! Resolution of token and style references (§3.6): what a node actually
//! draws with once its style references and `{token}` references are
//! applied, and the patch that detaches a node from its styles.

use std::borrow::Cow;

use crate::command::NodePatch;
use crate::id::StyleId;
use crate::model::*;

/// The color a reference resolves to, if it is a literal or a known token.
pub fn resolve_color<'a>(doc: &'a Document, c: &'a ColorRef) -> Option<&'a Color> {
    match c {
        ColorRef::Literal(c) => Some(c),
        ColorRef::Token(t) => doc.tokens.colors.get(t),
    }
}

/// The px value a spacing reference resolves to.
pub fn resolve_space(doc: &Document, s: &SpaceRef) -> Option<f32> {
    match s {
        SpaceRef::Px(v) => Some(*v),
        SpaceRef::Token(t) => doc.tokens.spacing.get(t).copied(),
    }
}

/// The px value a radius reference resolves to.
pub fn resolve_radius(doc: &Document, r: &RadiusRef) -> Option<f32> {
    match r {
        RadiusRef::Px(v) => Some(*v),
        RadiusRef::Token(t) => doc.tokens.radii.get(t).copied(),
    }
}

/// The corner radii `[tl, tr, br, bl]` in px a node draws with; an unknown
/// token counts as 0. Nodes without a corner radius have none.
pub fn effective_corner_radius(doc: &Document, node: &Node) -> [f32; 4] {
    let radius = match &node.kind {
        NodeKind::Image(i) => Some(&i.corner_radius),
        _ => node.shape().map(|s| &s.corner_radius),
    };
    radius.map_or([0.0; 4], |r| {
        r.corners().map(|c| resolve_radius(doc, &c).unwrap_or(0.0))
    })
}

/// The font family a text style names: a font token reference resolves to
/// the token's family, anything else (or an unknown token) is used as is.
pub fn resolve_font<'a>(doc: &'a Document, family: &'a str) -> Cow<'a, str> {
    match TokenId::from_reference(family).and_then(|t| doc.tokens.fonts.get(&t)) {
        Some(f) => Cow::Borrowed(f.as_str()),
        None => Cow::Borrowed(family),
    }
}

pub fn style<'a>(doc: &'a Document, id: &StyleId) -> Option<&'a Style> {
    doc.styles.iter().find(|s| &s.id == id)
}

fn fill_style<'a>(doc: &'a Document, id: &Option<StyleId>) -> Option<&'a FillStyle> {
    match &style(doc, id.as_ref()?)?.kind {
        StyleKind::Fill(f) => Some(f),
        _ => None,
    }
}

fn stroke_style<'a>(doc: &'a Document, id: &Option<StyleId>) -> Option<&'a StrokeStyle> {
    match &style(doc, id.as_ref()?)?.kind {
        StyleKind::Stroke(s) => Some(s),
        _ => None,
    }
}

fn effect_style<'a>(doc: &'a Document, id: &Option<StyleId>) -> Option<&'a EffectStyle> {
    match &style(doc, id.as_ref()?)?.kind {
        StyleKind::Effect(e) => Some(e),
        _ => None,
    }
}

/// The fills a node draws: its fill style's fills if the reference
/// resolves, else its own. Text nodes have no fill style.
pub fn effective_fills<'a>(doc: &'a Document, node: &'a Node) -> &'a [Paint] {
    if let Some(s) = node.shape() {
        return fill_style(doc, &s.fill_style).map_or(&s.fills, |f| &f.fills);
    }
    match &node.kind {
        NodeKind::Text(t) => &t.fills,
        _ => &[],
    }
}

/// The strokes a node draws (stroke style first, then its own).
pub fn effective_strokes<'a>(doc: &'a Document, node: &'a Node) -> &'a [Paint] {
    node.shape().map_or(&[], |s| {
        stroke_style(doc, &s.stroke_style).map_or(&s.strokes, |st| &st.strokes)
    })
}

/// The stroke width a node draws with; a stroke style carries its own.
pub fn effective_stroke_width(doc: &Document, node: &Node) -> f32 {
    node.shape().map_or(0.0, |s| {
        stroke_style(doc, &s.stroke_style).map_or(s.stroke_width, |st| st.width)
    })
}

/// The shadows a node draws (effect style first, then its own).
pub fn effective_shadows<'a>(doc: &'a Document, node: &'a Node) -> &'a [Shadow] {
    node.shape().map_or(&[], |s| {
        effect_style(doc, &s.effect_style).map_or(&s.shadows, |e| &e.shadows)
    })
}

/// The text style a text node renders with: its text style if the
/// reference resolves, else its inline style. `None` for other kinds.
pub fn effective_text_style<'a>(doc: &'a Document, node: &'a Node) -> Option<&'a TextStyle> {
    let NodeKind::Text(t) = &node.kind else {
        return None;
    };
    let from_style = t
        .text_style
        .as_ref()
        .and_then(|id| style(doc, id))
        .and_then(|s| match &s.kind {
            StyleKind::Text(def) => Some(&def.style),
            _ => None,
        });
    Some(from_style.unwrap_or(&t.style))
}

/// The patch that copies the values of the styles `node` references onto the
/// node and clears the references (`SetProps` applies it; its inverse puts
/// the references back). With `only`, just the references to that style.
/// `None` if there is nothing to detach. A reference to a missing or
/// wrong-kind style is cleared without copying anything.
pub fn detach_style_patch(
    doc: &Document,
    node: &Node,
    only: Option<&StyleId>,
) -> Option<NodePatch> {
    let wanted = |id: &Option<StyleId>| match (id, only) {
        (Some(id), Some(only)) => id == only,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let mut p = NodePatch::default();
    if let Some(s) = node.shape() {
        if wanted(&s.fill_style) {
            p.fills = fill_style(doc, &s.fill_style).map(|f| f.fills.clone());
            p.fill_style = Some(None);
        }
        if wanted(&s.stroke_style) {
            if let Some(st) = stroke_style(doc, &s.stroke_style) {
                p.strokes = Some(st.strokes.clone());
                p.stroke_width = Some(st.width);
            }
            p.stroke_style = Some(None);
        }
        if wanted(&s.effect_style) {
            p.shadows = effect_style(doc, &s.effect_style).map(|e| e.shadows.clone());
            p.effect_style = Some(None);
        }
    }
    if let NodeKind::Text(t) = &node.kind {
        if wanted(&t.text_style) {
            p.style = effective_text_style(doc, node).cloned();
            p.text_style = Some(None);
        }
    }
    (p != NodePatch::default()).then_some(p)
}
