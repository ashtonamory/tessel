//! Auto-layout on the canvas (§3.5, §9): dragging a flow child reorders it
//! among its siblings (Alt-drag takes it out of the frame instead), the
//! padding and gap of an auto-layout frame show as overlays on hover, and
//! their handles drag to change them. Each drag is one undo step.

use doc::{
    AutoLayout, Command, CommandError, Direction, Node, NodeId, NodePatch, ParentRef, SpaceRef,
};
use egui::{Color32, CornerRadius, CursorIcon, Painter, Pos2, Rect, Stroke, Vec2};
use kurbo::{Affine, Point};

use crate::auto_layout::{auto_layout, is_flow_child};
use crate::canvas::{View, SELECTION};
use crate::editor::Editor;

/// Padding and gap overlays.
const SPACING: Color32 = Color32::from_rgb(0xf2, 0x4e, 0x9e);
/// Handle hit radius, screen points.
const GRAB: f32 = 6.0;

/// What a spacing handle changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    /// Padding side, `[top, right, bottom, left]` order.
    Padding(usize),
    Gap,
}

impl Spacing {
    pub fn cursor(self) -> CursorIcon {
        match self {
            Spacing::Padding(0 | 2) => CursorIcon::ResizeVertical,
            Spacing::Padding(_) => CursorIcon::ResizeHorizontal,
            Spacing::Gap => CursorIcon::ResizeColumn,
        }
    }
}

/// An area of padding or gap, in the frame's local coordinates.
pub struct Area {
    pub spacing: Spacing,
    pub rect: kurbo::Rect,
}

fn px(ed: &Editor, s: &SpaceRef) -> f64 {
    layout::resolve_space(&ed.doc, s) as f64
}

/// The flow children of `frame` and their local boxes, in order.
fn flow_boxes(ed: &Editor, frame: &Node, fg: &layout::NodeGeometry) -> Vec<(NodeId, kurbo::Rect)> {
    frame
        .children()
        .into_iter()
        .flatten()
        .filter(|c| is_flow_child(frame, c))
        .filter_map(|c| {
            let g = ed.geometry.get(&c.id)?;
            let to_local = fg.transform.inverse() * g.transform;
            let r = to_local.transform_rect_bbox(g.local_rect());
            Some((c.id.clone(), r))
        })
        .collect()
}

/// The padding bands and gaps of auto-layout frame `id`.
pub fn areas(ed: &Editor, id: &NodeId) -> Vec<Area> {
    let (Some(frame), Some(fg)) = (ed.node(id), ed.geometry.get(id)) else {
        return Vec::new();
    };
    let Some(al) = auto_layout(frame) else {
        return Vec::new();
    };
    let r = fg.local_rect();
    let [t, rt, b, l] = std::array::from_fn(|i| px(ed, &al.padding[i]));
    let mut out = vec![
        Area {
            spacing: Spacing::Padding(0),
            rect: kurbo::Rect::new(r.x0, r.y0, r.x1, r.y0 + t),
        },
        Area {
            spacing: Spacing::Padding(1),
            rect: kurbo::Rect::new(r.x1 - rt, r.y0, r.x1, r.y1),
        },
        Area {
            spacing: Spacing::Padding(2),
            rect: kurbo::Rect::new(r.x0, r.y1 - b, r.x1, r.y1),
        },
        Area {
            spacing: Spacing::Padding(3),
            rect: kurbo::Rect::new(r.x0, r.y0, r.x0 + l, r.y1),
        },
    ];
    let row = al.direction == Direction::Row;
    let boxes = flow_boxes(ed, frame, fg);
    for w in boxes.windows(2) {
        let (a, b) = (w[0].1, w[1].1);
        // Neighbours on the same line only (wrapping starts a new one).
        let rect = if row && b.x0 >= a.x1 {
            kurbo::Rect::new(a.x1, a.y0.min(b.y0), b.x0, a.y1.max(b.y1))
        } else if !row && b.y0 >= a.y1 {
            kurbo::Rect::new(a.x0.min(b.x0), a.y1, a.x1.max(b.x1), b.y0)
        } else {
            continue;
        };
        out.push(Area {
            spacing: Spacing::Gap,
            rect,
        });
    }
    out
}

/// Where the handles of frame `id` are, in page coordinates: the middle of
/// each padding band (at least a little inside the frame, so a zero
/// padding still has a handle) and of each gap.
pub fn handles(ed: &Editor, id: &NodeId, zoom: f64) -> Vec<(Spacing, Point)> {
    let Some(fg) = ed.geometry.get(id) else {
        return Vec::new();
    };
    let r = fg.local_rect();
    let inset = 10.0 / zoom;
    areas(ed, id)
        .into_iter()
        .map(|a| {
            let c = a.rect.center();
            let p = match a.spacing {
                Spacing::Padding(0) => Point::new(c.x, r.y0 + (a.rect.height() / 2.0).max(inset)),
                Spacing::Padding(1) => Point::new(r.x1 - (a.rect.width() / 2.0).max(inset), c.y),
                Spacing::Padding(2) => Point::new(c.x, r.y1 - (a.rect.height() / 2.0).max(inset)),
                Spacing::Padding(_) => Point::new(r.x0 + (a.rect.width() / 2.0).max(inset), c.y),
                Spacing::Gap => c,
            };
            (a.spacing, fg.transform * p)
        })
        .collect()
}

/// The handle of the selected auto-layout frame under screen point `p`.
pub fn handle_at(ed: &Editor, view: &View, origin: Pos2, p: Pos2) -> Option<(NodeId, Spacing)> {
    if ed.read_only {
        return None;
    }
    let frame = ed.selected_auto_layout()?;
    if frame.locked {
        return None;
    }
    handles(ed, &frame.id, view.zoom as f64)
        .into_iter()
        .find(|(_, h)| (view.to_screen(origin, *h) - p).length() <= GRAB)
        .map(|(s, _)| (frame.id.clone(), s))
}

/// Dragging a padding or gap handle: one gesture.
pub struct SpacingDrag {
    frame: NodeId,
    spacing: Spacing,
    /// The press, in the frame's local coordinates.
    start: Point,
    to_local: Affine,
    base: AutoLayout,
    /// The value when the drag started, px.
    value: f64,
}

impl SpacingDrag {
    /// Begin the gesture; `None` if the document cannot be edited.
    pub fn start(ed: &mut Editor, frame: NodeId, spacing: Spacing, press: Point) -> Option<Self> {
        let base = auto_layout(ed.node(&frame)?)?.clone();
        let to_local = ed.geometry.get(&frame)?.transform.inverse();
        let value = match spacing {
            Spacing::Padding(i) => px(ed, &base.padding[i]),
            Spacing::Gap => px(ed, &base.gap),
        };
        let label = match spacing {
            Spacing::Padding(_) => "Change padding",
            Spacing::Gap => "Change gap",
        };
        ed.begin_gesture(label).ok()?;
        Some(SpacingDrag {
            frame,
            spacing,
            start: to_local * press,
            to_local,
            base,
            value,
        })
    }

    /// The command for the pointer at page point `page`.
    pub fn update(&self, page: Point, round: bool) -> Command {
        let d = self.to_local * page - self.start;
        let delta = match self.spacing {
            Spacing::Padding(0) => d.y,
            Spacing::Padding(1) => -d.x,
            Spacing::Padding(2) => -d.y,
            Spacing::Padding(_) => d.x,
            Spacing::Gap => match self.base.direction {
                Direction::Row => d.x,
                Direction::Column => d.y,
            },
        };
        let v = (self.value + delta).max(0.0);
        let v = if round {
            v.round()
        } else {
            (v * 100.0).round() / 100.0
        };
        let mut al = self.base.clone();
        match self.spacing {
            Spacing::Padding(i) => al.padding[i] = SpaceRef::Px(v as f32),
            Spacing::Gap => al.gap = SpaceRef::Px(v as f32),
        }
        Command::SetProps {
            id: self.frame.clone(),
            patch: NodePatch {
                layout: Some(Some(al)),
                ..Default::default()
            },
        }
    }
}

/// Dragging a flow child to a new place among its siblings. Nothing
/// changes until the drop, which is one `MoveNode`.
pub struct Reorder {
    pub id: NodeId,
    parent: NodeId,
    /// Where it goes in the final child list, once known.
    index: Option<usize>,
    /// The insertion line, page coordinates.
    pub indicator: Option<(Point, Point)>,
}

impl Reorder {
    /// A reorder drag for `id`, if it is a movable flow child of an
    /// auto-layout frame.
    pub fn start(ed: &Editor, id: &NodeId) -> Option<Reorder> {
        let n = ed.node(id)?;
        let (ParentRef::Node(parent), _) = ed.doc.parent_of(id)? else {
            return None;
        };
        let p = ed.node(&parent)?;
        (is_flow_child(p, n) && !n.locked && !ed.read_only).then(|| Reorder {
            id: id.clone(),
            parent,
            index: None,
            indicator: None,
        })
    }

    pub fn update(&mut self, ed: &Editor, page: Point) {
        let (Some(frame), Some(fg)) = (ed.node(&self.parent), ed.geometry.get(&self.parent)) else {
            return;
        };
        let Some(al) = auto_layout(frame) else {
            return;
        };
        let row = al.direction == Direction::Row;
        let p = fg.to_local(page);
        let others: Vec<&Node> = frame
            .children()
            .into_iter()
            .flatten()
            .filter(|c| c.id != self.id)
            .collect();
        let boxes: Vec<(NodeId, kurbo::Rect)> = flow_boxes(ed, frame, fg)
            .into_iter()
            .filter(|(id, _)| *id != self.id)
            .collect();
        let Some((near, r)) = boxes
            .iter()
            .min_by(|a, b| {
                let d = |r: &kurbo::Rect| (r.center() - p).hypot2();
                d(&a.1).total_cmp(&d(&b.1))
            })
            .cloned()
        else {
            self.index = Some(0);
            self.indicator = None;
            return;
        };
        let after = if row {
            p.x > r.center().x
        } else {
            p.y > r.center().y
        };
        let at = others.iter().position(|c| c.id == near).expect("a sibling");
        self.index = Some(if after { at + 1 } else { at });
        let gap = px(ed, &al.gap) / 2.0;
        let (a, b) = match (row, after) {
            (true, false) => (Point::new(r.x0 - gap, r.y0), Point::new(r.x0 - gap, r.y1)),
            (true, true) => (Point::new(r.x1 + gap, r.y0), Point::new(r.x1 + gap, r.y1)),
            (false, false) => (Point::new(r.x0, r.y0 - gap), Point::new(r.x1, r.y0 - gap)),
            (false, true) => (Point::new(r.x0, r.y1 + gap), Point::new(r.x1, r.y1 + gap)),
        };
        self.indicator = Some((fg.transform * a, fg.transform * b));
    }

    /// Drop: move the child to its new place (one undo step).
    pub fn finish(self, ed: &mut Editor) {
        if let Some(index) = self.index {
            ed.move_node(&self.id, ParentRef::Node(self.parent), index);
        }
    }
}

/// Alt-drag on a flow child: take it out of its auto-layout frame into the
/// frame's parent, just in front of the frame, where it is now on the
/// page. Part of the current gesture.
pub fn move_out(ed: &mut Editor, id: &NodeId) -> Result<(), CommandError> {
    let missing = || CommandError::NotFound(id.to_string());
    let n = ed.node(id).ok_or_else(missing)?.clone();
    let Some((ParentRef::Node(parent), _)) = ed.doc.parent_of(id) else {
        return Ok(());
    };
    if !ed.node(&parent).is_some_and(|p| is_flow_child(p, &n)) {
        return Ok(());
    }
    let (to, index) = ed.doc.parent_of(&parent).ok_or_else(missing)?;
    let g = ed.geometry.get(id).ok_or_else(missing)?;
    let size = g.size;
    let center = g.transform * Point::new(size.width / 2.0, size.height / 2.0);
    let local = match &to {
        ParentRef::Node(gp) => ed.geometry.get(gp).ok_or_else(missing)?.to_local(center),
        ParentRef::Page(_) => center,
    };
    let round = |v: f64| ((v * 100.0).round() / 100.0) as f32;
    ed.gesture_apply(Command::MoveNode {
        id: id.clone(),
        parent: to,
        index: Some(index + 1),
    })?;
    ed.gesture_apply(Command::SetGeometry {
        id: id.clone(),
        x: round(local.x - size.width / 2.0),
        y: round(local.y - size.height / 2.0),
        width: round(size.width),
        height: round(size.height),
        rotation: n.rotation,
    })
}

fn screen_quad(view: &View, origin: Pos2, tf: Affine, r: kurbo::Rect) -> Vec<Pos2> {
    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
        .into_iter()
        .map(|(x, y)| view.to_screen(origin, tf * Point::new(x, y)))
        .collect()
}

/// Padding and gap overlays of frame `id` (when the pointer is over it)
/// and, when `with_handles`, its handles.
pub fn paint(
    painter: &Painter,
    view: &View,
    origin: Pos2,
    ed: &Editor,
    id: &NodeId,
    with_handles: bool,
) {
    let Some(fg) = ed.geometry.get(id) else {
        return;
    };
    for a in areas(ed, id) {
        if a.rect.width() <= 0.0 || a.rect.height() <= 0.0 {
            continue;
        }
        let pts = screen_quad(view, origin, fg.transform, a.rect);
        painter.add(egui::Shape::convex_polygon(
            pts,
            SPACING.gamma_multiply(0.18),
            Stroke::NONE,
        ));
    }
    if !with_handles {
        return;
    }
    for (s, p) in handles(ed, id, view.zoom as f64) {
        let c = view.to_screen(origin, p);
        let size = match s {
            Spacing::Padding(0 | 2) => Vec2::new(12.0, 4.0),
            _ => Vec2::new(4.0, 12.0),
        };
        let r = Rect::from_center_size(c, size);
        painter.rect_filled(r, CornerRadius::same(2), SPACING);
        painter.rect_stroke(
            r,
            CornerRadius::same(2),
            Stroke::new(1.0, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    }
}

/// The insertion line of a reorder drag.
pub fn paint_reorder(painter: &Painter, view: &View, origin: Pos2, r: &Reorder) {
    if let Some((a, b)) = r.indicator {
        painter.line_segment(
            [view.to_screen(origin, a), view.to_screen(origin, b)],
            Stroke::new(2.0, SELECTION),
        );
    }
}
