//! Smart guides (§9): the alignment lines that snapped and design-tool-style
//! distance labels from the moving box to its nearest neighbours, or to its
//! parent's edges, painted over the canvas.

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Stroke, Vec2};
use kurbo::{Point, Rect};

use crate::canvas::View;
use crate::snap::{Axis, Guide};

pub const GUIDE: Color32 = Color32::from_rgb(0xf2, 0x48, 0x22);

/// A distance to show: a line from `a` to `b` (page coordinates) labelled
/// with its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    pub a: Point,
    pub b: Point,
    pub value: f64,
}

/// Distances from `m` to the nearest sibling on each side (a sibling is on
/// a side when it overlaps `m` across that direction), or to the parent's
/// inner edge when no sibling is there.
pub fn distances(m: Rect, siblings: &[Rect], parent: Option<Rect>) -> Vec<Measure> {
    let mut out = Vec::new();
    let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| (a0.max(b0), a1.min(b1));
    // Horizontal: left and right.
    for dir in [-1.0, 1.0] {
        let nearest = siblings
            .iter()
            .filter(|s| s.y0 < m.y1 && s.y1 > m.y0)
            .filter_map(|s| {
                let gap = if dir > 0.0 { s.x0 - m.x1 } else { m.x0 - s.x1 };
                (gap > 0.0).then_some((gap, s))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let (x0, x1, y) = match nearest {
            Some((_, s)) => {
                let (lo, hi) = overlap(m.y0, m.y1, s.y0, s.y1);
                let y = (lo + hi) / 2.0;
                if dir > 0.0 {
                    (m.x1, s.x0, y)
                } else {
                    (s.x1, m.x0, y)
                }
            }
            None => match parent {
                Some(p) if p.x0 <= m.x0 && m.x1 <= p.x1 && m.y0 < p.y1 && m.y1 > p.y0 => {
                    let y = m.center().y;
                    if dir > 0.0 {
                        (m.x1, p.x1, y)
                    } else {
                        (p.x0, m.x0, y)
                    }
                }
                _ => continue,
            },
        };
        if x1 - x0 > 1e-6 {
            out.push(Measure {
                a: Point::new(x0, y),
                b: Point::new(x1, y),
                value: x1 - x0,
            });
        }
    }
    // Vertical: up and down.
    for dir in [-1.0, 1.0] {
        let nearest = siblings
            .iter()
            .filter(|s| s.x0 < m.x1 && s.x1 > m.x0)
            .filter_map(|s| {
                let gap = if dir > 0.0 { s.y0 - m.y1 } else { m.y0 - s.y1 };
                (gap > 0.0).then_some((gap, s))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let (y0, y1, x) = match nearest {
            Some((_, s)) => {
                let (lo, hi) = overlap(m.x0, m.x1, s.x0, s.x1);
                let x = (lo + hi) / 2.0;
                if dir > 0.0 {
                    (m.y1, s.y0, x)
                } else {
                    (s.y1, m.y0, x)
                }
            }
            None => match parent {
                Some(p) if p.y0 <= m.y0 && m.y1 <= p.y1 && m.x0 < p.x1 && m.x1 > p.x0 => {
                    let x = m.center().x;
                    if dir > 0.0 {
                        (m.y1, p.y1, x)
                    } else {
                        (p.y0, m.y0, x)
                    }
                }
                _ => continue,
            },
        };
        if y1 - y0 > 1e-6 {
            out.push(Measure {
                a: Point::new(x, y0),
                b: Point::new(x, y1),
                value: y1 - y0,
            });
        }
    }
    out
}

/// Paint snapped alignment lines and distance labels.
pub fn paint(painter: &Painter, view: &View, origin: Pos2, guides: &[Guide], measures: &[Measure]) {
    let stroke = Stroke::new(1.0, GUIDE);
    for g in guides {
        let (a, b) = match g.axis {
            Axis::X => (Point::new(g.pos, g.from), Point::new(g.pos, g.to)),
            Axis::Y => (Point::new(g.from, g.pos), Point::new(g.to, g.pos)),
        };
        painter.line_segment(
            [view.to_screen(origin, a), view.to_screen(origin, b)],
            stroke,
        );
    }
    for m in measures {
        let a = view.to_screen(origin, m.a);
        let b = view.to_screen(origin, m.b);
        painter.line_segment([a, b], stroke);
        // End ticks across the line.
        let along = (b - a).normalized();
        let across = Vec2::new(-along.y, along.x) * 3.0;
        for p in [a, b] {
            painter.line_segment([p - across, p + across], stroke);
        }
        let galley =
            painter.layout_no_wrap(fmt_px(m.value), FontId::proportional(11.0), Color32::WHITE);
        let mid = a + (b - a) / 2.0;
        // Labels sit beside vertical lines and below horizontal ones.
        let (anchor, offset) = if along.x.abs() > along.y.abs() {
            (Align2::CENTER_TOP, Vec2::new(0.0, 4.0))
        } else {
            (Align2::LEFT_CENTER, Vec2::new(6.0, 0.0))
        };
        let rect = anchor.anchor_size(mid + offset, galley.size());
        painter.rect_filled(
            rect.expand2(Vec2::new(3.0, 1.0)),
            CornerRadius::same(2),
            GUIDE,
        );
        painter.galley(rect.min, galley, Color32::WHITE);
    }
}

/// A curved double arrow drawn in place of the system cursor while
/// hovering the rotate zone or rotating (egui has no rotate cursor).
pub fn paint_rotate_cursor(painter: &Painter, at: Pos2) {
    let r = 7.0;
    let arc: Vec<Pos2> = (0..=12)
        .map(|i| {
            let t = std::f32::consts::PI * (1.0 + 0.5 * i as f32 / 12.0);
            at + Vec2::new(t.cos(), t.sin()) * r + Vec2::new(r * 0.5, r * 0.5)
        })
        .collect();
    let (start, end) = (arc[0], arc[arc.len() - 1]);
    for (width, color) in [(3.5, Color32::WHITE), (1.5, Color32::BLACK)] {
        let stroke = Stroke::new(width, color);
        painter.add(egui::Shape::line(arc.clone(), stroke));
        // Arrowheads at both ends, pointing along the arc.
        painter.line_segment([start, start + Vec2::new(-3.0, -3.0)], stroke);
        painter.line_segment([start, start + Vec2::new(3.0, -3.0)], stroke);
        painter.line_segment([end, end + Vec2::new(-3.0, -3.0)], stroke);
        painter.line_segment([end, end + Vec2::new(-3.0, 3.0)], stroke);
    }
}

/// The rotation in progress, next to the cursor.
pub fn paint_angle(painter: &Painter, at: Pos2, degrees: f64) {
    let galley = painter.layout_no_wrap(
        format!("{}°", fmt_px(degrees)),
        FontId::proportional(11.0),
        Color32::WHITE,
    );
    let rect = Align2::LEFT_TOP.anchor_size(at + Vec2::new(14.0, 14.0), galley.size());
    painter.rect_filled(
        rect.expand2(Vec2::new(4.0, 2.0)),
        CornerRadius::same(3),
        crate::canvas::SELECTION,
    );
    painter.galley(rect.min, galley, Color32::WHITE);
}

fn fmt_px(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_sibling_on_each_side() {
        let m = Rect::new(100.0, 100.0, 200.0, 200.0);
        let sibs = [
            Rect::new(250.0, 150.0, 300.0, 400.0), // right, gap 50
            Rect::new(230.0, 120.0, 240.0, 130.0), // right, gap 30: nearer
            Rect::new(0.0, 0.0, 80.0, 50.0),       // no overlap either way
            Rect::new(120.0, 260.0, 180.0, 300.0), // below, gap 60
        ];
        let d = distances(m, &sibs, None);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].value, 30.0);
        assert_eq!(d[0].a, Point::new(200.0, 125.0));
        assert_eq!(d[0].b, Point::new(230.0, 125.0));
        assert_eq!(d[1].value, 60.0);
        assert_eq!(d[1].a, Point::new(150.0, 200.0));
    }

    #[test]
    fn parent_edges_when_no_sibling() {
        let m = Rect::new(10.0, 20.0, 110.0, 70.0);
        let parent = Rect::new(0.0, 0.0, 400.0, 300.0);
        let d = distances(m, &[], Some(parent));
        let values: Vec<f64> = d.iter().map(|m| m.value).collect();
        assert_eq!(values, vec![10.0, 290.0, 20.0, 230.0]);
        // Touching edges have no label.
        let m = Rect::new(0.0, 20.0, 110.0, 70.0);
        assert_eq!(distances(m, &[], Some(parent)).len(), 3);
        assert_eq!(fmt_px(12.0), "12");
        assert_eq!(fmt_px(12.04), "12");
        assert_eq!(fmt_px(0.25), "0.3");
    }
}
