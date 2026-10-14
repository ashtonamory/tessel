//! Document invariants (§3.9).
//!
//! `validate` checks a whole document and returns every problem found; a
//! file with problems still loads (the app opens it read-only and lists
//! them). `check_subtree` is the local check commands run after applying.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::id::{ComponentId, Id, IdKind, StyleId};
use crate::model::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The object the problem is about, if any.
    pub id: Option<String>,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.id {
            Some(id) => write!(f, "{id}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// What references can resolve to.
struct Ctx<'a> {
    tokens: &'a Tokens,
    styles: HashMap<&'a StyleId, &'static str>,
    components: HashSet<&'a ComponentId>,
}

impl<'a> Ctx<'a> {
    fn new(doc: &'a Document) -> Ctx<'a> {
        Ctx {
            tokens: &doc.tokens,
            styles: doc
                .styles
                .iter()
                .map(|s| (&s.id, style_kind_tag(&s.kind)))
                .collect(),
            components: doc.components.iter().map(|c| &c.id).collect(),
        }
    }
}

fn style_kind_tag(k: &StyleKind) -> &'static str {
    match k {
        StyleKind::Fill(_) => "fill",
        StyleKind::Stroke(_) => "stroke",
        StyleKind::Text(_) => "text",
        StyleKind::Effect(_) => "effect",
    }
}

struct Out<'p> {
    id: Option<String>,
    problems: &'p mut Vec<Problem>,
}

impl Out<'_> {
    fn push(&mut self, message: impl Into<String>) {
        self.problems.push(Problem {
            id: self.id.clone(),
            message: message.into(),
        });
    }

    fn finite(&mut self, what: &str, v: f32) {
        if !v.is_finite() {
            self.push(format!("{what} is not a finite number"));
        }
    }

    fn non_negative(&mut self, what: &str, v: f32) {
        if !v.is_finite() || v < 0.0 {
            self.push(format!("{what} must be a finite number >= 0, got {v}"));
        }
    }
}

/// Check the whole document.
pub fn validate(doc: &Document) -> Vec<Problem> {
    let mut problems = Vec::new();
    let ctx = Ctx::new(doc);
    let mut out = Out {
        id: None,
        problems: &mut problems,
    };
    if doc.format.name != FORMAT_NAME {
        out.push(format!(
            "format.name is {:?}, expected {FORMAT_NAME:?}",
            doc.format.name
        ));
    }

    // IDs: well formed and unique across the document.
    let mut seen: HashSet<&Id> = HashSet::new();
    let mut all: Vec<(&Id, IdKind)> = Vec::new();
    all.extend(doc.pages.iter().map(|p| (&p.id, IdKind::Page)));
    all.extend(doc.styles.iter().map(|s| (&s.id, IdKind::Style)));
    all.extend(doc.components.iter().map(|c| (&c.id, IdKind::Component)));
    let mut nodes: Vec<&Node> = Vec::new();
    doc.walk_nodes(&mut |n| nodes.push(n));
    for c in &doc.components {
        c.root.walk(&mut |n| nodes.push(n));
    }
    all.extend(nodes.iter().map(|n| (&n.id, IdKind::Node)));
    for (id, kind) in all {
        let mut o = Out {
            id: Some(id.to_string()),
            problems: out.problems,
        };
        if let Err(e) = id.check(kind) {
            o.push(e.to_string());
        }
        if !seen.insert(id) {
            o.push(format!("duplicate ID {id}"));
        }
    }

    check_tokens(doc, &mut out);
    for s in &doc.styles {
        let mut o = Out {
            id: Some(s.id.to_string()),
            problems: out.problems,
        };
        check_style(&ctx, s, &mut o);
    }
    for p in &doc.pages {
        let mut o = Out {
            id: Some(p.id.to_string()),
            problems: out.problems,
        };
        if !p.background.is_well_formed() {
            o.push(format!(
                "background {:?} is not a #rrggbb[aa] color",
                p.background.0
            ));
        }
    }
    for n in &nodes {
        check_node(&ctx, n, out.problems);
    }
    check_components(doc, out.problems);
    problems
}

/// Check one node and its stored descendants (no ID uniqueness check).
pub(crate) fn check_subtree(doc: &Document, node: &Node) -> Vec<Problem> {
    let ctx = Ctx::new(doc);
    let mut problems = Vec::new();
    node.walk(&mut |n| check_with_id(&ctx, n, &mut problems));
    problems
}

/// Check one node's own props and ID, not its descendants.
pub(crate) fn check_one(doc: &Document, node: &Node) -> Vec<Problem> {
    let mut problems = Vec::new();
    check_with_id(&Ctx::new(doc), node, &mut problems);
    problems
}

fn check_with_id(ctx: &Ctx, n: &Node, problems: &mut Vec<Problem>) {
    if let Err(e) = n.id.check(IdKind::Node) {
        problems.push(Problem {
            id: Some(n.id.to_string()),
            message: e.to_string(),
        });
    }
    check_node(ctx, n, problems);
}

fn check_tokens(doc: &Document, out: &mut Out) {
    for (name, c) in &doc.tokens.colors {
        if !c.is_well_formed() {
            out.push(format!(
                "token {name}: {:?} is not a #rrggbb[aa] color",
                c.0
            ));
        }
    }
    for (name, v) in doc.tokens.spacing.iter().chain(&doc.tokens.radii) {
        out.non_negative(&format!("token {name}"), *v);
    }
}

fn check_style(ctx: &Ctx, s: &Style, out: &mut Out) {
    match &s.kind {
        StyleKind::Fill(f) => check_paints(ctx, "fills", &f.fills, out),
        StyleKind::Stroke(st) => {
            check_paints(ctx, "strokes", &st.strokes, out);
            out.non_negative("width", st.width);
        }
        StyleKind::Text(t) => check_text_style(ctx, &t.style, out),
        StyleKind::Effect(e) => check_shadows(ctx, &e.shadows, out),
    }
}

fn check_color(ctx: &Ctx, what: &str, c: &ColorRef, out: &mut Out) {
    match c {
        ColorRef::Literal(c) if !c.is_well_formed() => {
            out.push(format!("{what}: {:?} is not a #rrggbb[aa] color", c.0))
        }
        ColorRef::Token(t) if !ctx.tokens.colors.contains_key(t) => {
            out.push(format!("{what}: unknown color token {t}"))
        }
        _ => {}
    }
}

fn check_space(ctx: &Ctx, what: &str, s: &SpaceRef, out: &mut Out) {
    match s {
        SpaceRef::Px(v) => out.non_negative(what, *v),
        SpaceRef::Token(t) if !ctx.tokens.spacing.contains_key(t) => {
            out.push(format!("{what}: unknown spacing token {t}"))
        }
        SpaceRef::Token(_) => {}
    }
}

fn check_radius(ctx: &Ctx, r: &CornerRadius, out: &mut Out) {
    let mut one = |r: &RadiusRef| match r {
        RadiusRef::Px(v) => out.non_negative("corner_radius", *v),
        RadiusRef::Token(t) if !ctx.tokens.radii.contains_key(t) => {
            out.push(format!("corner_radius: unknown radius token {t}"))
        }
        RadiusRef::Token(_) => {}
    };
    match r {
        CornerRadius::All(r) => one(r),
        CornerRadius::Each(e) => e.iter().for_each(one),
    }
}

fn check_paints(ctx: &Ctx, what: &str, paints: &[Paint], out: &mut Out) {
    for p in paints {
        match p {
            Paint::Solid(s) => check_color(ctx, what, &s.color, out),
            Paint::LinearGradient(g) => {
                for v in g.start.iter().chain(&g.end) {
                    out.finite(what, *v);
                }
                check_stops(ctx, what, &g.stops, out);
            }
            Paint::RadialGradient(g) => {
                for v in g.center.iter().chain(&g.radius) {
                    out.finite(what, *v);
                }
                check_stops(ctx, what, &g.stops, out);
            }
            Paint::Image(_) => {}
        }
    }
}

fn check_stops(ctx: &Ctx, what: &str, stops: &[Stop], out: &mut Out) {
    for s in stops {
        if !(0.0..=1.0).contains(&s.offset) {
            out.push(format!(
                "{what}: gradient stop offset {} not in 0..=1",
                s.offset
            ));
        }
        check_color(ctx, what, &s.color, out);
    }
}

fn check_shadows(ctx: &Ctx, shadows: &[Shadow], out: &mut Out) {
    for s in shadows {
        out.finite("shadow x", s.x);
        out.finite("shadow y", s.y);
        out.non_negative("shadow blur", s.blur);
        out.finite("shadow spread", s.spread);
        check_color(ctx, "shadow", &s.color, out);
    }
}

fn check_text_style(ctx: &Ctx, t: &TextStyle, out: &mut Out) {
    if let Some(tok) = TokenId::from_reference(&t.font_family) {
        if !ctx.tokens.fonts.contains_key(&tok) {
            out.push(format!("font_family: unknown font token {tok}"));
        }
    }
    if !(100..=900).contains(&t.font_weight) {
        out.push(format!("font_weight {} not in 100..=900", t.font_weight));
    }
    out.non_negative("font_size", t.font_size);
    out.finite("letter_spacing", t.letter_spacing);
    match t.line_height {
        LineHeight::Auto => {}
        LineHeight::Px(v) | LineHeight::Percent(v) => out.non_negative("line_height", v),
    }
}

fn check_style_ref(ctx: &Ctx, slot: &str, id: &Option<StyleId>, want: &str, out: &mut Out) {
    let Some(id) = id else { return };
    match ctx.styles.get(id) {
        None => out.push(format!("{slot}: unknown style {id}")),
        Some(kind) if *kind != want => out.push(format!(
            "{slot}: style {id} is a {kind} style, expected {want}"
        )),
        Some(_) => {}
    }
}

fn check_shape(ctx: &Ctx, s: &Shape, out: &mut Out) {
    check_paints(ctx, "fills", &s.fills, out);
    check_paints(ctx, "strokes", &s.strokes, out);
    out.non_negative("stroke_width", s.stroke_width);
    check_radius(ctx, &s.corner_radius, out);
    check_shadows(ctx, &s.shadows, out);
    check_style_ref(ctx, "fill_style", &s.fill_style, "fill", out);
    check_style_ref(ctx, "stroke_style", &s.stroke_style, "stroke", out);
    check_style_ref(ctx, "effect_style", &s.effect_style, "effect", out);
}

/// Check one node's own props (not its descendants).
fn check_node(ctx: &Ctx, n: &Node, problems: &mut Vec<Problem>) {
    let mut out = Out {
        id: Some(n.id.to_string()),
        problems,
    };
    out.finite("x", n.x);
    out.finite("y", n.y);
    out.non_negative("width", n.width);
    out.non_negative("height", n.height);
    out.finite("rotation", n.rotation);
    if !(0.0..=1.0).contains(&n.opacity) {
        out.push(format!("opacity {} not in 0..=1", n.opacity));
    }
    let lc = &n.layout_child;
    for (what, v) in [
        ("min_width", lc.min_width),
        ("max_width", lc.max_width),
        ("min_height", lc.min_height),
        ("max_height", lc.max_height),
    ] {
        if let Some(v) = v {
            out.non_negative(what, v);
        }
    }

    let extra = match &n.kind {
        NodeKind::Frame(f) => {
            check_shape(ctx, &f.shape, &mut out);
            if let Some(l) = &f.layout {
                check_space(ctx, "gap", &l.gap, &mut out);
                if let Some(g) = &l.cross_gap {
                    check_space(ctx, "cross_gap", g, &mut out);
                }
                for p in &l.padding {
                    check_space(ctx, "padding", p, &mut out);
                }
            }
            &f.extra
        }
        NodeKind::Group(g) => {
            if g.children.is_empty() {
                out.push("a group must have at least one child");
            }
            &g.extra
        }
        NodeKind::Rect(r) => {
            check_shape(ctx, &r.shape, &mut out);
            &r.extra
        }
        NodeKind::Ellipse(e) => {
            check_shape(ctx, &e.shape, &mut out);
            &e.extra
        }
        NodeKind::Vector(v) => {
            check_shape(ctx, &v.shape, &mut out);
            &v.extra
        }
        NodeKind::Text(t) => {
            check_paints(ctx, "fills", &t.fills, &mut out);
            check_text_style(ctx, &t.style, &mut out);
            check_style_ref(ctx, "text_style", &t.text_style, "text", &mut out);
            &t.extra
        }
        NodeKind::Image(i) => {
            check_radius(ctx, &i.corner_radius, &mut out);
            &i.extra
        }
        NodeKind::Instance(inst) => {
            if !ctx.components.contains(&inst.component) {
                out.push(format!("unknown component {}", inst.component));
            }
            for (key, o) in &inst.overrides {
                match o {
                    Override::Swap { component } if !ctx.components.contains(component) => {
                        out.push(format!("override {key}: unknown component {component}"))
                    }
                    Override::Fills { fills } => {
                        check_paints(ctx, "override fills", fills, &mut out)
                    }
                    _ => {}
                }
            }
            &inst.extra
        }
    };
    if extra.contains_key("children") {
        out.push(format!("a {} node cannot have children", n.kind.tag()));
    }
}

/// The component problems of a document: roots that are not frames,
/// missing prop targets and cycles.
pub(crate) fn component_problems(doc: &Document) -> Vec<Problem> {
    let mut problems = Vec::new();
    check_components(doc, &mut problems);
    problems
}

/// Component roots are frames, prop targets exist, and there are no cycles.
fn check_components(doc: &Document, problems: &mut Vec<Problem>) {
    let mut uses: BTreeMap<&ComponentId, BTreeSet<&ComponentId>> = BTreeMap::new();
    for c in &doc.components {
        let mut out = Out {
            id: Some(c.id.to_string()),
            problems,
        };
        if !matches!(c.root.kind, NodeKind::Frame(_)) {
            out.push("component root must be a frame");
        }
        for p in &c.props {
            if !c.root.contains(&p.target) {
                out.push(format!(
                    "prop {:?}: target {} is not in the component",
                    p.name, p.target
                ));
            }
        }
        let mut used = BTreeSet::new();
        c.root.walk(&mut |n| {
            if let NodeKind::Instance(i) = &n.kind {
                used.insert(&i.component);
                for o in i.overrides.values() {
                    if let Override::Swap { component } = o {
                        used.insert(component);
                    }
                }
            }
        });
        uses.insert(&c.id, used);
    }

    // Depth-first search for cycles; report each component on a cycle once.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn dfs<'a>(
        c: &'a ComponentId,
        uses: &BTreeMap<&'a ComponentId, BTreeSet<&'a ComponentId>>,
        marks: &mut HashMap<&'a ComponentId, Mark>,
        cyclic: &mut BTreeSet<&'a ComponentId>,
    ) {
        marks.insert(c, Mark::Visiting);
        for next in uses.get(c).into_iter().flatten() {
            match marks.get(next) {
                Some(Mark::Visiting) => {
                    cyclic.insert(*next);
                }
                Some(Mark::Done) => {}
                None => dfs(next, uses, marks, cyclic),
            }
        }
        marks.insert(c, Mark::Done);
    }
    let mut marks = HashMap::new();
    let mut cyclic = BTreeSet::new();
    for c in uses.keys() {
        if !marks.contains_key(c) {
            dfs(c, &uses, &mut marks, &mut cyclic);
        }
    }
    for c in cyclic {
        problems.push(Problem {
            id: Some(c.to_string()),
            message: "component contains an instance of itself (directly or indirectly)".into(),
        });
    }

    // Instances whose expansion is too big. Checking costs up to the budget
    // per instance, so stop at the first one.
    let mut too_big = None;
    let mut find = |n: &Node| {
        if too_big.is_none() && !crate::expand::within_budget(doc, n) {
            too_big = Some(n.id.to_string());
        }
    };
    doc.walk_nodes(&mut find);
    for c in &doc.components {
        c.root.walk(&mut find);
    }
    if let Some(id) = too_big {
        problems.push(Problem {
            id: Some(id),
            message: format!(
                "instance expands to more than {} nodes",
                crate::expand::MAX_EXPANDED_NODES
            ),
        });
    }
}
