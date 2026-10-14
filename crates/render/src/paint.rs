//! Document colors and paints to peniko brushes.

use doc::{ColorRef, Document, Paint};
use kurbo::{Affine, Point, Size};
use peniko::{Brush, Color, Gradient};

/// Parse `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, a))
}

/// Resolve a literal or token color. Unknown tokens and malformed colors
/// render magenta so they are easy to spot.
pub fn resolve_color(doc: &Document, c: &ColorRef) -> Color {
    let s = match c {
        ColorRef::Literal(c) => Some(c.0.as_str()),
        ColorRef::Token(t) => doc.tokens.colors.get(t).map(|c| c.0.as_str()),
    };
    s.and_then(parse_hex)
        .unwrap_or(Color::from_rgba8(255, 0, 255, 255))
}

/// A paint as a brush for a box of `size`, plus the brush transform it
/// needs. Image paints have no brush (they are fit and clipped per shape);
/// fills draw them with `image::draw_image` and strokes skip them (`None`).
pub fn brush(doc: &Document, paint: &Paint, size: Size) -> Option<(Brush, Option<Affine>)> {
    let at = |p: [f32; 2]| Point::new(p[0] as f64 * size.width, p[1] as f64 * size.height);
    match paint {
        Paint::Solid(s) => Some((Brush::Solid(resolve_color(doc, &s.color)), None)),
        Paint::LinearGradient(g) => {
            let stops: Vec<(f32, Color)> = g
                .stops
                .iter()
                .map(|s| (s.offset, resolve_color(doc, &s.color)))
                .collect();
            let grad = Gradient::new_linear(at(g.start), at(g.end)).with_stops(stops.as_slice());
            Some((Brush::Gradient(grad), None))
        }
        Paint::RadialGradient(g) => {
            let stops: Vec<(f32, Color)> = g
                .stops
                .iter()
                .map(|s| (s.offset, resolve_color(doc, &s.color)))
                .collect();
            let center = at(g.center);
            let rx = (g.radius[0] as f64 * size.width).max(1e-3);
            let ry = (g.radius[1] as f64 * size.height).max(1e-3);
            let grad = Gradient::new_radial(center, rx as f32).with_stops(stops.as_slice());
            // Elliptical: stretch the circular gradient vertically about its center.
            let tf = Affine::translate(center.to_vec2())
                * Affine::scale_non_uniform(1.0, ry / rx)
                * Affine::translate(-center.to_vec2());
            Some((Brush::Gradient(grad), Some(tf)))
        }
        Paint::Image(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors() {
        assert_eq!(
            parse_hex("#ff0000"),
            Some(Color::from_rgba8(255, 0, 0, 255))
        );
        assert_eq!(
            parse_hex("#00ff0080"),
            Some(Color::from_rgba8(0, 255, 0, 128))
        );
        assert_eq!(parse_hex("ff0000"), None);
        assert_eq!(parse_hex("#ff00"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }

    #[test]
    fn tokens_resolve() {
        let mut doc = Document::default();
        doc.tokens.colors.insert(
            doc::TokenId::new("color.primary"),
            doc::Color::new("#2563eb"),
        );
        let c = resolve_color(&doc, &ColorRef::Token(doc::TokenId::new("color.primary")));
        assert_eq!(c, Color::from_rgba8(0x25, 0x63, 0xeb, 255));
        let missing = resolve_color(&doc, &ColorRef::Token(doc::TokenId::new("nope")));
        assert_eq!(missing, Color::from_rgba8(255, 0, 255, 255));
    }
}
