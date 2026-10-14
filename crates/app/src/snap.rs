//! Snapping while moving and resizing (§9): the edges and centres of the
//! moving box snap to those of its siblings and its parent, otherwise to the
//! whole-pixel grid. The engine is a pure function of page rectangles; the
//! threshold is in screen pixels, so it is divided by the zoom.

use doc::NodeId;
use kurbo::{Rect, Vec2};

use crate::editor::Editor;

/// How close (in screen pixels) a feature must be to snap.
pub const THRESHOLD_PX: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// A vertical line at some x.
    X,
    /// A horizontal line at some y.
    Y,
}

/// Which features of the moving box take part on one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Edges {
    pub min: bool,
    pub center: bool,
    pub max: bool,
}

impl Edges {
    pub const ALL: Edges = Edges {
        min: true,
        center: true,
        max: true,
    };
    pub const NONE: Edges = Edges {
        min: false,
        center: false,
        max: false,
    };
    pub const MIN: Edges = Edges {
        min: true,
        center: false,
        max: false,
    };
    pub const MAX: Edges = Edges {
        min: false,
        center: false,
        max: true,
    };

    fn any(self) -> bool {
        self.min || self.center || self.max
    }

    /// The selected features of the span `lo..hi`.
    fn positions(self, lo: f64, hi: f64) -> impl Iterator<Item = f64> {
        [
            self.min.then_some(lo),
            self.center.then_some((lo + hi) / 2.0),
            self.max.then_some(hi),
        ]
        .into_iter()
        .flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapOptions {
    pub zoom: f64,
    pub threshold_px: f64,
    /// False while Ctrl is held: no snapping at all, not even to the grid.
    pub enabled: bool,
}

impl SnapOptions {
    pub fn new(zoom: f64, enabled: bool) -> SnapOptions {
        SnapOptions {
            zoom,
            threshold_px: THRESHOLD_PX,
            enabled,
        }
    }
}

/// An alignment line that snapped, in page coordinates: at `pos` on `axis`,
/// spanning `from..to` along the other axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub axis: Axis,
    pub pos: f64,
    pub from: f64,
    pub to: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapped {
    /// Add this to the moving box (or the dragged edges).
    pub delta: Vec2,
    pub guides: Vec<Guide>,
}

/// Snap `moving` against `targets` (sibling bounds and the parent's bounds).
/// `xs`/`ys` say which features of `moving` may snap on each axis: all of
/// them when moving, only the dragged edges when resizing.
pub fn snap(moving: Rect, xs: Edges, ys: Edges, targets: &[Rect], opts: SnapOptions) -> Snapped {
    let mut out = Snapped::default();
    if !opts.enabled {
        return out;
    }
    let threshold = opts.threshold_px / opts.zoom.max(1e-9);
    out.delta.x = snap_axis(moving, Axis::X, xs, targets, threshold);
    out.delta.y = snap_axis(moving, Axis::Y, ys, targets, threshold);
    out.guides = guides(moving + out.delta, xs, ys, targets);
    out
}

fn span(r: Rect, axis: Axis) -> (f64, f64) {
    match axis {
        Axis::X => (r.x0, r.x1),
        Axis::Y => (r.y0, r.y1),
    }
}

fn snap_axis(moving: Rect, axis: Axis, edges: Edges, targets: &[Rect], threshold: f64) -> f64 {
    if !edges.any() {
        return 0.0;
    }
    let (lo, hi) = span(moving, axis);
    let mut best: Option<f64> = None;
    for t in targets {
        let (tlo, thi) = span(*t, axis);
        for src in edges.positions(lo, hi) {
            for dst in Edges::ALL.positions(tlo, thi) {
                let d = dst - src;
                if d.abs() <= threshold && best.is_none_or(|b| d.abs() < b.abs()) {
                    best = Some(d);
                }
            }
        }
    }
    best.unwrap_or_else(|| {
        // The grid: the first snapping feature lands on a whole pixel.
        let src = edges.positions(lo, hi).next().unwrap_or(lo);
        src.round() - src
    })
}

/// The lines along which `snapped` now lines up with a target.
fn guides(snapped: Rect, xs: Edges, ys: Edges, targets: &[Rect]) -> Vec<Guide> {
    const EPS: f64 = 1e-6;
    let mut out: Vec<Guide> = Vec::new();
    for (axis, edges) in [(Axis::X, xs), (Axis::Y, ys)] {
        let (lo, hi) = span(snapped, axis);
        let other = match axis {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        };
        let (olo, ohi) = span(snapped, other);
        for src in edges.positions(lo, hi) {
            for t in targets {
                let (tlo, thi) = span(*t, axis);
                if Edges::ALL
                    .positions(tlo, thi)
                    .any(|dst| (dst - src).abs() < EPS)
                {
                    let (tolo, tohi) = span(*t, other);
                    let (from, to) = (olo.min(tolo), ohi.max(tohi));
                    match out
                        .iter_mut()
                        .find(|g| g.axis == axis && (g.pos - src).abs() < EPS)
                    {
                        Some(g) => {
                            g.from = g.from.min(from);
                            g.to = g.to.max(to);
                        }
                        None => out.push(Guide {
                            axis,
                            pos: src,
                            from,
                            to,
                        }),
                    }
                }
            }
        }
    }
    out
}

/// What the nodes in `moving` can snap to: the bounds of their visible
/// siblings (not themselves moving) and of their parent node, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    pub siblings: Vec<Rect>,
    pub parent: Option<Rect>,
}

impl Targets {
    pub fn collect(ed: &Editor, moving: &[NodeId]) -> Targets {
        let Some(first) = moving.first() else {
            return Targets::default();
        };
        let parent_id = ed.geometry.get(first).and_then(|g| g.parent.clone());
        let siblings = match &parent_id {
            Some(p) => ed
                .node(p)
                .and_then(|n| n.children())
                .map_or(&[][..], |v| &v[..]),
            None => &ed.page().children[..],
        };
        Targets {
            siblings: siblings
                .iter()
                .filter(|n| n.visible && !moving.contains(&n.id))
                .filter_map(|n| ed.geometry.get(&n.id).map(|g| g.bounds))
                .collect(),
            parent: parent_id
                .as_ref()
                .and_then(|p| ed.geometry.get(p))
                .map(|g| g.bounds),
        }
    }

    pub fn all(&self) -> Vec<Rect> {
        let mut v = self.siblings.clone();
        v.extend(self.parent);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(zoom: f64) -> SnapOptions {
        SnapOptions::new(zoom, true)
    }

    #[test]
    fn snaps_edges_to_sibling_edges() {
        let sib = Rect::new(200.0, 0.0, 300.0, 100.0);
        // Right edge 197 → sibling's left edge 200.
        let m = Rect::new(97.0, 300.0, 197.0, 400.0);
        let s = snap(m, Edges::ALL, Edges::ALL, &[sib], on(1.0));
        assert_eq!(s.delta.x, 3.0);
        let g = s.guides.iter().find(|g| g.axis == Axis::X).unwrap();
        assert_eq!(g.pos, 200.0);
        assert_eq!((g.from, g.to), (0.0, 400.0));
        // Left edge 296 → sibling's right edge 300.
        let m = Rect::new(296.0, 300.0, 396.0, 400.0);
        let s = snap(m, Edges::ALL, Edges::ALL, &[sib], on(1.0));
        assert_eq!(s.delta.x, 4.0);
    }

    #[test]
    fn snaps_centres_and_to_the_parent() {
        let parent = Rect::new(0.0, 0.0, 400.0, 300.0);
        // Centre 198 → parent centre 200; centre y 147 → 150.
        let m = Rect::new(148.0, 122.0, 248.0, 172.0);
        let s = snap(m, Edges::ALL, Edges::ALL, &[parent], on(1.0));
        assert_eq!(s.delta, Vec2::new(2.0, 3.0));
        assert_eq!(s.guides.len(), 2);
        // Centre to a sibling's edge.
        let sib = Rect::new(500.0, 0.0, 600.0, 50.0);
        let m = Rect::new(450.0, 200.0, 554.0, 260.0);
        let s = snap(m, Edges::ALL, Edges::NONE, &[sib], on(1.0));
        assert_eq!(s.delta.x, -2.0);
        assert_eq!(s.delta.y, 0.0);
    }

    #[test]
    fn picks_the_nearest_candidate() {
        let a = Rect::new(100.0, 0.0, 110.0, 10.0);
        let b = Rect::new(104.0, 0.0, 120.0, 10.0);
        let m = Rect::new(103.0, 50.0, 153.0, 60.0);
        let s = snap(m, Edges::ALL, Edges::NONE, &[a, b], on(1.0));
        assert_eq!(s.delta.x, 1.0);
    }

    #[test]
    fn falls_back_to_the_pixel_grid() {
        let far = Rect::new(1000.0, 1000.0, 1100.0, 1100.0);
        let m = Rect::new(10.4, 20.6, 50.4, 60.6);
        let s = snap(m, Edges::ALL, Edges::ALL, &[far], on(1.0));
        assert!((s.delta.x + 0.4).abs() < 1e-9);
        assert!((s.delta.y - 0.4).abs() < 1e-9);
        assert!(s.guides.is_empty());
        // Resizing the right edge rounds that edge.
        let s = snap(m, Edges::MAX, Edges::NONE, &[], on(1.0));
        assert!((s.delta.x + 0.4).abs() < 1e-9);
        assert_eq!(s.delta.y, 0.0);
    }

    #[test]
    fn threshold_is_in_screen_pixels() {
        let sib = Rect::new(200.0, 0.0, 300.0, 100.0);
        let m = Rect::new(90.0, 300.0, 190.0, 400.0);
        // 10 page units is 10 px at 100%: too far.
        let s = snap(m, Edges::ALL, Edges::NONE, &[sib], on(1.0));
        assert_eq!(s.delta.x, 0.0);
        // At 50% it is 5 px: snaps.
        let s = snap(m, Edges::ALL, Edges::NONE, &[sib], on(0.5));
        assert_eq!(s.delta.x, 10.0);
        // At 400%, 2 page units is 8 px: too far.
        let m = Rect::new(98.0, 300.0, 198.0, 400.0);
        let s = snap(m, Edges::ALL, Edges::NONE, &[sib], on(4.0));
        assert_eq!(s.delta.x, 0.0);
        let s = snap(m, Edges::ALL, Edges::NONE, &[sib], on(2.0));
        assert_eq!(s.delta.x, 2.0);
    }

    #[test]
    fn disabled_does_nothing() {
        let sib = Rect::new(200.0, 0.0, 300.0, 100.0);
        let m = Rect::new(97.5, 0.5, 197.5, 100.5);
        let s = snap(
            m,
            Edges::ALL,
            Edges::ALL,
            &[sib],
            SnapOptions::new(1.0, false),
        );
        assert_eq!(s, Snapped::default());
    }

    #[test]
    fn resize_only_snaps_the_dragged_edges() {
        let sib = Rect::new(200.0, 0.0, 300.0, 100.0);
        // The left edge is 2 from the sibling's right edge but is not dragged.
        let m = Rect::new(302.0, 0.0, 397.0, 50.0);
        let s = snap(m, Edges::MAX, Edges::NONE, &[sib], on(1.0));
        assert_eq!(s.delta.x, 0.0);
        let s = snap(m, Edges::MIN, Edges::NONE, &[sib], on(1.0));
        assert_eq!(s.delta.x, -2.0);
    }
}
