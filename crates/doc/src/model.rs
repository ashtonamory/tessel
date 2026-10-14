//! Document model types (§3). Plain data: `Clone`, `Debug`, `PartialEq`,
//! `Serialize`, `Deserialize`.
//!
//! Field order is the canonical key order of the file format (§4.3), and
//! `skip_serializing_if` omits defaults. Objects keep unknown keys in an
//! `extra` map so files from newer minor versions survive a round trip.

use std::collections::BTreeMap;
use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::id::{ComponentId, NodeId, PageId, StyleId};
use crate::index::NodeIndex;

/// Unknown keys of an object, kept verbatim (sorted) for round trips.
pub type Extra = BTreeMap<String, Value>;

pub const FORMAT_NAME: &str = "tessel";
/// The name files carried before the rename; still accepted on load.
pub const LEGACY_FORMAT_NAME: &str = "game-on-design";
pub const FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// skip/default helpers

// Defaults are compared as written, after rounding: a value that rounds to
// the default must be omitted, or the next save (of the rounded value)
// would differ.

pub(crate) fn is_default<T: Default + PartialEq + Serialize>(v: &T) -> bool {
    let d = T::default();
    *v == d
        || crate::canonical::to_canonical_string(v).ok()
            == crate::canonical::to_canonical_string(&d).ok()
}
pub(crate) fn is_true(v: &bool) -> bool {
    *v
}
pub(crate) fn is_zero(v: &f32) -> bool {
    crate::canonical::format_f32(*v) == "0"
}
pub(crate) fn is_one(v: &f32) -> bool {
    crate::canonical::format_f32(*v) == "1"
}
fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------
// Scalars

/// An sRGB color, `#rrggbb` or `#rrggbbaa`. Stored as written; `validate`
/// checks the form.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub String);

impl Color {
    pub fn new(s: impl Into<String>) -> Color {
        Color(s.into())
    }

    pub fn is_well_formed(&self) -> bool {
        let s = self.0.as_bytes();
        (s.len() == 7 || s.len() == 9) && s[0] == b'#' && s[1..].iter().all(u8::is_ascii_hexdigit)
    }
}

/// A dotted, human-chosen token name such as `color.primary` (§3.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TokenId(pub String);

impl TokenId {
    pub fn new(s: impl Into<String>) -> TokenId {
        TokenId(s.into())
    }

    /// Parse a `{token.name}` reference.
    pub fn from_reference(s: &str) -> Option<TokenId> {
        s.strip_prefix('{')?
            .strip_suffix('}')
            .map(|t| TokenId(t.to_string()))
    }

    pub fn to_reference(&self) -> String {
        format!("{{{}}}", self.0)
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An image asset file name inside `<name>.assets/` (§3.8).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetRef(pub String);

/// A literal color or a color token reference: `"#ff0000"` or `"{color.primary}"`.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorRef {
    Literal(Color),
    Token(TokenId),
}

impl ColorRef {
    pub fn literal(s: &str) -> ColorRef {
        ColorRef::Literal(Color::new(s))
    }
}

impl Serialize for ColorRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ColorRef::Literal(c) => s.serialize_str(&c.0),
            ColorRef::Token(t) => s.serialize_str(&t.to_reference()),
        }
    }
}

impl<'de> Deserialize<'de> for ColorRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match TokenId::from_reference(&s) {
            Some(t) => ColorRef::Token(t),
            None => ColorRef::Literal(Color(s)),
        })
    }
}

/// A spacing value in px or a spacing token reference: `16` or `"{space.4}"`.
#[derive(Clone, Debug, PartialEq)]
pub enum SpaceRef {
    Px(f32),
    Token(TokenId),
}

impl Default for SpaceRef {
    fn default() -> Self {
        SpaceRef::Px(0.0)
    }
}

impl Serialize for SpaceRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            SpaceRef::Px(v) => s.serialize_f32(*v),
            SpaceRef::Token(t) => s.serialize_str(&t.to_reference()),
        }
    }
}

impl<'de> Deserialize<'de> for SpaceRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = SpaceRef;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a number or a \"{token}\" reference")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<SpaceRef, E> {
                Ok(SpaceRef::Px(v as f32))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<SpaceRef, E> {
                Ok(SpaceRef::Px(v as f32))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<SpaceRef, E> {
                Ok(SpaceRef::Px(v as f32))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<SpaceRef, E> {
                TokenId::from_reference(v)
                    .map(SpaceRef::Token)
                    .ok_or_else(|| {
                        E::custom(format!("expected a \"{{token}}\" reference, got {v:?}"))
                    })
            }
        }
        d.deserialize_any(V)
    }
}

/// A radius in px or a radius token reference: `16` or `"{radius.md}"`.
#[derive(Clone, Debug, PartialEq)]
pub enum RadiusRef {
    Px(f32),
    Token(TokenId),
}

impl Default for RadiusRef {
    fn default() -> Self {
        RadiusRef::Px(0.0)
    }
}

impl Serialize for RadiusRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            RadiusRef::Px(v) => s.serialize_f32(*v),
            RadiusRef::Token(t) => s.serialize_str(&t.to_reference()),
        }
    }
}

impl<'de> Deserialize<'de> for RadiusRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = RadiusRef;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a number or a \"{token}\" reference")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<RadiusRef, E> {
                Ok(RadiusRef::Px(v as f32))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<RadiusRef, E> {
                Ok(RadiusRef::Px(v as f32))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<RadiusRef, E> {
                Ok(RadiusRef::Px(v as f32))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<RadiusRef, E> {
                TokenId::from_reference(v)
                    .map(RadiusRef::Token)
                    .ok_or_else(|| {
                        E::custom(format!("expected a \"{{token}}\" reference, got {v:?}"))
                    })
            }
        }
        d.deserialize_any(V)
    }
}

// ---------------------------------------------------------------------------
// Top level (§3.3)

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Document {
    pub format: FormatHeader,
    #[serde(default, skip_serializing_if = "is_default")]
    pub meta: Meta,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<Style>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    #[serde(flatten)]
    pub extra: Extra,
    #[serde(default)]
    pub pages: Vec<Page>,
    /// Derived `NodeId -> path` index; not serialized, rebuilt by commands
    /// and on load.
    #[serde(skip)]
    pub(crate) index: NodeIndex,
}

impl PartialEq for Document {
    fn eq(&self, o: &Self) -> bool {
        // The index is derived from the rest, so it does not take part.
        self.format == o.format
            && self.meta == o.meta
            && self.tokens == o.tokens
            && self.styles == o.styles
            && self.components == o.components
            && self.extra == o.extra
            && self.pages == o.pages
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormatHeader {
    pub name: String,
    pub version: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for FormatHeader {
    fn default() -> Self {
        FormatHeader {
            name: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            extra: Extra::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_with: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub id: PageId,
    #[serde(default)]
    pub name: String,
    #[serde(default = "Page::default_background")]
    pub background: Color,
    #[serde(flatten)]
    pub extra: Extra,
    /// Top-level nodes, back-to-front.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

impl Page {
    pub fn default_background() -> Color {
        Color::new("#f5f5f5")
    }

    pub fn new(id: PageId, name: impl Into<String>) -> Page {
        Page {
            id,
            name: name.into(),
            background: Page::default_background(),
            extra: Extra::new(),
            children: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Nodes (§3.4). `Node` has a hand-written serde impl (see `node_serde`) so
// that the `kind` tag sits after `name` and before the geometry.

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub opacity: f32,
    pub layout_child: LayoutChild,
    pub constraints: Constraints,
    pub kind: NodeKind,
}

impl Node {
    pub fn new(id: NodeId, name: impl Into<String>, kind: NodeKind) -> Node {
        Node {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            rotation: 0.0,
            opacity: 1.0,
            layout_child: LayoutChild::default(),
            constraints: Constraints::default(),
            kind,
        }
    }

    pub fn with_geometry(mut self, x: f32, y: f32, width: f32, height: f32) -> Node {
        self.x = x;
        self.y = y;
        self.width = width;
        self.height = height;
        self
    }

    /// Stored children, for kinds that store them (frame, group).
    pub fn children(&self) -> Option<&Vec<Node>> {
        match &self.kind {
            NodeKind::Frame(f) => Some(&f.children),
            NodeKind::Group(g) => Some(&g.children),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Node>> {
        match &mut self.kind {
            NodeKind::Frame(f) => Some(&mut f.children),
            NodeKind::Group(g) => Some(&mut g.children),
            _ => None,
        }
    }

    /// Calls `f` on this node and every stored descendant, depth first.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        if let Some(children) = self.children() {
            for c in children {
                c.walk(f);
            }
        }
    }

    pub fn contains(&self, id: &NodeId) -> bool {
        let mut found = false;
        self.walk(&mut |n| found |= &n.id == id);
        found
    }

    pub fn shape(&self) -> Option<&Shape> {
        match &self.kind {
            NodeKind::Frame(f) => Some(&f.shape),
            NodeKind::Rect(r) => Some(&r.shape),
            NodeKind::Ellipse(e) => Some(&e.shape),
            NodeKind::Vector(v) => Some(&v.shape),
            _ => None,
        }
    }

    pub fn shape_mut(&mut self) -> Option<&mut Shape> {
        match &mut self.kind {
            NodeKind::Frame(f) => Some(&mut f.shape),
            NodeKind::Rect(r) => Some(&mut r.shape),
            NodeKind::Ellipse(e) => Some(&mut e.shape),
            NodeKind::Vector(v) => Some(&mut v.shape),
            _ => None,
        }
    }
}

// Frames are much larger than groups; boxing would complicate every match on
// the kind for little gain, since nodes already live in `Vec`s.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Frame(Frame),
    Group(Group),
    Rect(Rect),
    Ellipse(Ellipse),
    Text(Text),
    Image(Image),
    Vector(Vector),
    Instance(Instance),
}

impl NodeKind {
    /// The `kind` tag as written in the file.
    pub fn tag(&self) -> &'static str {
        match self {
            NodeKind::Frame(_) => "frame",
            NodeKind::Group(_) => "group",
            NodeKind::Rect(_) => "rect",
            NodeKind::Ellipse(_) => "ellipse",
            NodeKind::Text(_) => "text",
            NodeKind::Image(_) => "image",
            NodeKind::Vector(_) => "vector",
            NodeKind::Instance(_) => "instance",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(flatten)]
    pub shape: Shape,
    /// Clip children to bounds.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub clip: bool,
    /// `None` = children positioned absolutely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<AutoLayout>,
    /// Export hint: "button", "nav", "section", …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            shape: Shape::default(),
            clip: true,
            layout: None,
            semantic: None,
            extra: Extra::new(),
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    #[serde(flatten)]
    pub extra: Extra,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    #[serde(flatten)]
    pub shape: Shape,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ellipse {
    #[serde(flatten)]
    pub shape: Shape,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Text {
    /// Plain text; `\n` for line breaks.
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_style: Option<StyleId>,
    /// Inline values; `text_style`, if set, overrides them.
    #[serde(default, skip_serializing_if = "is_default")]
    pub style: TextStyle,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fills: Vec<Paint>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align: TextAlign,
    #[serde(default, skip_serializing_if = "is_default")]
    pub vertical_align: VAlign,
    #[serde(default, skip_serializing_if = "is_default")]
    pub sizing: TextSizing,
    /// "h1", "p", "label", …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub asset: AssetRef,
    #[serde(default, skip_serializing_if = "is_default")]
    pub fit: ImageFit,
    #[serde(default, skip_serializing_if = "is_default")]
    pub corner_radius: CornerRadius,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vector {
    /// SVG path data.
    #[serde(default)]
    pub path: String,
    #[serde(flatten)]
    pub shape: Shape,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A placed copy of a component (§3.7). Its children are derived from the
/// component, never stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub component: ComponentId,
    /// Key: component node ID inside the component root.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Override>,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---------------------------------------------------------------------------
// Paint and shape

/// Shared paint/shape props of frame, rect, ellipse and vector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Painted bottom-to-top.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fills: Vec<Paint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<Paint>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub stroke_width: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub stroke_align: StrokeAlign,
    /// Rect and frame only.
    #[serde(default, skip_serializing_if = "is_default")]
    pub corner_radius: CornerRadius,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<Shadow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_style: Option<StyleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_style: Option<StyleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_style: Option<StyleId>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            fills: Vec::new(),
            strokes: Vec::new(),
            stroke_width: 1.0,
            stroke_align: StrokeAlign::default(),
            corner_radius: CornerRadius::default(),
            shadows: Vec::new(),
            fill_style: None,
            stroke_style: None,
            effect_style: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeAlign {
    #[default]
    Inside,
    Center,
    Outside,
}

/// A single radius, or `[top_left, top_right, bottom_right, bottom_left]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CornerRadius {
    All(RadiusRef),
    Each([RadiusRef; 4]),
}

impl Default for CornerRadius {
    fn default() -> Self {
        CornerRadius::All(RadiusRef::default())
    }
}

impl From<f32> for RadiusRef {
    fn from(v: f32) -> Self {
        RadiusRef::Px(v)
    }
}

impl CornerRadius {
    /// A uniform radius in px.
    pub fn px(v: f32) -> CornerRadius {
        CornerRadius::All(RadiusRef::Px(v))
    }

    /// The four corners `[tl, tr, br, bl]`, token references included.
    pub fn corners(&self) -> [RadiusRef; 4] {
        match self {
            CornerRadius::All(r) => [r.clone(), r.clone(), r.clone(), r.clone()],
            CornerRadius::Each(e) => e.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Paint {
    Solid(SolidPaint),
    LinearGradient(LinearGradient),
    RadialGradient(RadialGradient),
    Image(ImagePaint),
}

impl Paint {
    pub fn solid(color: ColorRef) -> Paint {
        Paint::Solid(SolidPaint {
            color,
            extra: Extra::new(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolidPaint {
    pub color: ColorRef,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Gradient coordinates are in the node's unit box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearGradient {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub stops: Vec<Stop>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadialGradient {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    pub stops: Vec<Stop>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImagePaint {
    pub asset: AssetRef,
    #[serde(default, skip_serializing_if = "is_default")]
    pub fit: ImageFit,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub offset: f32,
    pub color: ColorRef,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    #[default]
    Fill,
    Fit,
    Crop,
    Tile,
}

/// A drop shadow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub blur: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spread: f32,
    pub color: ColorRef,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---------------------------------------------------------------------------
// Text

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// A family name such as "Inter", or a font token reference "{font.sans}".
    #[serde(default = "TextStyle::default_family")]
    pub font_family: String,
    /// 100..=900.
    #[serde(default = "TextStyle::default_weight")]
    pub font_weight: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default = "TextStyle::default_size")]
    pub font_size: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub line_height: LineHeight,
    /// px.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub letter_spacing: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub text_transform: TextTransform,
    #[serde(default, skip_serializing_if = "is_default")]
    pub decoration: TextDecoration,
    #[serde(flatten)]
    pub extra: Extra,
}

impl TextStyle {
    fn default_family() -> String {
        "Inter".to_string()
    }
    fn default_weight() -> u16 {
        400
    }
    fn default_size() -> f32 {
        16.0
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font_family: TextStyle::default_family(),
            font_weight: TextStyle::default_weight(),
            italic: false,
            font_size: TextStyle::default_size(),
            line_height: LineHeight::default(),
            letter_spacing: 0.0,
            text_transform: TextTransform::default(),
            decoration: TextDecoration::default(),
            extra: Extra::new(),
        }
    }
}

/// `"auto"`, `{ "px": 24 }` or `{ "percent": 150 }`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineHeight {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextTransform {
    #[default]
    None,
    Upper,
    Lower,
    Title,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextDecoration {
    #[default]
    None,
    Underline,
    Strike,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSizing {
    #[default]
    AutoWidth,
    AutoHeight,
    Fixed,
}

// ---------------------------------------------------------------------------
// Auto-layout (§3.5)

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoLayout {
    #[serde(default)]
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrap: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub gap: SpaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_gap: Option<SpaceRef>,
    /// `[top, right, bottom, left]`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub padding: [SpaceRef; 4],
    #[serde(default, skip_serializing_if = "is_default")]
    pub justify: Justify,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align: Align,
    #[serde(default)]
    pub width: Sizing,
    #[serde(default)]
    pub height: Sizing,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
    Baseline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sizing {
    #[default]
    Fixed,
    Hug,
    Fill,
}

/// How a node sits in an auto-layout parent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutChild {
    #[serde(default, skip_serializing_if = "is_default")]
    pub position: ChildPosition,
    #[serde(default, skip_serializing_if = "is_default")]
    pub width: Sizing,
    #[serde(default, skip_serializing_if = "is_default")]
    pub height: Sizing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_self: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f32>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildPosition {
    #[default]
    Flow,
    Absolute,
}

/// How a node sits in a non-auto-layout frame when the frame resizes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Constraints {
    #[serde(default, skip_serializing_if = "is_default")]
    pub horizontal: HConstraint,
    #[serde(default, skip_serializing_if = "is_default")]
    pub vertical: VConstraint,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HConstraint {
    #[default]
    Left,
    Right,
    LeftRight,
    Center,
    Scale,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VConstraint {
    #[default]
    Top,
    Bottom,
    TopBottom,
    Center,
    Scale,
}

// ---------------------------------------------------------------------------
// Tokens and styles (§3.6)

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<TokenId, Color>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spacing: BTreeMap<TokenId, f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub radii: BTreeMap<TokenId, f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fonts: BTreeMap<TokenId, String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub id: StyleId,
    #[serde(default)]
    pub name: String,
    /// Written as `"kind": "fill" | "stroke" | "text" | "effect"` plus the
    /// kind's fields; the kind's struct keeps unknown keys.
    #[serde(flatten)]
    pub kind: StyleKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StyleKind {
    Fill(FillStyle),
    Stroke(StrokeStyle),
    Text(TextStyleDef),
    Effect(EffectStyle),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FillStyle {
    #[serde(default)]
    pub fills: Vec<Paint>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    #[serde(default)]
    pub strokes: Vec<Paint>,
    #[serde(default = "one")]
    pub width: f32,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextStyleDef {
    #[serde(default)]
    pub style: TextStyle,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EffectStyle {
    #[serde(default)]
    pub shadows: Vec<Shadow>,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---------------------------------------------------------------------------
// Components (§3.7)

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: ComponentId,
    /// "Button/Primary"; becomes the React name `ButtonPrimary`.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<ComponentProp>,
    #[serde(flatten)]
    pub extra: Extra,
    /// A frame; the master definition. Last, like `children`.
    pub root: Node,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentProp {
    pub name: String,
    pub target: NodeId,
    pub kind: PropKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKind {
    Text,
    Visible,
    Swap,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Override {
    Text { content: String },
    Fills { fills: Vec<Paint> },
    Visible { visible: bool },
    Swap { component: ComponentId },
}
