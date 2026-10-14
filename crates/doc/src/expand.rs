//! Instance expansion (§3.7): the children an instance shows, derived from
//! its component's root with the instance's overrides applied.
//!
//! Expansion is one level deep. [`expand_instance`] turns an instance into
//! the frame it stands for: the component root with the instance's own ID,
//! name, geometry and visibility, and the root's children re-identified as
//! `<instanceId>/<componentNodeId>`. A nested instance in that tree stays an
//! instance, carrying everything it needs to expand in turn: its derived ID,
//! its component (after a `Swap`) and its overrides merged with the outer
//! instance's. Its own children are then `<instanceId>/<nestedId>/<nodeId>`.
//!
//! Override keys are relative to the instance that holds them: a node of
//! the component root is `<componentNodeId>`; a node inside a nested
//! instance is `<nestedInstanceNodeId>/<nodeId>`, and so on. An outer
//! instance's override wins over the nested instance's own. Overrides that
//! name no node, or do not fit the node's kind, are ignored.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::id::{ComponentId, Id, IdKind, NodeId};
use crate::model::*;

/// Instances nest at most this deep; deeper ones (only possible in a file
/// with a component cycle, which validation reports) do not expand.
pub const MAX_NESTING: usize = 32;

/// One instance may expand to at most this many nodes (the instance itself
/// included, nested instances counted fully expanded). Nesting alone does not
/// bound this: components that each hold several instances of the next grow
/// exponentially. An instance over the budget stays unexpanded, and
/// validation reports it.
pub const MAX_EXPANDED_NODES: usize = 100_000;

/// The component with this ID.
pub fn component<'a>(doc: &'a Document, id: &ComponentId) -> Option<&'a Component> {
    doc.components.iter().find(|c| &c.id == id)
}

/// The frame `instance` stands for, one level deep (see the module docs).
/// `None` if the node is not an instance, its component is missing or its
/// root is not a frame, it is nested deeper than [`MAX_NESTING`], or its full
/// expansion would pass [`MAX_EXPANDED_NODES`].
pub fn expand_instance(doc: &Document, instance: &Node) -> Option<Node> {
    if !within_budget(doc, instance) {
        return None;
    }
    expand_unchecked(doc, instance)
}

/// Whether `instance`, expanded all the way down, stays within
/// [`MAX_EXPANDED_NODES`]. Counts without building anything and stops as soon
/// as the budget is spent. Anything that is not an instance is within budget.
pub fn within_budget(doc: &Document, instance: &Node) -> bool {
    let NodeKind::Instance(_) = &instance.kind else {
        return true;
    };
    let depth = instance.id.as_str().matches('/').count();
    let mut budget = MAX_EXPANDED_NODES;
    count_nodes(doc, instance, &BTreeMap::new(), depth, &mut budget).is_some()
}

/// Spend one unit of `budget` per node of `node`'s subtree as expansion would
/// show it. `overrides` are those of the instance holding `node`, `depth` is
/// the number of `/` in the ID `node` would have. `None` once the budget is
/// out.
fn count_nodes(
    doc: &Document,
    node: &Node,
    overrides: &BTreeMap<String, Override>,
    depth: usize,
    budget: &mut usize,
) -> Option<()> {
    *budget = budget.checked_sub(1)?;
    if let NodeKind::Instance(inst) = &node.kind {
        if depth >= MAX_NESTING {
            return Some(());
        }
        let key = node.id.as_str();
        let target = match overrides.get(key) {
            Some(Override::Swap { component }) => component,
            _ => &inst.component,
        };
        let Some(comp) =
            component(doc, target).filter(|c| matches!(c.root.kind, NodeKind::Frame(_)))
        else {
            return Some(());
        };
        // Most instances carry no overrides; then there is nothing to merge.
        let inner = if overrides.is_empty() {
            Vec::new()
        } else {
            let prefix = format!("{key}/");
            overrides
                .iter()
                .filter_map(|(k, o)| Some((k.strip_prefix(&prefix)?, o)))
                .collect::<Vec<_>>()
        };
        let merged_owned;
        let merged = if inner.is_empty() {
            &inst.overrides
        } else {
            let mut m = inst.overrides.clone();
            m.extend(inner.into_iter().map(|(k, o)| (k.to_string(), o.clone())));
            merged_owned = m;
            &merged_owned
        };
        for c in comp.root.children().into_iter().flatten() {
            count_nodes(doc, c, merged, depth + 1, budget)?;
        }
        return Some(());
    }
    for c in node.children().into_iter().flatten() {
        count_nodes(doc, c, overrides, depth, budget)?;
    }
    Some(())
}

fn expand_unchecked(doc: &Document, instance: &Node) -> Option<Node> {
    let NodeKind::Instance(inst) = &instance.kind else {
        return None;
    };
    if instance.id.as_str().matches('/').count() >= MAX_NESTING {
        return None;
    }
    let comp = component(doc, &inst.component)?;
    if !matches!(comp.root.kind, NodeKind::Frame(_)) {
        return None;
    }
    let mut root = comp.root.clone();
    let master = [root.width, root.height];
    root.id = instance.id.clone();
    root.name = instance.name.clone();
    root.visible = instance.visible;
    root.locked = instance.locked;
    root.x = instance.x;
    root.y = instance.y;
    root.width = instance.width;
    root.height = instance.height;
    root.rotation = instance.rotation;
    root.opacity = instance.opacity;
    root.layout_child = instance.layout_child.clone();
    root.constraints = instance.constraints.clone();
    if let Some(o) = inst.overrides.get(comp.root.id.as_str()) {
        apply_override(&mut root, o);
    }
    let auto_layout = matches!(&root.kind, NodeKind::Frame(f) if f.layout.is_some());
    let size = [root.width, root.height];
    for c in root.children_mut().expect("a frame") {
        // Children that auto-layout places keep their size; the rest follow
        // their constraints from the master's size to the instance's.
        let flow = c.visible && c.layout_child.position == ChildPosition::Flow;
        if size != master && !(auto_layout && flow) {
            [c.x, c.y, c.width, c.height] =
                constrain(master, size, [c.x, c.y, c.width, c.height], &c.constraints);
        }
        derive(c, &instance.id, &inst.overrides);
    }
    Some(root)
}

/// Re-identify a component node (and its subtree) as seen from `instance`,
/// applying `overrides`.
fn derive(node: &mut Node, instance: &NodeId, overrides: &BTreeMap<String, Override>) {
    let key = node.id.as_str().to_string();
    if let Some(o) = overrides.get(&key) {
        apply_override(node, o);
    }
    if let NodeKind::Instance(nested) = &mut node.kind {
        let prefix = format!("{key}/");
        for (k, o) in overrides {
            if let Some(inner) = k.strip_prefix(&prefix) {
                nested.overrides.insert(inner.to_string(), o.clone());
            }
        }
    }
    node.id = Id::derived(instance, &node.id);
    if let Some(children) = node.children_mut() {
        for c in children {
            derive(c, instance, overrides);
        }
    }
}

/// Apply one override to a node, if it fits the node's kind.
pub fn apply_override(node: &mut Node, o: &Override) {
    match (o, &mut node.kind) {
        (Override::Visible { visible }, _) => node.visible = *visible,
        (Override::Text { content }, NodeKind::Text(t)) => t.content = content.clone(),
        (Override::Fills { fills }, NodeKind::Text(t)) => t.fills = fills.clone(),
        (Override::Fills { fills }, _) => {
            if let Some(s) = node.shape_mut() {
                s.fills = fills.clone();
                s.fill_style = None;
            }
        }
        (Override::Swap { component }, NodeKind::Instance(i)) => i.component = component.clone(),
        _ => {}
    }
}

/// A node with every instance in its subtree replaced, recursively, by the
/// frame it stands for. Instances that cannot expand are kept as they are.
pub fn expand_deep(doc: &Document, node: &Node) -> Node {
    let mut n = expand_instance(doc, node).unwrap_or_else(|| node.clone());
    if let Some(children) = n.children_mut() {
        for c in children {
            *c = expand_deep(doc, c);
        }
    }
    n
}

/// A node shown inside an instance, found by its derived ID.
#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    /// The node as the instance shows it: overrides applied, derived IDs.
    /// A nested instance is not expanded (see [`expand_instance`]).
    pub node: Node,
    /// The instance stored in the document (on a page or in a component
    /// master) that the ID starts with.
    pub instance: NodeId,
    /// The override key for this node in `instance`: the ID after
    /// `<instance>/`.
    pub key: String,
}

/// Resolve a derived ID `<instanceId>/<componentNodeId>[/…]` to the node it
/// names. `None` for plain IDs and IDs that name nothing.
pub fn resolve_derived(doc: &Document, id: &NodeId) -> Option<Derived> {
    let mut segments = id.as_str().split('/');
    let first = Id::from_raw(segments.next()?);
    let mut cur = doc.node_or_master(&first)?.clone();
    let mut found = None;
    let mut prefix = first.as_str().to_string();
    for seg in segments {
        let expanded = expand_instance(doc, &cur)?;
        prefix = format!("{prefix}/{seg}");
        let mut hit = None;
        for c in expanded.children()? {
            c.walk(&mut |n| {
                if hit.is_none() && n.id.as_str() == prefix {
                    hit = Some(n.clone());
                }
            });
        }
        let n = hit?;
        cur = n.clone();
        found = Some(n);
    }
    let node = found?;
    let key = id.as_str()[first.as_str().len() + 1..].to_string();
    Some(Derived {
        node,
        instance: first,
        key,
    })
}

/// A node of a page or component master, or a derived node of an instance,
/// as it is shown.
pub fn shown_node(doc: &Document, id: &NodeId) -> Option<Node> {
    if id.is_derived() {
        resolve_derived(doc, id).map(|d| d.node)
    } else {
        doc.node_or_master(id).cloned()
    }
}

/// The instance stored in the document that a derived ID belongs to (the
/// part before the first `/`), or `None` for a plain ID.
pub fn owning_instance(id: &NodeId) -> Option<NodeId> {
    let (first, _) = id.as_str().split_once('/')?;
    Some(Id::from_raw(first))
}

/// Every component used by `node`'s subtree: instances and swaps, and what
/// those components use in turn. A cycle stops at the repeated component.
pub fn components_used(doc: &Document, node: &Node) -> std::collections::BTreeSet<ComponentId> {
    let mut out = std::collections::BTreeSet::new();
    let mut todo: Vec<ComponentId> = Vec::new();
    fn direct(n: &Node, todo: &mut Vec<ComponentId>) {
        n.walk(&mut |n| {
            if let NodeKind::Instance(i) = &n.kind {
                todo.push(i.component.clone());
                for o in i.overrides.values() {
                    if let Override::Swap { component } = o {
                        todo.push(component.clone());
                    }
                }
            }
        });
    }
    direct(node, &mut todo);
    while let Some(c) = todo.pop() {
        if out.insert(c.clone()) {
            if let Some(comp) = component(doc, &c) {
                direct(&comp.root, &mut todo);
            }
        }
    }
    out
}

/// A child's box `[x, y, width, height]` after its parent goes from `old`
/// to `new` (`[width, height]`), following its constraints. The same rules
/// as `layout::apply_constraints`, which the app applies on resize.
pub fn constrain(old: [f32; 2], new: [f32; 2], child: [f32; 4], c: &Constraints) -> [f32; 4] {
    fn axis(kind: u8, old: f32, new: f32, pos: f32, len: f32) -> (f32, f32) {
        let d = new - old;
        match kind {
            0 => (pos, len),
            1 => (pos + d, len),
            2 => (pos, (len + d).max(0.0)),
            3 => (pos + d / 2.0, len),
            _ if old > 0.0 => (pos * (new / old), len * (new / old)),
            _ => (pos, len),
        }
    }
    let h = match c.horizontal {
        HConstraint::Left => 0,
        HConstraint::Right => 1,
        HConstraint::LeftRight => 2,
        HConstraint::Center => 3,
        HConstraint::Scale => 4,
    };
    let v = match c.vertical {
        VConstraint::Top => 0,
        VConstraint::Bottom => 1,
        VConstraint::TopBottom => 2,
        VConstraint::Center => 3,
        VConstraint::Scale => 4,
    };
    let (x, w) = axis(h, old[0], new[0], child[0], child[2]);
    let (y, ht) = axis(v, old[1], new[1], child[1], child[3]);
    [x, y, w, ht]
}

/// A new ID of `kind` that is a pure function of `seed` and of which IDs are
/// `taken`, so re-applying a command to the same document (redo) makes the
/// same IDs.
pub(crate) fn fresh_id(kind: IdKind, seed: &str, taken: impl Fn(&Id) -> bool) -> Id {
    (0u64..)
        .map(|attempt| {
            let h = Sha256::new()
                .chain_update(seed.as_bytes())
                .chain_update(attempt.to_le_bytes())
                .finalize();
            let bits = u64::from_le_bytes(h[..8].try_into().expect("8 bytes"));
            Id::from_entropy(kind, bits)
        })
        .find(|id| !taken(id))
        .expect("an unused ID")
}
