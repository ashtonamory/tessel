//! Auto-layout and constraints edits (§3.5): adding and removing
//! auto-layout, Shift+A, and the child moves that constraints cause when a
//! frame resizes. Every change is a doc command, one undo step per action.

use doc::{
    AutoLayout, ChildPosition, Command, Direction, Extra, Node, NodeId, NodeKind, NodePatch,
    ParentRef, Selection, Sizing, SpaceRef,
};
use kurbo::{Point, Rect, Size};

use crate::editor::Editor;

fn round2(v: f64) -> f32 {
    ((v * 100.0).round() / 100.0) as f32
}

/// The frame's auto-layout, if it is a frame with one.
pub fn auto_layout(n: &Node) -> Option<&AutoLayout> {
    match &n.kind {
        NodeKind::Frame(f) => f.layout.as_ref(),
        _ => None,
    }
}

/// Whether `n` is placed by an auto-layout parent `parent`.
pub fn is_flow_child(parent: &Node, n: &Node) -> bool {
    auto_layout(parent).is_some() && n.visible && n.layout_child.position == ChildPosition::Flow
}

/// A new auto-layout with everything at its default.
pub fn new_layout(direction: Direction, width: Sizing, height: Sizing) -> AutoLayout {
    AutoLayout {
        direction,
        wrap: false,
        gap: SpaceRef::Px(0.0),
        cross_gap: None,
        padding: Default::default(),
        justify: Default::default(),
        align: Default::default(),
        width,
        height,
        extra: Extra::new(),
    }
}

/// The auto-layout that keeps `frame`'s children roughly where they are:
/// the direction they spread in, the mean gap between them and the space
/// around them as padding. Also the child order along that direction (IDs
/// in their new order).
pub fn infer_layout(frame: &Node) -> (AutoLayout, Vec<NodeId>) {
    let children: Vec<&Node> = frame
        .children()
        .map(|c| c.iter().filter(|c| c.visible).collect())
        .unwrap_or_default();
    let mut al = new_layout(Direction::Row, Sizing::Fixed, Sizing::Fixed);
    if children.is_empty() {
        return (al, Vec::new());
    }
    let rect = |n: &Node| {
        Rect::new(
            n.x as f64,
            n.y as f64,
            (n.x + n.width) as f64,
            (n.y + n.height) as f64,
        )
    };
    let centers: Vec<Point> = children.iter().map(|n| rect(n).center()).collect();
    let spread = |f: fn(&Point) -> f64| {
        let (lo, hi) = centers
            .iter()
            .map(f)
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        hi - lo
    };
    if spread(|p| p.y) > spread(|p| p.x) {
        al.direction = Direction::Column;
    }
    let row = al.direction == Direction::Row;
    let mut sorted = children.clone();
    let key = |n: &&Node| if row { n.x } else { n.y };
    sorted.sort_by(|a, b| key(a).total_cmp(&key(b)));
    let rects: Vec<Rect> = sorted.iter().map(|n| rect(n)).collect();
    if rects.len() > 1 {
        let gaps: f64 = rects
            .windows(2)
            .map(|w| {
                if row {
                    w[1].x0 - w[0].x1
                } else {
                    w[1].y0 - w[0].y1
                }
            })
            .sum();
        al.gap = SpaceRef::Px((gaps / (rects.len() - 1) as f64).max(0.0).round() as f32);
    }
    let all = rects.iter().skip(1).fold(rects[0], |a, r| a.union(*r));
    let pad = |v: f64| SpaceRef::Px(v.max(0.0).round() as f32);
    al.padding = [
        pad(all.y0),
        pad(frame.width as f64 - all.x1),
        pad(frame.height as f64 - all.y1),
        pad(all.x0),
    ];
    (al, sorted.iter().map(|n| n.id.clone()).collect())
}

fn set_layout(id: &NodeId, layout: Option<AutoLayout>) -> Command {
    Command::SetProps {
        id: id.clone(),
        patch: NodePatch {
            layout: Some(layout),
            ..Default::default()
        },
    }
}

impl Editor {
    /// Commands that give frame `id` auto-layout inferred from its children,
    /// ordering the children along it.
    fn add_layout_commands(&self, id: &NodeId, width: Sizing, height: Sizing) -> Vec<Command> {
        let Some(frame) = self.node(id) else {
            return Vec::new();
        };
        let (mut al, order) = infer_layout(frame);
        al.width = width;
        al.height = height;
        let mut cmds = vec![set_layout(id, Some(al))];
        // Hidden children keep their places; the visible ones take the
        // positions of the visible ones in order.
        let children = frame.children().cloned().unwrap_or_default();
        let slots: Vec<usize> = (0..children.len())
            .filter(|i| children[*i].visible)
            .collect();
        let mut current: Vec<NodeId> = children.iter().map(|c| c.id.clone()).collect();
        for (slot, want) in slots.iter().zip(&order) {
            if current[*slot] != *want {
                let from = current.iter().position(|c| c == want).expect("a child");
                let moved = current.remove(from);
                current.insert(*slot, moved);
                cmds.push(Command::MoveNode {
                    id: want.clone(),
                    parent: ParentRef::Node(id.clone()),
                    index: Some(*slot),
                });
            }
        }
        cmds
    }

    /// Commands that remove frame `id`'s auto-layout, leaving every child
    /// where layout put it.
    fn remove_layout_commands(&self, id: &NodeId) -> Vec<Command> {
        let (Some(frame), Some(fg)) = (self.node(id), self.geometry.get(id)) else {
            return Vec::new();
        };
        let mut cmds = vec![set_layout(id, None)];
        for c in frame.children().into_iter().flatten() {
            let Some(g) = self.geometry.get(&c.id) else {
                continue;
            };
            let center =
                fg.to_local(g.transform * Point::new(g.size.width / 2.0, g.size.height / 2.0));
            cmds.push(Command::SetGeometry {
                id: c.id.clone(),
                x: round2(center.x - g.size.width / 2.0),
                y: round2(center.y - g.size.height / 2.0),
                width: round2(g.size.width),
                height: round2(g.size.height),
                rotation: c.rotation,
            });
        }
        cmds
    }

    /// Add auto-layout to the selected frames that have none. Returns
    /// whether anything changed.
    pub fn add_auto_layout(&mut self) -> bool {
        let frames: Vec<NodeId> = self
            .top_selected()
            .into_iter()
            .filter(|id| {
                self.node(id)
                    .is_some_and(|n| matches!(&n.kind, NodeKind::Frame(f) if f.layout.is_none()))
            })
            .collect();
        let cmds: Vec<Command> = frames
            .iter()
            .flat_map(|id| self.add_layout_commands(id, Sizing::Fixed, Sizing::Fixed))
            .collect();
        if cmds.is_empty() {
            return false;
        }
        let sel = self.selection.clone();
        self.commit("Add auto layout", vec![Command::Batch(cmds)], sel)
            .is_ok()
    }

    /// Remove auto-layout from the selected frames that have it.
    pub fn remove_auto_layout(&mut self) -> bool {
        let cmds: Vec<Command> = self
            .top_selected()
            .iter()
            .filter(|id| self.node(id).and_then(auto_layout).is_some())
            .flat_map(|id| self.remove_layout_commands(id))
            .collect();
        if cmds.is_empty() {
            return false;
        }
        let sel = self.selection.clone();
        self.commit("Remove auto layout", vec![Command::Batch(cmds)], sel)
            .is_ok()
    }

    /// Shift+A: on frames, add auto-layout (or remove it when they all have
    /// it); on other layers, wrap them in a new auto-layout frame that hugs
    /// them. One undo step either way.
    pub fn toggle_auto_layout(&mut self) {
        let sel = self.top_selected();
        if sel.is_empty() {
            return;
        }
        let frames = sel
            .iter()
            .all(|id| matches!(self.node(id).map(|n| &n.kind), Some(NodeKind::Frame(_))));
        if frames {
            if !self.add_auto_layout() {
                self.remove_auto_layout();
            }
            return;
        }
        let depth = self.history.transactions().len();
        self.frame_selection();
        if self.history.transactions().len() == depth {
            return;
        }
        let Some(frame) = self.selection.first().cloned() else {
            return;
        };
        let cmds = self.add_layout_commands(&frame, Sizing::Hug, Sizing::Hug);
        self.amend_last(cmds);
        if let Some(tx) = self.history.last_mut() {
            tx.label = "Add auto layout".into();
        }
    }

    /// Add `cmds` to the last transaction (one undo step), storing computed
    /// sizes as `commit` does.
    pub(crate) fn amend_last(&mut self, cmds: Vec<Command>) {
        if cmds.is_empty() {
            return;
        }
        if self.master.is_some() {
            self.master_amend(cmds);
            return;
        }
        let after = Selection {
            ids: self.selection.clone(),
        };
        let touched = cmds.iter().flat_map(|c| c.touched_ids()).collect();
        if let Err(e) = self.history.amend_last(&mut self.doc, cmds, after.clone()) {
            self.status = Some(e.to_string());
            return;
        }
        self.changed(touched);
        let writes = self.size_writes();
        if !writes.is_empty() {
            let touched = writes.iter().flat_map(|c| c.touched_ids()).collect();
            if self
                .history
                .amend_last(&mut self.doc, writes, after)
                .is_ok()
            {
                self.changed(touched);
            }
        }
    }

    /// The single selected node, when it is an auto-layout frame.
    pub fn selected_auto_layout(&self) -> Option<&Node> {
        match self.selection.as_slice() {
            [id] => self.node(id).filter(|n| auto_layout(n).is_some()),
            _ => None,
        }
    }
}

/// Where the children of `frame` (as it was before the gesture) go when
/// it resizes from `old` to `new` (§3.5): constraints for the children of
/// a frame without auto-layout and for the absolute children of one with
/// it. Frames among those children that change size pass it on to theirs.
/// One `SetGeometry` per affected child, whether or not it moved, so a
/// gesture that comes back undoes the move.
pub fn constrained_children(frame: &Node, old: Size, new: Size, out: &mut Vec<Command>) {
    let NodeKind::Frame(f) = &frame.kind else {
        return;
    };
    for c in &f.children {
        if f.layout.is_some() && c.layout_child.position == ChildPosition::Flow {
            continue;
        }
        let before =
            Rect::from_origin_size((c.x as f64, c.y as f64), (c.width as f64, c.height as f64));
        let r = layout::apply_constraints(old, new, before, &c.constraints);
        let (w, h) = (round2(r.width()), round2(r.height()));
        out.push(Command::SetGeometry {
            id: c.id.clone(),
            x: round2(r.x0),
            y: round2(r.y0),
            width: w,
            height: h,
            rotation: c.rotation,
        });
        if (w, h) != (c.width, c.height) {
            constrained_children(
                c,
                Size::new(c.width as f64, c.height as f64),
                Size::new(w as f64, h as f64),
                out,
            );
        }
    }
}

/// What else changes when a resize gesture sets the sizes in `cmds` on the
/// nodes in `snapshots` (their state when the gesture began): constraints
/// move the children of resized frames, and an axis that hugged or filled
/// becomes fixed so the new size holds.
pub fn resize_effects(ed: &Editor, snapshots: &[Node], cmds: &[Command]) -> Vec<Command> {
    let mut out = Vec::new();
    for cmd in cmds {
        let Command::SetGeometry {
            id, width, height, ..
        } = cmd
        else {
            continue;
        };
        let Some(s) = snapshots.iter().find(|s| s.id == *id) else {
            continue;
        };
        let (dw, dh) = (*width != s.width, *height != s.height);
        constrained_children(
            s,
            Size::new(s.width as f64, s.height as f64),
            Size::new(*width as f64, *height as f64),
            &mut out,
        );
        let Some(now) = ed.node(id) else { continue };
        let fix = |v: &mut Sizing, changed: bool| {
            if changed && *v != Sizing::Fixed {
                *v = Sizing::Fixed;
            }
        };
        if let Some(al) = auto_layout(now) {
            let mut al = al.clone();
            fix(&mut al.width, dw);
            fix(&mut al.height, dh);
            if Some(&al) != auto_layout(now) {
                out.push(set_layout(id, Some(al)));
            }
        }
        let in_flow = match ed.doc.parent_of(id) {
            Some((ParentRef::Node(p), _)) => ed.node(&p).is_some_and(|p| is_flow_child(p, now)),
            _ => false,
        };
        if in_flow {
            let mut lc = now.layout_child.clone();
            fix(&mut lc.width, dw);
            fix(&mut lc.height, dh);
            if lc != now.layout_child {
                out.push(Command::SetProps {
                    id: id.clone(),
                    patch: NodePatch {
                        layout_child: Some(lc),
                        ..Default::default()
                    },
                });
            }
        }
    }
    out
}
