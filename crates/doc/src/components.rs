//! The component commands (§3.7, §5.1): `CreateComponent`,
//! `UpsertComponent`, `InsertComponent`, `DeleteComponent`,
//! `DetachInstance` and `ReplaceNode`.

use std::collections::{BTreeMap, HashSet};
use std::mem;

use crate::command::{apply_all, non_node_ids, Command, CommandError, NodePatch};
use crate::expand::{expand_instance, fresh_id};
use crate::id::{ComponentId, Id, IdKind, NodeId};
use crate::model::*;
use crate::tokens_styles::{check_or_rollback, problems_of};
use crate::validate::{check_subtree, component_problems, Problem};

fn invariant(id: impl ToString, message: impl Into<String>) -> CommandError {
    CommandError::Invariant(vec![Problem {
        id: Some(id.to_string()),
        message: message.into(),
    }])
}

/// Every ID in use: page nodes, component masters, pages, styles and
/// components.
fn taken_ids(doc: &Document) -> HashSet<Id> {
    let mut ids: HashSet<Id> = non_node_ids(doc).into_iter().cloned().collect();
    ids.extend(doc.node_ids().cloned());
    for c in &doc.components {
        c.root.walk(&mut |n| {
            ids.insert(n.id.clone());
        });
    }
    ids
}

/// `nodes`' IDs must be unique among themselves and unused in the document,
/// except by `freed` (IDs that leave the document in the same edit).
fn check_ids_free(doc: &Document, node: &Node, freed: &HashSet<Id>) -> Result<(), CommandError> {
    let taken = taken_ids(doc);
    let mut fresh = HashSet::new();
    let mut dup = None;
    node.walk(&mut |n| {
        if dup.is_none()
            && ((taken.contains(&n.id) && !freed.contains(&n.id)) || !fresh.insert(&n.id))
        {
            dup = Some(n.id.to_string());
        }
    });
    dup.map_or(Ok(()), |id| Err(CommandError::DuplicateId(id)))
}

fn subtree_ids(node: &Node) -> HashSet<Id> {
    let mut ids = HashSet::new();
    node.walk(&mut |n| {
        ids.insert(n.id.clone());
    });
    ids
}

pub(crate) fn create_component(doc: &mut Document, from: NodeId) -> Result<Command, CommandError> {
    let (parent, at) = doc
        .parent_of(&from)
        .ok_or_else(|| CommandError::NotFound(from.to_string()))?;
    let frame = doc.node(&from).expect("indexed");
    if !matches!(frame.kind, NodeKind::Frame(_)) {
        return Err(invariant(
            &from,
            format!(
                "only a frame can become a component, not a {}",
                frame.kind.tag()
            ),
        ));
    }
    let taken = taken_ids(doc);
    let id = fresh_id(IdKind::Component, &format!("component:{from}"), |i| {
        taken.contains(i)
    });
    let root_id = fresh_id(IdKind::Node, &format!("component-root:{from}"), |i| {
        taken.contains(i)
    });
    let instance = Node {
        kind: NodeKind::Instance(Instance {
            component: id.clone(),
            overrides: BTreeMap::new(),
            extra: Extra::new(),
        }),
        ..frame.clone()
    };
    let children = doc.children_of_mut(&parent).expect("parent exists");
    let frame = mem::replace(&mut children[at], instance);
    let mut root = frame.clone();
    root.id = root_id;
    let component = Component {
        id: id.clone(),
        name: frame.name.clone(),
        props: Vec::new(),
        extra: Extra::new(),
        root,
    };
    // Undo empties the master first, so the frame can take its children's
    // IDs back, then swaps the frame in for the instance (in place, so a
    // group never goes empty) and drops the component.
    let mut emptied = component.clone();
    emptied.root.children_mut().expect("a frame").clear();
    doc.components.push(component);
    doc.rebuild_index();
    Ok(Command::Batch(vec![
        Command::UpsertComponent { component: emptied },
        Command::ReplaceNode {
            id: from,
            node: frame,
        },
        Command::DeleteComponent { id, detach: false },
    ]))
}

fn component_index(doc: &Document, id: &ComponentId) -> Option<usize> {
    doc.components.iter().position(|c| &c.id == id)
}

pub(crate) fn upsert_component(
    doc: &mut Document,
    component: Component,
    check: bool,
) -> Result<Command, CommandError> {
    let Some(at) = component_index(doc, &component.id) else {
        return insert_component(doc, doc.components.len(), component, check);
    };
    let before = problems_of(doc, check);
    check_ids_free(doc, &component.root, &subtree_ids(&doc.components[at].root))?;
    let old = mem::replace(&mut doc.components[at], component);
    check_or_rollback(doc, before, Command::UpsertComponent { component: old })
}

pub(crate) fn insert_component(
    doc: &mut Document,
    index: usize,
    component: Component,
    check: bool,
) -> Result<Command, CommandError> {
    let id = component.id.clone();
    id.check(IdKind::Component)
        .map_err(|e| invariant(&id, e.to_string()))?;
    if index > doc.components.len() {
        return Err(CommandError::IndexOutOfRange {
            index,
            len: doc.components.len(),
        });
    }
    if taken_ids(doc).contains(&id) {
        return Err(CommandError::DuplicateId(id.to_string()));
    }
    check_ids_free(doc, &component.root, &HashSet::new())?;
    let before = problems_of(doc, check);
    doc.components.insert(index, component);
    check_or_rollback(doc, before, Command::DeleteComponent { id, detach: false })
}

/// What refers to component `id` outside its own master: instances of it,
/// and `(instance, override key)` of swaps to it.
fn users(doc: &Document, id: &ComponentId) -> (Vec<NodeId>, Vec<(NodeId, String)>) {
    let mut instances = Vec::new();
    let mut swaps = Vec::new();
    let mut visit = |n: &Node| {
        if let NodeKind::Instance(i) = &n.kind {
            if &i.component == id {
                instances.push(n.id.clone());
                return;
            }
            for (k, o) in &i.overrides {
                if matches!(o, Override::Swap { component } if component == id) {
                    swaps.push((n.id.clone(), k.clone()));
                }
            }
        }
    };
    doc.walk_nodes(&mut visit);
    for c in doc.components.iter().filter(|c| &c.id != id) {
        c.root.walk(&mut visit);
    }
    (instances, swaps)
}

pub(crate) fn delete_component(
    doc: &mut Document,
    id: ComponentId,
    detach: bool,
    check: bool,
) -> Result<Command, CommandError> {
    let at = component_index(doc, &id).ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let (instances, swaps) = users(doc, &id);
    if !detach && !(instances.is_empty() && swaps.is_empty()) {
        let names: Vec<String> = instances
            .iter()
            .chain(swaps.iter().map(|(n, _)| n))
            .map(|n| n.to_string())
            .collect();
        return Err(invariant(
            &id,
            format!("component is still used by {}", names.join(", ")),
        ));
    }
    // Detaching can bring new users to light (a detached instance whose
    // overrides swapped a nested instance to this component), so repeat
    // until there are none. Without cycles that ends within the nesting
    // depth.
    let mut inverses = Vec::new();
    for _ in 0..=crate::expand::MAX_NESTING {
        let (instances, swaps) = users(doc, &id);
        if instances.is_empty() && swaps.is_empty() {
            break;
        }
        let mut cmds: Vec<Command> = instances
            .iter()
            .map(|i| Command::DetachInstance { id: i.clone() })
            .collect();
        let mut cleared: BTreeMap<NodeId, BTreeMap<String, Option<Override>>> = BTreeMap::new();
        for (n, key) in swaps {
            cleared.entry(n).or_default().insert(key, None);
        }
        cmds.extend(cleared.into_iter().map(|(n, overrides)| Command::SetProps {
            id: n,
            patch: NodePatch {
                overrides: Some(overrides),
                ..NodePatch::default()
            },
        }));
        match apply_all(doc, cmds, check) {
            Ok(inv) => inverses.extend(inv),
            Err(e) => {
                for inv in inverses.into_iter().rev() {
                    inv.apply_unchecked(doc)
                        .expect("the inverse of an applied command always applies");
                }
                return Err(e);
            }
        }
    }
    if !users(doc, &id).0.is_empty() {
        for inv in inverses.into_iter().rev() {
            inv.apply_unchecked(doc)
                .expect("the inverse of an applied command always applies");
        }
        return Err(invariant(
            &id,
            "component instances nest too deep to detach",
        ));
    }
    let component = doc.components.remove(at);
    inverses.push(Command::InsertComponent {
        index: at,
        component,
    });
    // Undo re-inserts the component first, then restores its instances.
    inverses.reverse();
    Ok(match inverses.len() {
        1 => inverses.remove(0),
        _ => Command::Batch(inverses),
    })
}

/// The ordinary frame subtree an instance detaches to: its expansion, with
/// the instance's ID on the frame and fresh IDs below (derived from the
/// derived IDs, so redo makes the same ones). Nested instances stay
/// instances, with the outer overrides merged into theirs.
pub fn detached(doc: &Document, instance: &Node) -> Result<Node, CommandError> {
    let NodeKind::Instance(i) = &instance.kind else {
        return Err(invariant(&instance.id, "not an instance"));
    };
    let mut frame = expand_instance(doc, instance)
        .ok_or_else(|| invariant(&instance.id, format!("component {} not found", i.component)))?;
    let mut taken = taken_ids(doc);
    for c in frame.children_mut().expect("a frame") {
        reid(c, &mut taken);
    }
    Ok(frame)
}

fn reid(node: &mut Node, taken: &mut HashSet<Id>) {
    let id = fresh_id(IdKind::Node, &format!("detach:{}", node.id), |i| {
        taken.contains(i)
    });
    taken.insert(id.clone());
    node.id = id;
    if let Some(children) = node.children_mut() {
        for c in children {
            reid(c, taken);
        }
    }
}

pub(crate) fn detach_instance(
    doc: &mut Document,
    id: NodeId,
    check: bool,
) -> Result<Command, CommandError> {
    let node = doc
        .node_or_master(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let frame = detached(doc, node)?;
    replace_node(doc, id, frame, check)
}

pub(crate) fn replace_node(
    doc: &mut Document,
    id: NodeId,
    node: Node,
    check: bool,
) -> Result<Command, CommandError> {
    let old = doc
        .node_or_master(&id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    check_ids_free(doc, &node, &subtree_ids(old))?;
    let in_master = doc.path_of(&id).is_none();
    let before = check.then(|| {
        let mut p = check_subtree(doc, old);
        if in_master {
            p.extend(component_problems(doc));
        }
        p
    });
    if check {
        let mut new = check_subtree(doc, &node);
        new.retain(|p| !before.as_ref().expect("checking").contains(p));
        if !new.is_empty() {
            return Err(CommandError::Invariant(new));
        }
    }
    let new_id = node.id.clone();
    let slot = doc.node_or_master_mut(&id).expect("found above");
    let old = mem::replace(slot, node);
    doc.rebuild_index();
    let inverse = Command::ReplaceNode {
        id: new_id,
        node: old,
    };
    if let (true, Some(before)) = (in_master, before) {
        let new: Vec<Problem> = component_problems(doc)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        if !new.is_empty() {
            inverse
                .apply_unchecked(doc)
                .expect("restoring the previous node always applies");
            return Err(CommandError::Invariant(new));
        }
    }
    Ok(inverse)
}
