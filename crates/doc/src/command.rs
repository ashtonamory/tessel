//! Commands (§5.1–5.2): the only way to change a `Document`. Applying a
//! command returns its exact inverse; on error the document is unchanged.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::mem;

use serde::{Deserialize, Deserializer, Serialize};

use crate::components;
use crate::id::{ComponentId, Id, IdKind, NodeId, PageId, StyleId};
use crate::index::ParentRef;
use crate::model::*;
use crate::tokens_styles::{
    delete_style, insert_style_at, rename_token, upsert_style, upsert_tokens, TokensPatch,
};
use crate::validate::{check_one, check_subtree, component_problems, Problem};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Insert `node` (which may carry a subtree) at `index`, or at the end.
    CreateNode {
        parent: ParentRef,
        index: Option<usize>,
        node: Node,
    },
    DeleteNode {
        id: NodeId,
    },
    /// Reparent and/or reorder. `index` is the position in the new parent's
    /// children after the node has been taken out; `None` appends.
    MoveNode {
        id: NodeId,
        parent: ParentRef,
        index: Option<usize>,
    },
    SetProps {
        id: NodeId,
        patch: NodePatch,
    },
    SetGeometry {
        id: NodeId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        rotation: f32,
    },
    CreatePage {
        index: Option<usize>,
        page: Page,
    },
    DeletePage {
        id: PageId,
    },
    UpdatePage {
        id: PageId,
        name: Option<String>,
        background: Option<Color>,
    },
    /// Add, change or remove tokens (§3.6). Removing a referenced token is
    /// an error.
    UpsertTokens {
        patch: TokensPatch,
    },
    /// Rename a token and rewrite every reference to it. Errors if `to`
    /// exists.
    RenameToken {
        from: TokenId,
        to: TokenId,
    },
    /// Replace the style with the same ID, or append a new one.
    UpsertStyle {
        style: Style,
    },
    /// Insert a new style at `index` in the style list. This is what undoes
    /// a `DeleteStyle`.
    InsertStyle {
        index: usize,
        style: Style,
    },
    /// Delete a style. With `detach`, nodes that use it first get its values
    /// copied onto them; without, it is an error while the style is used.
    DeleteStyle {
        id: StyleId,
        detach: bool,
    },
    /// Turn the frame `from` (on a page) into a component: the frame moves
    /// to `Document::components` as the component's root (with a new ID) and
    /// an instance of it, with the frame's ID, name and geometry, takes its
    /// place. The new IDs depend only on `from` and the document, so redo
    /// recreates the same ones.
    CreateComponent {
        from: NodeId,
    },
    /// Replace the component with the same ID, or append a new one.
    UpsertComponent {
        component: Component,
    },
    /// Insert a new component at `index` in the component list. This is
    /// what undoes a `DeleteComponent`.
    InsertComponent {
        index: usize,
        component: Component,
    },
    /// Delete a component. With `detach`, its instances are first detached
    /// (and swaps to it cleared); without, it is an error while it is used.
    DeleteComponent {
        id: ComponentId,
        detach: bool,
    },
    /// Turn an instance into an ordinary frame subtree with the overrides
    /// baked in. The frame keeps the instance's ID; the nodes below get
    /// fresh IDs (a pure function of the document, so redo makes the same).
    DetachInstance {
        id: NodeId,
    },
    /// Replace the node `id` (on a page or in a component master) and its
    /// subtree by `node`, which may have another ID. This is what undoes a
    /// `DetachInstance`.
    ReplaceNode {
        id: NodeId,
        node: Node,
    },
    /// Applied in order, atomically.
    Batch(Vec<Command>),
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0} cannot have children")]
    NotAContainer(String),
    #[error("index {index} is out of range (0..={len})")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("ID {0} is already used in the document")]
    DuplicateId(String),
    #[error("cannot move {0} into itself or one of its descendants")]
    Cycle(String),
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
    #[error("invariant violated: {}", .0.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("; "))]
    Invariant(Vec<Problem>),
}

fn invariant(id: &Id, message: impl Into<String>) -> CommandError {
    CommandError::Invariant(vec![Problem {
        id: Some(id.to_string()),
        message: message.into(),
    }])
}

impl Command {
    /// The node IDs this command changes or whose children it changes (§5.5):
    /// created, deleted and moved nodes with their subtrees, patched nodes,
    /// and the node parents involved. Pages are not node IDs, so they only
    /// contribute the nodes of their subtrees. A `DeleteNode` knows only its
    /// target; its inverse (a `CreateNode`) names the parent.
    pub fn touched_ids(&self) -> BTreeSet<NodeId> {
        let mut out = BTreeSet::new();
        self.collect_touched(&mut out);
        out
    }

    fn collect_touched(&self, out: &mut BTreeSet<NodeId>) {
        let parent = |p: &ParentRef, out: &mut BTreeSet<NodeId>| {
            if let ParentRef::Node(id) = p {
                out.insert(id.clone());
            }
        };
        match self {
            Command::CreateNode {
                parent: p, node, ..
            } => {
                parent(p, out);
                node.walk(&mut |n| {
                    out.insert(n.id.clone());
                });
            }
            Command::SetProps { id, .. } => {
                // A derived node changes through its instance's overrides.
                if let Some(i) = crate::expand::owning_instance(id) {
                    out.insert(i);
                }
                out.insert(id.clone());
            }
            Command::DeleteNode { id }
            | Command::SetGeometry { id, .. }
            | Command::CreateComponent { from: id }
            | Command::DetachInstance { id } => {
                out.insert(id.clone());
            }
            Command::ReplaceNode { id, node } => {
                out.insert(id.clone());
                node.walk(&mut |n| {
                    out.insert(n.id.clone());
                });
            }
            // Master nodes are not on any page; layout and scene caches
            // watch `Document::components` for changes to instances.
            Command::UpsertComponent { component } | Command::InsertComponent { component, .. } => {
                component.root.walk(&mut |n| {
                    out.insert(n.id.clone());
                });
            }
            Command::MoveNode { id, parent: p, .. } => {
                out.insert(id.clone());
                parent(p, out);
            }
            Command::CreatePage { page, .. } => {
                for n in &page.children {
                    n.walk(&mut |n| {
                        out.insert(n.id.clone());
                    });
                }
            }
            // Tokens and styles change how any node may draw; they name no
            // nodes, so callers watch `Document::tokens` and `styles`.
            Command::DeletePage { .. }
            | Command::UpdatePage { .. }
            | Command::UpsertTokens { .. }
            | Command::RenameToken { .. }
            | Command::UpsertStyle { .. }
            | Command::InsertStyle { .. }
            | Command::DeleteStyle { .. }
            | Command::DeleteComponent { .. } => {}
            Command::Batch(cmds) => {
                for c in cmds {
                    c.collect_touched(out);
                }
            }
        }
    }

    /// Apply to the document. On success returns the inverse command; on
    /// error the document is unchanged.
    pub fn apply(self, doc: &mut Document) -> Result<Command, CommandError> {
        self.apply_with(doc, true)
    }

    /// Apply without the local invariant checks (structural checks such as
    /// "parent exists" still run). Used to roll back and to undo/redo, which
    /// only ever restore a state the document was already in.
    pub(crate) fn apply_unchecked(self, doc: &mut Document) -> Result<Command, CommandError> {
        self.apply_with(doc, false)
    }

    fn apply_with(self, doc: &mut Document, check: bool) -> Result<Command, CommandError> {
        match self {
            Command::CreateNode {
                parent,
                index,
                node,
            } => create_node(doc, parent, index, node, check),
            Command::DeleteNode { id } => delete_node(doc, id, check),
            Command::MoveNode { id, parent, index } => move_node(doc, id, parent, index, check),
            Command::SetProps { id, patch } => set_props(doc, id, patch, check),
            Command::SetGeometry {
                id,
                x,
                y,
                width,
                height,
                rotation,
            } => set_geometry(doc, id, [x, y, width, height, rotation], check),
            Command::CreatePage { index, page } => create_page(doc, index, page, check),
            Command::DeletePage { id } => delete_page(doc, id),
            Command::UpdatePage {
                id,
                name,
                background,
            } => update_page(doc, id, name, background, check),
            Command::UpsertTokens { patch } => upsert_tokens(doc, patch, check),
            Command::RenameToken { from, to } => rename_token(doc, from, to),
            Command::UpsertStyle { style } => upsert_style(doc, style, check),
            Command::InsertStyle { index, style } => insert_style_at(doc, index, style, check),
            Command::DeleteStyle { id, detach } => delete_style(doc, id, detach, check),
            Command::CreateComponent { from } => components::create_component(doc, from),
            Command::UpsertComponent { component } => {
                components::upsert_component(doc, component, check)
            }
            Command::InsertComponent { index, component } => {
                components::insert_component(doc, index, component, check)
            }
            Command::DeleteComponent { id, detach } => {
                components::delete_component(doc, id, detach, check)
            }
            Command::DetachInstance { id } => components::detach_instance(doc, id, check),
            Command::ReplaceNode { id, node } => components::replace_node(doc, id, node, check),
            Command::Batch(cmds) => {
                let mut inverses = apply_all(doc, cmds, check)?;
                inverses.reverse();
                Ok(Command::Batch(inverses))
            }
        }
    }
}

/// Apply commands in order, atomically. Returns their inverses in the same
/// order as `cmds` (undo applies them back to front).
pub(crate) fn apply_all(
    doc: &mut Document,
    cmds: Vec<Command>,
    check: bool,
) -> Result<Vec<Command>, CommandError> {
    let mut inverses = Vec::with_capacity(cmds.len());
    for c in cmds {
        match c.apply_with(doc, check) {
            Ok(inv) => inverses.push(inv),
            Err(e) => {
                for inv in inverses.into_iter().rev() {
                    inv.apply_unchecked(doc)
                        .expect("the inverse of an applied command always applies");
                }
                return Err(e);
            }
        }
    }
    Ok(inverses)
}

/// Every ID used by pages, styles and components (not nodes; those are in
/// the index).
pub(crate) fn non_node_ids(doc: &Document) -> HashSet<&Id> {
    doc.pages
        .iter()
        .map(|p| &p.id)
        .chain(doc.styles.iter().map(|s| &s.id))
        .chain(doc.components.iter().map(|c| &c.id))
        .collect()
}

/// New node IDs must be unique within the subtree and unused in the document.
fn check_new_ids<'a>(
    doc: &Document,
    nodes: impl IntoIterator<Item = &'a Node>,
) -> Result<(), CommandError> {
    let taken = non_node_ids(doc);
    // Component masters live outside the page index but share the ID space.
    let mut masters = HashSet::new();
    for c in &doc.components {
        c.root.walk(&mut |n| {
            masters.insert(&n.id);
        });
    }
    let mut fresh = HashSet::new();
    let mut dup = None;
    for n in nodes {
        n.walk(&mut |n| {
            if dup.is_none()
                && (doc.path_of(&n.id).is_some()
                    || taken.contains(&n.id)
                    || masters.contains(&n.id)
                    || !fresh.insert(&n.id))
            {
                dup = Some(n.id.to_string());
            }
        });
    }
    dup.map_or(Ok(()), |id| Err(CommandError::DuplicateId(id)))
}

fn parent_name(p: &ParentRef) -> &Id {
    match p {
        ParentRef::Page(id) | ParentRef::Node(id) => id,
    }
}

fn children_len(doc: &Document, parent: &ParentRef) -> Result<usize, CommandError> {
    if let Some(c) = doc.children_of(parent) {
        return Ok(c.len());
    }
    let id = parent_name(parent);
    match parent {
        ParentRef::Node(_) if doc.node(id).is_some() => {
            Err(CommandError::NotAContainer(id.to_string()))
        }
        _ => Err(CommandError::NotFound(id.to_string())),
    }
}

fn check_index(index: Option<usize>, len: usize) -> Result<usize, CommandError> {
    match index {
        None => Ok(len),
        Some(i) if i <= len => Ok(i),
        Some(i) => Err(CommandError::IndexOutOfRange { index: i, len }),
    }
}

fn is_group(doc: &Document, parent: &ParentRef) -> bool {
    matches!(parent, ParentRef::Node(id)
        if matches!(doc.node(id).map(|n| &n.kind), Some(NodeKind::Group(_))))
}

fn create_node(
    doc: &mut Document,
    parent: ParentRef,
    index: Option<usize>,
    node: Node,
    check: bool,
) -> Result<Command, CommandError> {
    let at = check_index(index, children_len(doc, &parent)?)?;
    check_new_ids(doc, [&node])?;
    if check {
        let problems = check_subtree(doc, &node);
        if !problems.is_empty() {
            return Err(CommandError::Invariant(problems));
        }
    }
    let id = node.id.clone();
    doc.children_of_mut(&parent)
        .expect("checked above")
        .insert(at, node);
    doc.rebuild_index();
    Ok(Command::DeleteNode { id })
}

fn delete_node(doc: &mut Document, id: NodeId, check: bool) -> Result<Command, CommandError> {
    let (parent, at) = doc
        .parent_of(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    if check && is_group(doc, &parent) && children_len(doc, &parent)? == 1 {
        return Err(invariant(
            parent_name(&parent),
            "a group must have at least one child",
        ));
    }
    let node = doc
        .children_of_mut(&parent)
        .expect("parent exists")
        .remove(at);
    doc.rebuild_index();
    Ok(Command::CreateNode {
        parent,
        index: Some(at),
        node,
    })
}

fn move_node(
    doc: &mut Document,
    id: NodeId,
    parent: ParentRef,
    index: Option<usize>,
    check: bool,
) -> Result<Command, CommandError> {
    let (old_parent, old_at) = doc
        .parent_of(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let mut len = children_len(doc, &parent)?;
    if let ParentRef::Node(target) = &parent {
        if doc.node(&id).is_some_and(|n| n.contains(target)) {
            return Err(CommandError::Cycle(id.to_string()));
        }
    }
    if old_parent == parent {
        len -= 1;
    } else if check && is_group(doc, &old_parent) && children_len(doc, &old_parent)? == 1 {
        return Err(invariant(
            parent_name(&old_parent),
            "a group must have at least one child",
        ));
    }
    let at = check_index(index, len)?;
    let node = doc
        .children_of_mut(&old_parent)
        .expect("parent exists")
        .remove(old_at);
    doc.rebuild_index();
    doc.children_of_mut(&parent)
        .expect("checked above")
        .insert(at, node);
    doc.rebuild_index();
    Ok(Command::MoveNode {
        id,
        parent: old_parent,
        index: Some(old_at),
    })
}

/// Problems of one node before an edit, if checking.
fn problems_before(doc: &Document, id: &NodeId, check: bool) -> Option<Vec<Problem>> {
    check.then(|| {
        doc.node_or_master(id)
            .map(|n| check_one(doc, n))
            .unwrap_or_default()
    })
}

/// After an edit of one node: if it introduced problems that were not there
/// before, undo it with `rollback` and fail.
fn check_or_rollback(
    doc: &mut Document,
    id: &NodeId,
    before: Option<Vec<Problem>>,
    rollback: Command,
) -> Result<Command, CommandError> {
    let Some(before) = before else {
        return Ok(rollback);
    };
    let node = doc.node_or_master(id).expect("node exists");
    let new: Vec<Problem> = check_one(doc, node)
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    if new.is_empty() {
        return Ok(rollback);
    }
    rollback
        .apply_unchecked(doc)
        .expect("restoring previous values always applies");
    Err(CommandError::Invariant(new))
}

fn set_props(
    doc: &mut Document,
    id: NodeId,
    patch: NodePatch,
    check: bool,
) -> Result<Command, CommandError> {
    if id.is_derived() {
        return set_derived_props(doc, id, patch, check);
    }
    let before = problems_before(doc, &id, check);
    // Swaps and component changes inside a master can make a cycle.
    let in_master = doc.path_of(&id).is_none();
    let components_before =
        (check && in_master && (patch.component.is_some() || patch.overrides.is_some()))
            .then(|| component_problems(doc));
    let node = doc
        .node_or_master_mut(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let inverse = patch.apply_to(node)?;
    let inverse = Command::SetProps {
        id: id.clone(),
        patch: inverse,
    };
    if let Some(before) = components_before {
        let new: Vec<Problem> = component_problems(doc)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        if !new.is_empty() {
            inverse
                .apply_unchecked(doc)
                .expect("restoring previous values always applies");
            return Err(CommandError::Invariant(new));
        }
    }
    check_or_rollback(doc, &id, before, inverse)
}

/// `SetProps` on a node inside an instance (§3.7): derived nodes are
/// read-only except for what an override can change, so the patch becomes
/// an override on the instance stored in the document. Its inverse is a
/// `SetProps` of that instance's overrides.
fn set_derived_props(
    doc: &mut Document,
    id: NodeId,
    mut patch: NodePatch,
    check: bool,
) -> Result<Command, CommandError> {
    let d = crate::expand::resolve_derived(doc, &id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let mut overrides = Vec::new();
    if let Some(content) = patch.content.take() {
        overrides.push(("content", Override::Text { content }));
    }
    if let Some(fills) = patch.fills.take() {
        overrides.push(("fills", Override::Fills { fills }));
    }
    if let Some(visible) = patch.visible.take() {
        overrides.push(("visible", Override::Visible { visible }));
    }
    if let Some(component) = patch.component.take() {
        overrides.push(("component", Override::Swap { component }));
    }
    if patch != NodePatch::default() {
        return Err(CommandError::InvalidPatch(format!(
            "{id} is inside an instance: only content, fills, visible and component \
             (as overrides) can change"
        )));
    }
    if let Some((field, _)) = overrides.iter().find(|(f, _)| !applies(f, &d.node.kind)) {
        return Err(CommandError::InvalidPatch(format!(
            "{field} not applicable to a {} node",
            d.node.kind.tag()
        )));
    }
    let o = match overrides.len() {
        0 => {
            return Ok(Command::SetProps {
                id,
                patch: NodePatch::default(),
            })
        }
        1 => overrides.pop().expect("one").1,
        _ => {
            return Err(CommandError::InvalidPatch(format!(
                "{id} can hold one override; set {} separately",
                overrides
                    .iter()
                    .map(|(f, _)| *f)
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    };
    let patch = NodePatch {
        overrides: Some(BTreeMap::from([(d.key, Some(o))])),
        ..NodePatch::default()
    };
    set_props(doc, d.instance, patch, check)
}

fn set_geometry(
    doc: &mut Document,
    id: NodeId,
    g: [f32; 5],
    check: bool,
) -> Result<Command, CommandError> {
    let before = problems_before(doc, &id, check);
    let n = doc
        .node_mut(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let old = [n.x, n.y, n.width, n.height, n.rotation];
    [n.x, n.y, n.width, n.height, n.rotation] = g;
    let [x, y, width, height, rotation] = old;
    let inverse = Command::SetGeometry {
        id: id.clone(),
        x,
        y,
        width,
        height,
        rotation,
    };
    check_or_rollback(doc, &id, before, inverse)
}

fn create_page(
    doc: &mut Document,
    index: Option<usize>,
    page: Page,
    check: bool,
) -> Result<Command, CommandError> {
    let at = check_index(index, doc.pages.len())?;
    page.id
        .check(IdKind::Page)
        .map_err(|e| invariant(&page.id, e.to_string()))?;
    if non_node_ids(doc).contains(&page.id) || doc.path_of(&page.id).is_some() {
        return Err(CommandError::DuplicateId(page.id.to_string()));
    }
    check_new_ids(doc, &page.children)?;
    if check {
        if !page.background.is_well_formed() {
            return Err(invariant(&page.id, "background is not a #rrggbb[aa] color"));
        }
        let problems: Vec<Problem> = page
            .children
            .iter()
            .flat_map(|n| check_subtree(doc, n))
            .collect();
        if !problems.is_empty() {
            return Err(CommandError::Invariant(problems));
        }
    }
    let id = page.id.clone();
    doc.pages.insert(at, page);
    doc.rebuild_index();
    Ok(Command::DeletePage { id })
}

fn delete_page(doc: &mut Document, id: PageId) -> Result<Command, CommandError> {
    let at = doc
        .page_index(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let page = doc.pages.remove(at);
    doc.rebuild_index();
    Ok(Command::CreatePage {
        index: Some(at),
        page,
    })
}

fn update_page(
    doc: &mut Document,
    id: PageId,
    name: Option<String>,
    background: Option<Color>,
    check: bool,
) -> Result<Command, CommandError> {
    let at = doc
        .page_index(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    if check && background.as_ref().is_some_and(|c| !c.is_well_formed()) {
        return Err(invariant(&id, "background is not a #rrggbb[aa] color"));
    }
    let page = &mut doc.pages[at];
    Ok(Command::UpdatePage {
        id,
        name: name.map(|v| mem::replace(&mut page.name, v)),
        background: background.map(|v| mem::replace(&mut page.background, v)),
    })
}

// ---------------------------------------------------------------------------
// NodePatch

/// Distinguishes a missing key (`None`) from `null` (`Some(None)`).
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Any subset of a node's props, in file-format JSON (§5.1). Unknown keys
/// are an error. For optional props, `null` clears the value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePatch {
    // Common to all kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_child: Option<LayoutChild>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Constraints>,

    // Shape: frame, rect, ellipse, vector (fills also text; corner_radius
    // also image).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fills: Option<Vec<Paint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strokes: Option<Vec<Paint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_align: Option<StrokeAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<CornerRadius>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadows: Option<Vec<Shadow>>,
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub fill_style: Option<Option<StyleId>>,
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub stroke_style: Option<Option<StyleId>>,
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub effect_style: Option<Option<StyleId>>,

    // Frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<bool>,
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub layout: Option<Option<AutoLayout>>,
    /// Frame and text.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub semantic: Option<Option<String>>,

    // Text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub text_style: Option<Option<StyleId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_align: Option<VAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizing: Option<TextSizing>,

    // Image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<ImageFit>,

    // Vector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    // Instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    /// Per key: set the override, or with `null` clear it. Keys not named
    /// keep their override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<BTreeMap<String, Option<Override>>>,
}

/// Which kinds a kind-specific patch field applies to.
fn applies(field: &str, kind: &NodeKind) -> bool {
    use NodeKind as K;
    let shaped = matches!(
        kind,
        K::Frame(_) | K::Rect(_) | K::Ellipse(_) | K::Vector(_)
    );
    match field {
        "fills" => shaped || matches!(kind, K::Text(_)),
        "corner_radius" => shaped || matches!(kind, K::Image(_)),
        "strokes" | "stroke_width" | "stroke_align" | "shadows" | "fill_style" | "stroke_style"
        | "effect_style" => shaped,
        "clip" | "layout" => matches!(kind, K::Frame(_)),
        "semantic" => matches!(kind, K::Frame(_) | K::Text(_)),
        "content" | "text_style" | "style" | "align" | "vertical_align" | "sizing" => {
            matches!(kind, K::Text(_))
        }
        "asset" | "fit" => matches!(kind, K::Image(_)),
        "path" => matches!(kind, K::Vector(_)),
        "component" | "overrides" => matches!(kind, K::Instance(_)),
        _ => true,
    }
}

macro_rules! swap {
    ($patch:ident, $inv:ident, $field:ident => $target:expr) => {
        if let Some(v) = $patch.$field.take() {
            $inv.$field = Some(mem::replace(&mut $target, v));
        }
    };
}

macro_rules! swap_shape {
    ($patch:ident, $inv:ident, $shape:expr) => {
        swap!($patch, $inv, fills => $shape.fills);
        swap!($patch, $inv, strokes => $shape.strokes);
        swap!($patch, $inv, stroke_width => $shape.stroke_width);
        swap!($patch, $inv, stroke_align => $shape.stroke_align);
        swap!($patch, $inv, corner_radius => $shape.corner_radius);
        swap!($patch, $inv, shadows => $shape.shadows);
        swap!($patch, $inv, fill_style => $shape.fill_style);
        swap!($patch, $inv, stroke_style => $shape.stroke_style);
        swap!($patch, $inv, effect_style => $shape.effect_style);
    };
}

impl NodePatch {
    /// Parse a patch from JSON; unknown keys are an error.
    pub fn from_json(v: serde_json::Value) -> Result<NodePatch, CommandError> {
        serde_json::from_value(v).map_err(|e| CommandError::InvalidPatch(e.to_string()))
    }

    /// The kind-specific fields that are set.
    fn kind_fields(&self) -> Vec<&'static str> {
        let mut f = Vec::new();
        macro_rules! set {
            ($($name:ident),*) => { $(if self.$name.is_some() { f.push(stringify!($name)); })* };
        }
        set!(
            fills,
            strokes,
            stroke_width,
            stroke_align,
            corner_radius,
            shadows,
            fill_style,
            stroke_style,
            effect_style,
            clip,
            layout,
            semantic,
            content,
            text_style,
            style,
            align,
            vertical_align,
            sizing,
            asset,
            fit,
            path,
            component,
            overrides
        );
        f
    }

    /// Apply to `node`, returning a patch holding the previous values of
    /// exactly the patched fields. Fails, leaving `node` untouched, if a
    /// field does not apply to the node's kind.
    pub fn apply_to(mut self, node: &mut Node) -> Result<NodePatch, CommandError> {
        let bad: Vec<_> = self
            .kind_fields()
            .into_iter()
            .filter(|f| !applies(f, &node.kind))
            .collect();
        if !bad.is_empty() {
            return Err(CommandError::InvalidPatch(format!(
                "{} not applicable to a {} node",
                bad.join(", "),
                node.kind.tag()
            )));
        }
        let p = &mut self;
        let mut inv = NodePatch::default();
        swap!(p, inv, name => node.name);
        swap!(p, inv, x => node.x);
        swap!(p, inv, y => node.y);
        swap!(p, inv, width => node.width);
        swap!(p, inv, height => node.height);
        swap!(p, inv, rotation => node.rotation);
        swap!(p, inv, visible => node.visible);
        swap!(p, inv, locked => node.locked);
        swap!(p, inv, opacity => node.opacity);
        swap!(p, inv, layout_child => node.layout_child);
        swap!(p, inv, constraints => node.constraints);
        match &mut node.kind {
            NodeKind::Frame(f) => {
                swap_shape!(p, inv, f.shape);
                swap!(p, inv, clip => f.clip);
                swap!(p, inv, layout => f.layout);
                swap!(p, inv, semantic => f.semantic);
            }
            NodeKind::Rect(r) => {
                swap_shape!(p, inv, r.shape);
            }
            NodeKind::Ellipse(e) => {
                swap_shape!(p, inv, e.shape);
            }
            NodeKind::Vector(v) => {
                swap_shape!(p, inv, v.shape);
                swap!(p, inv, path => v.path);
            }
            NodeKind::Text(t) => {
                swap!(p, inv, fills => t.fills);
                swap!(p, inv, content => t.content);
                swap!(p, inv, text_style => t.text_style);
                swap!(p, inv, style => t.style);
                swap!(p, inv, align => t.align);
                swap!(p, inv, vertical_align => t.vertical_align);
                swap!(p, inv, sizing => t.sizing);
                swap!(p, inv, semantic => t.semantic);
            }
            NodeKind::Image(i) => {
                swap!(p, inv, asset => i.asset);
                swap!(p, inv, fit => i.fit);
                swap!(p, inv, corner_radius => i.corner_radius);
            }
            NodeKind::Instance(i) => {
                swap!(p, inv, component => i.component);
                if let Some(patch) = p.overrides.take() {
                    let mut old = BTreeMap::new();
                    for (k, v) in patch {
                        let prev = match v {
                            Some(v) => i.overrides.insert(k.clone(), v),
                            None => i.overrides.remove(&k),
                        };
                        old.insert(k, prev);
                    }
                    inv.overrides = Some(old);
                }
            }
            NodeKind::Group(_) => {}
        }
        debug_assert_eq!(*p, NodePatch::default(), "every patch field was applied");
        Ok(inv)
    }
}
