//! Structure edits (§5.1, §10 M2): group, ungroup, frame selection, z-order,
//! duplicate, lock and select-all. Each is one `Editor::commit`, so each is
//! one undo step.

use doc::{Command, Frame, Group, Id, IdKind, Node, NodeId, NodeKind, NodePatch, ParentRef};

use crate::editor::Editor;

/// How far a z-order command moves the selection among its siblings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZOrder {
    Forward,
    Backward,
    Front,
    Back,
}

impl ZOrder {
    pub fn label(self) -> &'static str {
        match self {
            ZOrder::Forward => "Bring forward",
            ZOrder::Backward => "Send backward",
            ZOrder::Front => "Bring to front",
            ZOrder::Back => "Send to back",
        }
    }
}

/// How far a duplicate or an over-the-source paste is shifted.
pub const OFFSET: f32 = 10.0;

/// Replace every node ID in the subtree with a fresh one.
pub fn fresh_ids(node: &mut Node) {
    node.id = Id::new(IdKind::Node);
    if let Some(children) = node.children_mut() {
        for c in children {
            fresh_ids(c);
        }
    }
}

/// Axis-aligned bounds `(x0, y0, x1, y1)` of a box in its parent's space.
fn aabb(x: f32, y: f32, w: f32, h: f32, rotation: f32) -> (f32, f32, f32, f32) {
    let (s, c) = rotation.to_radians().sin_cos();
    let hx = (w * c.abs() + h * s.abs()) / 2.0;
    let hy = (w * s.abs() + h * c.abs()) / 2.0;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    (cx - hx, cy - hy, cx + hx, cy + hy)
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

impl Editor {
    /// The top-level selection with its position: parent and index, in
    /// back-to-front order within each parent.
    fn selected_with_parents(&self) -> Vec<(NodeId, ParentRef, usize)> {
        let mut out: Vec<_> = self
            .top_selected()
            .into_iter()
            .filter_map(|id| {
                let (p, i) = self.doc.parent_of(&id)?;
                Some((id, p, i))
            })
            .collect();
        out.sort_by(|a, b| (format!("{:?}", a.1), a.2).cmp(&(format!("{:?}", b.1), b.2)));
        out
    }

    fn notice(&mut self, text: &str) {
        self.status = Some(text.to_string());
    }

    /// Wrap the selection in a new group or frame whose bounds are the
    /// union of the selection. Children keep their absolute positions.
    fn wrap_selection(&mut self, frame: bool) {
        let Some((cmds, gid)) = self.wrap_commands(frame) else {
            return;
        };
        let label = if frame {
            "Frame selection"
        } else {
            "Group layers"
        };
        let _ = self.commit(label, vec![Command::Batch(cmds)], vec![gid]);
    }

    /// The commands that wrap the selection in a new group or frame, and
    /// the wrapper's ID; `None` (with a notice) if it cannot be wrapped.
    pub(crate) fn wrap_commands(&mut self, frame: bool) -> Option<(Vec<Command>, NodeId)> {
        let sel = self.selected_with_parents();
        let what = if frame { "Frame" } else { "Group" };
        let (_, parent, _) = sel.first().cloned()?;
        if sel.iter().any(|(_, p, _)| *p != parent) {
            self.notice(&format!("Select layers with the same parent to {what}"));
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (id, _, _) in &sel {
            let Some(n) = self.node(id) else { continue };
            let (w, h) = self.geometry.get(id).map_or((n.width, n.height), |g| {
                (g.size.width as f32, g.size.height as f32)
            });
            let (a, b, c, d) = aabb(n.x, n.y, w, h, n.rotation);
            x0 = x0.min(a);
            y0 = y0.min(b);
            x1 = x1.max(c);
            y1 = y1.max(d);
        }
        let (gx, gy) = (round2(x0), round2(y0));
        let mut children = Vec::new();
        let mut cmds = Vec::new();
        for (id, _, _) in &sel {
            let Some(mut n) = self.node(id).cloned() else {
                continue;
            };
            n.x -= gx;
            n.y -= gy;
            children.push(n);
            cmds.push(Command::DeleteNode { id: id.clone() });
        }
        let index = sel.iter().map(|(_, _, i)| *i).min().unwrap_or(0);
        let gid = Id::new(IdKind::Node);
        let name = self.next_name(what);
        let kind = if frame {
            NodeKind::Frame(Frame {
                children,
                ..Default::default()
            })
        } else {
            NodeKind::Group(Group {
                children,
                ..Default::default()
            })
        };
        let wrapper = Node::new(gid.clone(), name, kind).with_geometry(
            gx,
            gy,
            round2(x1 - x0).max(0.0),
            round2(y1 - y0).max(0.0),
        );
        cmds.push(Command::CreateNode {
            parent,
            index: Some(index),
            node: wrapper,
        });
        Some((cmds, gid))
    }

    /// Ctrl+G: group the selection.
    pub fn group_selection(&mut self) {
        self.wrap_selection(false);
    }

    /// Ctrl+Alt+G: put the selection in a new frame.
    pub fn frame_selection(&mut self) {
        self.wrap_selection(true);
    }

    /// The size a node rotates about the center of: computed for text and
    /// auto-layout frames, stored for groups (whose geometry is the union
    /// of their children, but which turn as their own box).
    fn size_of(&self, n: &Node) -> (f32, f32) {
        if matches!(n.kind, NodeKind::Group(_)) {
            return (n.width, n.height);
        }
        self.geometry.get(&n.id).map_or((n.width, n.height), |g| {
            (g.size.width as f32, g.size.height as f32)
        })
    }

    /// Ctrl+Shift+G: replace each selected group by its children, which
    /// keep their absolute positions.
    pub fn ungroup_selection(&mut self) {
        let mut groups: Vec<_> = self
            .selected_with_parents()
            .into_iter()
            .filter(|(id, _, _)| matches!(self.node(id).map(|n| &n.kind), Some(NodeKind::Group(_))))
            .collect();
        if groups.is_empty() {
            self.notice("Select a group to ungroup");
            return;
        }
        // Back to front, so earlier indices stay valid.
        groups.reverse();
        let mut cmds = Vec::new();
        let mut selection = Vec::new();
        for (id, parent, index) in groups {
            let Some(g) = self.node(&id).cloned() else {
                continue;
            };
            cmds.push(Command::DeleteNode { id });
            let children = g.children().cloned().unwrap_or_default();
            let (gw, gh) = self.size_of(&g);
            let (s, co) = g.rotation.to_radians().sin_cos();
            for (k, mut c) in children.into_iter().enumerate() {
                if g.rotation == 0.0 {
                    c.x = round2(c.x + g.x);
                    c.y = round2(c.y + g.y);
                } else {
                    // The group turns about its center, then the child about
                    // its own: the child's center lands at
                    // `R(c + cc - gc) + gc + g` and the rotations add up.
                    let (cw, ch) = self.size_of(&c);
                    let (vx, vy) = (c.x + cw / 2.0 - gw / 2.0, c.y + ch / 2.0 - gh / 2.0);
                    let (rx, ry) = (vx * co - vy * s, vx * s + vy * co);
                    c.x = round2(rx + gw / 2.0 + g.x - cw / 2.0);
                    c.y = round2(ry + gh / 2.0 + g.y - ch / 2.0);
                    c.rotation =
                        crate::canvas::normalize_degrees((c.rotation + g.rotation) as f64) as f32;
                }
                selection.push(c.id.clone());
                cmds.push(Command::CreateNode {
                    parent: parent.clone(),
                    index: Some(index + k),
                    node: c,
                });
            }
        }
        let _ = self.commit("Ungroup", vec![Command::Batch(cmds)], selection);
    }

    /// Move the selection in z-order among its siblings.
    pub fn reorder(&mut self, op: ZOrder) {
        let mut parents: Vec<ParentRef> = Vec::new();
        for (_, p, _) in self.selected_with_parents() {
            if !parents.contains(&p) {
                parents.push(p);
            }
        }
        let mut cmds = Vec::new();
        for parent in parents {
            let Some(children) = self.doc.children_of(&parent) else {
                continue;
            };
            let ids: Vec<NodeId> = children.iter().map(|c| c.id.clone()).collect();
            let sel: Vec<bool> = ids.iter().map(|i| self.selection.contains(i)).collect();
            let mut order: Vec<(NodeId, bool)> = ids.iter().cloned().zip(sel).collect();
            match op {
                ZOrder::Front => order.sort_by_key(|(_, s)| *s),
                ZOrder::Back => order.sort_by_key(|(_, s)| !*s),
                ZOrder::Forward => {
                    for i in (0..order.len().saturating_sub(1)).rev() {
                        if order[i].1 && !order[i + 1].1 {
                            order.swap(i, i + 1);
                        }
                    }
                }
                ZOrder::Backward => {
                    for i in 1..order.len() {
                        if order[i].1 && !order[i - 1].1 {
                            order.swap(i, i - 1);
                        }
                    }
                }
            }
            // Turn the target order into moves, simulating each on a copy.
            let mut cur = ids;
            for (i, (id, _)) in order.iter().enumerate() {
                if &cur[i] != id {
                    let j = cur.iter().position(|c| c == id).expect("same ids");
                    let moved = cur.remove(j);
                    cur.insert(i, moved);
                    cmds.push(Command::MoveNode {
                        id: id.clone(),
                        parent: parent.clone(),
                        index: Some(i),
                    });
                }
            }
        }
        if !cmds.is_empty() {
            let sel = self.selection.clone();
            let _ = self.commit(op.label(), vec![Command::Batch(cmds)], sel);
        }
    }

    /// Ctrl+D: copy the selection right above the originals, offset a
    /// little, and select the copies.
    pub fn duplicate_selection(&mut self) {
        let mut items = self.selected_with_parents();
        // Front to back, so inserting above one does not shift the next.
        items.reverse();
        let mut cmds = Vec::new();
        let mut copies = Vec::new();
        for (id, parent, index) in items {
            let Some(mut n) = self.node(&id).cloned() else {
                continue;
            };
            fresh_ids(&mut n);
            n.x += OFFSET;
            n.y += OFFSET;
            copies.push(n.id.clone());
            cmds.push(Command::CreateNode {
                parent,
                index: Some(index + 1),
                node: n,
            });
        }
        if cmds.is_empty() {
            return;
        }
        copies.reverse();
        let label = if copies.len() == 1 {
            "Duplicate layer".to_string()
        } else {
            format!("Duplicate {} layers", copies.len())
        };
        let _ = self.commit(&label, vec![Command::Batch(cmds)], copies);
    }

    pub fn set_locked(&mut self, id: &NodeId, locked: bool) {
        let patch = NodePatch {
            locked: Some(locked),
            ..Default::default()
        };
        let sel = self.selection.clone();
        let label = if locked { "Lock layer" } else { "Unlock layer" };
        let _ = self.commit(
            label,
            vec![Command::SetProps {
                id: id.clone(),
                patch,
            }],
            sel,
        );
    }

    /// Select every unlocked top-level layer of the current page.
    pub fn select_all(&mut self) {
        self.selection = self
            .page()
            .children
            .iter()
            .filter(|n| !n.locked)
            .map(|n| n.id.clone())
            .collect();
    }
}
