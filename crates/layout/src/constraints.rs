//! Constraints (§3.5): where a child of a frame without auto-layout goes
//! when the frame resizes. Pure; the app applies the result on resize.

use doc::{Constraints, HConstraint, VConstraint};
use kurbo::{Rect, Size};

/// The child's new box when its parent goes from `old_parent` to
/// `new_parent`. `child` is in the parent's coordinates (its stored `x`,
/// `y`, `width`, `height`).
///
/// Per axis: `left`/`top` keeps the near offset, `right`/`bottom` the far
/// offset, `left_right`/`top_bottom` both (the child stretches),
/// `center` keeps the offset of the child's center from the parent's
/// center, and `scale` scales position and size with the parent.
pub fn apply_constraints(
    old_parent: Size,
    new_parent: Size,
    child: Rect,
    constraints: &Constraints,
) -> Rect {
    let h = match constraints.horizontal {
        HConstraint::Left => Axis::Start,
        HConstraint::Right => Axis::End,
        HConstraint::LeftRight => Axis::Both,
        HConstraint::Center => Axis::Center,
        HConstraint::Scale => Axis::Scale,
    };
    let v = match constraints.vertical {
        VConstraint::Top => Axis::Start,
        VConstraint::Bottom => Axis::End,
        VConstraint::TopBottom => Axis::Both,
        VConstraint::Center => Axis::Center,
        VConstraint::Scale => Axis::Scale,
    };
    let (x, w) = h.apply(old_parent.width, new_parent.width, child.x0, child.width());
    let (y, ht) = v.apply(
        old_parent.height,
        new_parent.height,
        child.y0,
        child.height(),
    );
    Rect::from_origin_size((x, y), (w, ht))
}

#[derive(Clone, Copy)]
enum Axis {
    Start,
    End,
    Both,
    Center,
    Scale,
}

impl Axis {
    /// `(position, length)` on one axis.
    fn apply(self, old: f64, new: f64, pos: f64, len: f64) -> (f64, f64) {
        let d = new - old;
        match self {
            Axis::Start => (pos, len),
            Axis::End => (pos + d, len),
            Axis::Both => (pos, (len + d).max(0.0)),
            Axis::Center => (pos + d / 2.0, len),
            Axis::Scale if old > 0.0 => {
                let k = new / old;
                (pos * k, len * k)
            }
            Axis::Scale => (pos, len),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(h: HConstraint, v: VConstraint) -> Constraints {
        Constraints {
            horizontal: h,
            vertical: v,
            ..Default::default()
        }
    }

    const OLD: Size = Size::new(200.0, 100.0);
    const NEW: Size = Size::new(300.0, 150.0);
    const CHILD: Rect = Rect::new(20.0, 10.0, 60.0, 30.0);

    #[test]
    fn left_top_keeps_the_box() {
        let r = apply_constraints(OLD, NEW, CHILD, &c(HConstraint::Left, VConstraint::Top));
        assert_eq!(r, CHILD);
    }

    #[test]
    fn right_bottom_keep_the_far_offset() {
        let r = apply_constraints(OLD, NEW, CHILD, &c(HConstraint::Right, VConstraint::Bottom));
        assert_eq!(r, Rect::new(120.0, 60.0, 160.0, 80.0));
        assert_eq!(NEW.width - r.x1, OLD.width - CHILD.x1);
    }

    #[test]
    fn left_right_stretches() {
        let r = apply_constraints(
            OLD,
            NEW,
            CHILD,
            &c(HConstraint::LeftRight, VConstraint::TopBottom),
        );
        assert_eq!(r, Rect::new(20.0, 10.0, 160.0, 80.0));
        // Shrinking past the child's size clamps it to zero.
        let r = apply_constraints(
            OLD,
            Size::new(100.0, 60.0),
            CHILD,
            &c(HConstraint::LeftRight, VConstraint::TopBottom),
        );
        assert_eq!(r, Rect::new(20.0, 10.0, 20.0, 10.0));
    }

    #[test]
    fn center_keeps_the_center_offset() {
        let r = apply_constraints(
            OLD,
            NEW,
            CHILD,
            &c(HConstraint::Center, VConstraint::Center),
        );
        assert_eq!(r, Rect::new(70.0, 35.0, 110.0, 55.0));
        assert_eq!(
            r.center().x - NEW.width / 2.0,
            CHILD.center().x - OLD.width / 2.0
        );
    }

    #[test]
    fn scale_scales_position_and_size() {
        let r = apply_constraints(OLD, NEW, CHILD, &c(HConstraint::Scale, VConstraint::Scale));
        assert_eq!(r, Rect::new(30.0, 15.0, 90.0, 45.0));
        // A zero-size parent cannot scale; the box stays.
        let r = apply_constraints(
            Size::ZERO,
            NEW,
            CHILD,
            &c(HConstraint::Scale, VConstraint::Scale),
        );
        assert_eq!(r, CHILD);
    }

    #[test]
    fn axes_are_independent() {
        let r = apply_constraints(OLD, NEW, CHILD, &c(HConstraint::Right, VConstraint::Top));
        assert_eq!(r, Rect::new(120.0, 10.0, 160.0, 30.0));
    }
}
