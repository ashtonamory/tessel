//! The token and style commands (§3.6, §5.1): `UpsertTokens`, `RenameToken`,
//! `UpsertStyle`, `InsertStyle` and `DeleteStyle`.

use std::collections::BTreeMap;
use std::mem;

use serde::{Deserialize, Serialize};

use crate::command::NodePatch;
use crate::command::{apply_all, non_node_ids, Command, CommandError};
use crate::id::{Id, IdKind, NodeId, StyleId};
use crate::model::*;
use crate::resolve::detach_style_patch;
use crate::validate::{validate, Problem};

/// Additions, changes and removals of tokens. A `null` value removes the
/// token. Removing a token that is still referenced is an error: detach the
/// references first, in the same `Batch` if it should be atomic.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokensPatch {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<TokenId, Option<Color>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spacing: BTreeMap<TokenId, Option<f32>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub radii: BTreeMap<TokenId, Option<f32>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fonts: BTreeMap<TokenId, Option<String>>,
}

/// Put `patch` into `map`, returning the previous state of the same keys.
fn swap_map<V: Clone>(
    map: &mut BTreeMap<TokenId, V>,
    patch: BTreeMap<TokenId, Option<V>>,
) -> BTreeMap<TokenId, Option<V>> {
    let mut inverse = BTreeMap::new();
    for (k, v) in patch {
        let old = match v {
            Some(v) => map.insert(k.clone(), v),
            None => map.remove(&k),
        };
        inverse.insert(k, old);
    }
    inverse
}

pub(crate) fn problems_of(doc: &Document, check: bool) -> Option<Vec<Problem>> {
    check.then(|| validate(doc))
}

/// If the edit introduced problems, undo it with `rollback` and fail.
pub(crate) fn check_or_rollback(
    doc: &mut Document,
    before: Option<Vec<Problem>>,
    rollback: Command,
) -> Result<Command, CommandError> {
    let Some(before) = before else {
        return Ok(rollback);
    };
    let new: Vec<Problem> = validate(doc)
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    if new.is_empty() {
        return Ok(rollback);
    }
    rollback
        .apply_unchecked(doc)
        .expect("restoring the previous state always applies");
    Err(CommandError::Invariant(new))
}

fn invariant(id: impl ToString, message: impl Into<String>) -> CommandError {
    CommandError::Invariant(vec![Problem {
        id: Some(id.to_string()),
        message: message.into(),
    }])
}

/// Token names are dotted words: non-empty, no braces or whitespace (they
/// are written `"{name}"` in references).
fn check_token_name(name: &TokenId) -> Result<(), CommandError> {
    let s = &name.0;
    if s.is_empty() || s.contains(['{', '}']) || s.chars().any(char::is_whitespace) {
        return Err(invariant(
            s,
            "token names must be non-empty and contain no braces or whitespace",
        ));
    }
    Ok(())
}

pub(crate) fn upsert_tokens(
    doc: &mut Document,
    patch: TokensPatch,
    check: bool,
) -> Result<Command, CommandError> {
    if check {
        let names = patch
            .colors
            .keys()
            .chain(patch.spacing.keys())
            .chain(patch.radii.keys())
            .chain(patch.fonts.keys());
        for n in names {
            check_token_name(n)?;
        }
    }
    let before = problems_of(doc, check);
    let t = &mut doc.tokens;
    let inverse = TokensPatch {
        colors: swap_map(&mut t.colors, patch.colors),
        spacing: swap_map(&mut t.spacing, patch.spacing),
        radii: swap_map(&mut t.radii, patch.radii),
        fonts: swap_map(&mut t.fonts, patch.fonts),
    };
    check_or_rollback(doc, before, Command::UpsertTokens { patch: inverse })
}

/// A reference to a token, by the kind of value it stands for.
pub(crate) enum RefMut<'a> {
    Color(&'a mut ColorRef),
    Space(&'a mut SpaceRef),
    Radius(&'a mut RadiusRef),
    /// A `font_family`, which is a token reference only if it is `{...}`.
    Font(&'a mut String),
}

type Visit<'v> = &'v mut dyn FnMut(RefMut);

fn visit_paints(paints: &mut [Paint], f: Visit) {
    for p in paints {
        match p {
            Paint::Solid(s) => f(RefMut::Color(&mut s.color)),
            Paint::LinearGradient(g) => g
                .stops
                .iter_mut()
                .for_each(|s| f(RefMut::Color(&mut s.color))),
            Paint::RadialGradient(g) => g
                .stops
                .iter_mut()
                .for_each(|s| f(RefMut::Color(&mut s.color))),
            Paint::Image(_) => {}
        }
    }
}

fn visit_shadows(shadows: &mut [Shadow], f: Visit) {
    for s in shadows {
        f(RefMut::Color(&mut s.color));
    }
}

fn visit_node(n: &mut Node, f: Visit) {
    match &mut n.kind {
        NodeKind::Frame(fr) => {
            visit_shape(&mut fr.shape, f);
            if let Some(l) = &mut fr.layout {
                f(RefMut::Space(&mut l.gap));
                if let Some(g) = &mut l.cross_gap {
                    f(RefMut::Space(g));
                }
                l.padding.iter_mut().for_each(|p| f(RefMut::Space(p)));
            }
        }
        NodeKind::Rect(r) => visit_shape(&mut r.shape, f),
        NodeKind::Ellipse(e) => visit_shape(&mut e.shape, f),
        NodeKind::Vector(v) => visit_shape(&mut v.shape, f),
        NodeKind::Text(t) => {
            visit_paints(&mut t.fills, f);
            f(RefMut::Font(&mut t.style.font_family));
        }
        NodeKind::Instance(i) => {
            for o in i.overrides.values_mut() {
                if let Override::Fills { fills } = o {
                    visit_paints(fills, f);
                }
            }
        }
        NodeKind::Image(i) => visit_radius(&mut i.corner_radius, f),
        NodeKind::Group(_) => {}
    }
    if let Some(children) = n.children_mut() {
        for c in children {
            visit_node(c, f);
        }
    }
}

fn visit_radius(r: &mut CornerRadius, f: Visit) {
    match r {
        CornerRadius::All(x) => f(RefMut::Radius(x)),
        CornerRadius::Each(e) => e.iter_mut().for_each(|x| f(RefMut::Radius(x))),
    }
}

fn visit_shape(s: &mut Shape, f: Visit) {
    visit_radius(&mut s.corner_radius, f);
    visit_paints(&mut s.fills, f);
    visit_paints(&mut s.strokes, f);
    visit_shadows(&mut s.shadows, f);
}

/// Every token reference in the document: pages, component masters and
/// styles.
fn visit_refs(doc: &mut Document, f: Visit) {
    for p in &mut doc.pages {
        for n in &mut p.children {
            visit_node(n, f);
        }
    }
    for c in &mut doc.components {
        visit_node(&mut c.root, f);
    }
    for s in &mut doc.styles {
        match &mut s.kind {
            StyleKind::Fill(x) => visit_paints(&mut x.fills, f),
            StyleKind::Stroke(x) => visit_paints(&mut x.strokes, f),
            StyleKind::Text(x) => f(RefMut::Font(&mut x.style.font_family)),
            StyleKind::Effect(x) => visit_shadows(&mut x.shadows, f),
        }
    }
}

fn rename_key<V>(map: &mut BTreeMap<TokenId, V>, from: &TokenId, to: &TokenId) -> bool {
    match map.remove(from) {
        Some(v) => {
            map.insert(to.clone(), v);
            true
        }
        None => false,
    }
}

pub(crate) fn rename_token(
    doc: &mut Document,
    from: TokenId,
    to: TokenId,
) -> Result<Command, CommandError> {
    let t = &doc.tokens;
    let kinds = [
        t.colors.contains_key(&from),
        t.spacing.contains_key(&from),
        t.radii.contains_key(&from),
        t.fonts.contains_key(&from),
    ];
    if !kinds.contains(&true) {
        return Err(CommandError::NotFound(format!("token {from}")));
    }
    check_token_name(&to)?;
    if t.colors.contains_key(&to)
        || t.spacing.contains_key(&to)
        || t.radii.contains_key(&to)
        || t.fonts.contains_key(&to)
    {
        return Err(invariant(&to, "a token with this name already exists"));
    }
    let [colors, spacing, radii, fonts] = kinds;
    let t = &mut doc.tokens;
    rename_key(&mut t.colors, &from, &to);
    rename_key(&mut t.spacing, &from, &to);
    rename_key(&mut t.radii, &from, &to);
    rename_key(&mut t.fonts, &from, &to);
    let (old_ref, new_ref) = (from.to_reference(), to.to_reference());
    visit_refs(doc, &mut |r| match r {
        RefMut::Color(ColorRef::Token(t)) if colors && *t == from => *t = to.clone(),
        RefMut::Space(SpaceRef::Token(t)) if spacing && *t == from => *t = to.clone(),
        RefMut::Radius(RadiusRef::Token(t)) if radii && *t == from => *t = to.clone(),
        RefMut::Font(f) if fonts && *f == old_ref => *f = new_ref.clone(),
        _ => {}
    });
    Ok(Command::RenameToken { from: to, to: from })
}

pub(crate) fn upsert_style(
    doc: &mut Document,
    style: Style,
    check: bool,
) -> Result<Command, CommandError> {
    let before = problems_of(doc, check);
    if let Some(at) = doc.styles.iter().position(|s| s.id == style.id) {
        let old = mem::replace(&mut doc.styles[at], style);
        return check_or_rollback(doc, before, Command::UpsertStyle { style: old });
    }
    let id = style.id.clone();
    insert_style(doc, doc.styles.len(), style)?;
    check_or_rollback(doc, before, Command::DeleteStyle { id, detach: false })
}

fn insert_style(doc: &mut Document, index: usize, style: Style) -> Result<(), CommandError> {
    let id: &Id = &style.id;
    id.check(IdKind::Style)
        .map_err(|e| invariant(id, e.to_string()))?;
    let mut in_masters = false;
    for c in &doc.components {
        c.root.walk(&mut |n| in_masters |= &n.id == id);
    }
    if index > doc.styles.len() {
        return Err(CommandError::IndexOutOfRange {
            index,
            len: doc.styles.len(),
        });
    }
    if doc.path_of(id).is_some() || in_masters || non_node_ids(doc).contains(id) {
        return Err(CommandError::DuplicateId(id.to_string()));
    }
    doc.styles.insert(index, style);
    Ok(())
}

pub(crate) fn insert_style_at(
    doc: &mut Document,
    index: usize,
    style: Style,
    check: bool,
) -> Result<Command, CommandError> {
    let before = problems_of(doc, check);
    let id = style.id.clone();
    insert_style(doc, index, style)?;
    check_or_rollback(doc, before, Command::DeleteStyle { id, detach: false })
}

/// The nodes (pages and component masters) that reference style `id`, with
/// the patch that detaches each from it.
fn detach_patches(doc: &Document, id: &StyleId) -> Vec<(NodeId, NodePatch)> {
    let mut out = Vec::new();
    let mut visit = |n: &Node| {
        if let Some(p) = detach_style_patch(doc, n, Some(id)) {
            out.push((n.id.clone(), p));
        }
    };
    doc.walk_nodes(&mut visit);
    for c in &doc.components {
        c.root.walk(&mut visit);
    }
    out
}

pub(crate) fn delete_style(
    doc: &mut Document,
    id: StyleId,
    detach: bool,
    check: bool,
) -> Result<Command, CommandError> {
    let at = doc
        .styles
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let users = detach_patches(doc, &id);
    if !detach && !users.is_empty() {
        let names: Vec<String> = users.iter().map(|(n, _)| n.to_string()).collect();
        return Err(invariant(
            &id,
            format!("style is still used by {}", names.join(", ")),
        ));
    }
    let cmds = users
        .into_iter()
        .map(|(id, patch)| Command::SetProps { id, patch })
        .collect();
    let mut inverses = apply_all(doc, cmds, check)?;
    let style = doc.styles.remove(at);
    inverses.push(Command::InsertStyle { index: at, style });
    // Undo re-inserts the style first, then restores the references.
    inverses.reverse();
    Ok(match inverses.len() {
        1 => inverses.remove(0),
        _ => Command::Batch(inverses),
    })
}
