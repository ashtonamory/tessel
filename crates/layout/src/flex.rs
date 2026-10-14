//! Auto-layout (§3.5, §7.3): a persistent taffy tree mirroring every frame
//! with auto-layout and its flow children, keyed by `NodeId`.
//!
//! A *layout root* is an auto-layout frame that is not itself placed by an
//! auto-layout parent. Each pass re-syncs only the roots whose subtrees were
//! touched (or every root, when tokens or styles changed): styles, leaf
//! contexts and child lists are compared with what taffy already has and set
//! only when they differ, so taffy recomputes only the dirty subtrees.
//!
//! Mapping rules:
//! - Hidden and `position: absolute` children are not in the flex flow;
//!   absolute children keep their stored `x`/`y` and size (CSS
//!   `position: absolute; left; top`). Constraints apply to them on resize,
//!   as in frames without auto-layout (see [`crate::apply_constraints`]).
//! - Text nodes are leaves measured with parley; groups are fixed-size leaves
//!   the size of their content; other kinds are fixed-size (or fill) leaves.
//! - Rotation does not affect layout (as with a CSS transform).

use std::collections::{BTreeSet, HashMap, HashSet};

use doc::{
    Align, AutoLayout, ChildPosition, Component, Direction, Document, Justify, Node, NodeId,
    NodeKind, Sizing, SpaceRef, Style as DocStyle, TextAlign, TextSizing, TextStyle, Tokens,
};
use taffy::{
    AlignContent, AlignItems, AvailableSpace, Dimension, Display, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, Style, TaffyTree,
};

use crate::text::TextMeasurer;

/// A text leaf's measure inputs. Compared on sync: a change marks it dirty.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextLeaf {
    pub content: String,
    pub family: String,
    pub style: TextStyle,
    pub align: TextAlign,
    /// Lines break at the available width (otherwise only at `\n`).
    pub wrap: bool,
}

struct Entry {
    taffy: taffy::NodeId,
    /// The layout root whose subtree this node belongs to.
    root: NodeId,
    /// The pass that last synced this node.
    epoch: u64,
}

/// The persistent taffy tree and its bookkeeping.
pub(crate) struct FlexTree {
    tree: TaffyTree<TextLeaf>,
    nodes: HashMap<NodeId, Entry>,
    /// Content bounds of groups placed by auto-layout, in the group's own
    /// coordinates (before its `x`/`y`).
    group_content: HashMap<NodeId, kurbo::Rect>,
    epoch: u64,
    /// Roots to re-sync this pass.
    all_dirty: bool,
    dirty: HashSet<NodeId>,
    /// Roots synced / reached this pass.
    synced: HashSet<NodeId>,
    seen: HashSet<NodeId>,
    /// Tokens, styles and components at the last pass: spacing tokens, font
    /// tokens, text styles and component masters (which instances show)
    /// change layout without touching any page node.
    deps: Option<(Tokens, Vec<DocStyle>, Vec<Component>)>,
}

impl Default for FlexTree {
    fn default() -> Self {
        let mut tree = TaffyTree::new();
        // Keep fractional positions, as a browser does; write-back rounds.
        tree.disable_rounding();
        FlexTree {
            tree,
            nodes: HashMap::new(),
            group_content: HashMap::new(),
            epoch: 0,
            all_dirty: true,
            dirty: HashSet::new(),
            synced: HashSet::new(),
            seen: HashSet::new(),
            deps: None,
        }
    }
}

impl FlexTree {
    /// Start a pass. `touched` is the changed node set of §5.5; `None`
    /// re-syncs every root.
    pub fn begin_pass(&mut self, doc: &Document, touched: Option<&BTreeSet<NodeId>>) {
        self.epoch += 1;
        self.synced.clear();
        self.seen.clear();
        self.dirty.clear();
        let deps_changed = self
            .deps
            .as_ref()
            .is_none_or(|(t, s, c)| *t != doc.tokens || *s != doc.styles || *c != doc.components);
        if deps_changed {
            self.deps = Some((
                doc.tokens.clone(),
                doc.styles.clone(),
                doc.components.clone(),
            ));
        }
        self.all_dirty = touched.is_none() || deps_changed;
        if self.all_dirty {
            return;
        }
        // A touched node dirties the root of every mirrored node on its
        // ancestor chain; a deleted node, the root it belonged to. Roots not
        // mirrored yet are always synced.
        for id in touched.into_iter().flatten() {
            let mut cur = Some(id.clone());
            while let Some(id) = cur {
                if let Some(e) = self.nodes.get(&id) {
                    self.dirty.insert(e.root.clone());
                }
                cur = match doc.parent_of(&id) {
                    Some((doc::ParentRef::Node(p), _)) => Some(p),
                    _ => None,
                };
            }
        }
    }

    /// Whether the root `id` must be re-synced before it is computed.
    pub fn needs_sync(&self, id: &NodeId) -> bool {
        !self.synced.contains(id)
            && (self.all_dirty
                || self.dirty.contains(id)
                || self.nodes.get(id).is_none_or(|e| &e.root != id))
    }

    pub fn mark_synced(&mut self, root: &NodeId) {
        self.synced.insert(root.clone());
    }

    pub fn set_group_content(&mut self, id: &NodeId, r: kurbo::Rect) {
        self.group_content.insert(id.clone(), r);
    }

    pub fn group_content(&self, id: &NodeId) -> kurbo::Rect {
        self.group_content
            .get(id)
            .copied()
            .unwrap_or(kurbo::Rect::ZERO)
    }

    /// Create or update the taffy node of `id`, touching taffy only where
    /// something differs.
    pub fn upsert(
        &mut self,
        id: &NodeId,
        root: &NodeId,
        style: Style,
        leaf: Option<TextLeaf>,
        children: &[taffy::NodeId],
    ) -> taffy::NodeId {
        let t = match self.nodes.get_mut(id) {
            Some(e) => {
                e.root = root.clone();
                e.epoch = self.epoch;
                let t = e.taffy;
                if self.tree.style(t).ok() != Some(&style) {
                    self.tree.set_style(t, style).expect("live taffy node");
                }
                if self.tree.get_node_context(t) != leaf.as_ref() {
                    self.tree
                        .set_node_context(t, leaf)
                        .expect("live taffy node");
                }
                t
            }
            None => {
                let t = match leaf {
                    Some(l) => self.tree.new_leaf_with_context(style, l),
                    None => self.tree.new_leaf(style),
                }
                .expect("taffy node");
                self.nodes.insert(
                    id.clone(),
                    Entry {
                        taffy: t,
                        root: root.clone(),
                        epoch: self.epoch,
                    },
                );
                t
            }
        };
        if self.tree.children(t).ok().as_deref() != Some(children) {
            self.tree
                .set_children(t, children)
                .expect("live taffy nodes");
        }
        if id == root {
            if let Some(p) = self.tree.parent(t) {
                let _ = self.tree.remove_child(p, t);
            }
        }
        t
    }

    /// Lay out the root `id` (cheap when nothing in it is dirty) and return
    /// its size.
    pub fn compute(&mut self, id: &NodeId, text: &mut TextMeasurer) -> Option<kurbo::Size> {
        self.seen.insert(id.clone());
        let t = self.nodes.get(id)?.taffy;
        self.tree
            .compute_layout_with_measure(
                t,
                taffy::Size {
                    width: AvailableSpace::MaxContent,
                    height: AvailableSpace::MaxContent,
                },
                |inputs, _, leaf, style| {
                    taffy::compute_leaf_layout(
                        inputs,
                        style,
                        |_, _| 0.0,
                        |known, available| match leaf {
                            Some(leaf) => measure(text, leaf, known, available),
                            None => taffy::Size::ZERO,
                        },
                    )
                },
            )
            .ok()?;
        let s = self.tree.layout(t).ok()?.size;
        Some(kurbo::Size::new(s.width as f64, s.height as f64))
    }

    /// The box of `child` in `parent`'s coordinates, if `parent` lays it out.
    pub fn placed(&self, parent: &NodeId, child: &NodeId) -> Option<kurbo::Rect> {
        let p = self.nodes.get(parent)?.taffy;
        let c = self.nodes.get(child)?.taffy;
        if self.tree.parent(c) != Some(p) {
            return None;
        }
        let l = self.tree.layout(c).ok()?;
        Some(kurbo::Rect::from_origin_size(
            (l.location.x as f64, l.location.y as f64),
            (l.size.width as f64, l.size.height as f64),
        ))
    }

    /// Drop nodes of roots that no longer exist and nodes that left a
    /// re-synced root.
    pub fn end_pass(&mut self) {
        let stale: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, e)| {
                !self.seen.contains(&e.root)
                    || (self.synced.contains(&e.root) && e.epoch != self.epoch)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(e) = self.nodes.remove(&id) {
                let _ = self.tree.remove(e.taffy);
            }
            self.group_content.remove(&id);
        }
    }

    /// Number of mirrored nodes (for tests).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the last pass re-synced the root `id` (for tests).
    #[cfg(test)]
    pub fn was_synced(&self, id: &NodeId) -> bool {
        self.synced.contains(id)
    }
}

fn measure(
    text: &mut TextMeasurer,
    leaf: &TextLeaf,
    known: taffy::Size<Option<f32>>,
    available: taffy::Size<AvailableSpace>,
) -> taffy::Size<f32> {
    if let (Some(width), Some(height)) = (known.width, known.height) {
        return taffy::Size { width, height };
    }
    let wrap_at = if leaf.wrap {
        known.width.or(match available.width {
            AvailableSpace::Definite(w) => Some(w),
            AvailableSpace::MinContent => Some(0.0),
            AvailableSpace::MaxContent => None,
        })
    } else {
        None
    };
    let l = text.layout(
        &leaf.content,
        &leaf.family,
        &leaf.style,
        leaf.align,
        wrap_at,
    );
    taffy::Size {
        width: known.width.unwrap_or(l.width()),
        height: known.height.unwrap_or(l.height()),
    }
}

/// Whether a flow child is laid out by its auto-layout parent.
pub(crate) fn in_flow(n: &Node) -> bool {
    n.visible && n.layout_child.position == ChildPosition::Flow
}

/// Whether a text node placed by auto-layout breaks lines at its width.
pub(crate) fn text_wraps(n: &Node) -> bool {
    match &n.kind {
        NodeKind::Text(t) => {
            t.sizing != TextSizing::AutoWidth || n.layout_child.width == Sizing::Fill
        }
        _ => false,
    }
}

/// The sizing a node lays out with on each axis, `(width, height)`.
/// `in_flow` is whether an auto-layout parent places it.
///
/// - An auto-layout frame uses its own `layout.width`/`height`; inside an
///   auto-layout parent a non-`fixed` `layout_child` sizing overrides it.
///   Outside one, `fill` means `fixed`.
/// - Text sizes as its `sizing` says (`auto_width` hugs both axes,
///   `auto_height` hugs the height); `layout_child` can only make it `fill`.
/// - Groups are fixed (their content bounds).
/// - Everything else is `fixed`, or `fill` from `layout_child`; `hug` means
///   `fixed` since there is no flow content to hug.
pub fn effective_sizing(node: &Node, in_flow: bool) -> (Sizing, Sizing) {
    use Sizing::*;
    let lc = &node.layout_child;
    let fill_or = |c: Sizing, d: Sizing| if in_flow && c == Fill { Fill } else { d };
    match &node.kind {
        NodeKind::Frame(f) if f.layout.is_some() => {
            let al = f.layout.as_ref().expect("checked");
            let pick = |c: Sizing, own: Sizing| match (in_flow, c, own) {
                (true, Hug | Fill, _) => c,
                (false, _, Fill) => Fixed,
                _ => own,
            };
            (pick(lc.width, al.width), pick(lc.height, al.height))
        }
        NodeKind::Text(t) => {
            let (w, h) = match t.sizing {
                TextSizing::AutoWidth => (Hug, Hug),
                TextSizing::AutoHeight => (Fixed, Hug),
                TextSizing::Fixed => (Fixed, Fixed),
            };
            (fill_or(lc.width, w), fill_or(lc.height, h))
        }
        NodeKind::Group(_) => (Fixed, Fixed),
        _ => (fill_or(lc.width, Fixed), fill_or(lc.height, Fixed)),
    }
}

/// A spacing value in px; unknown tokens are 0.
pub fn resolve_space(doc: &Document, s: &SpaceRef) -> f32 {
    match s {
        SpaceRef::Px(v) => *v,
        SpaceRef::Token(t) => doc.tokens.spacing.get(t).copied().unwrap_or(0.0),
    }
    .max(0.0)
}

/// `(main-axis gap, gap between wrapped lines)` in px. Without a
/// `cross_gap`, lines are `gap` apart too (CSS `gap: G`).
pub fn resolve_gaps(doc: &Document, al: &AutoLayout) -> (f32, f32) {
    let gap = resolve_space(doc, &al.gap);
    let cross = al.cross_gap.as_ref().map_or(gap, |g| resolve_space(doc, g));
    (gap, cross)
}

fn align_items(a: Align) -> AlignItems {
    match a {
        Align::Start => AlignItems::FLEX_START,
        Align::Center => AlignItems::CENTER,
        Align::End => AlignItems::FLEX_END,
        Align::Stretch => AlignItems::STRETCH,
        Align::Baseline => AlignItems::BASELINE,
    }
}

fn lpa(v: Option<f32>) -> LengthPercentageAuto {
    v.map_or(LengthPercentageAuto::auto(), |v| {
        LengthPercentageAuto::length(v.max(0.0))
    })
}

/// The taffy style of `node`. `parent` is its auto-layout parent when it is
/// in that parent's flow; `group_size` is a group's content size.
pub(crate) fn node_style(
    doc: &Document,
    node: &Node,
    parent: Option<&AutoLayout>,
    group_size: Option<kurbo::Size>,
) -> Style {
    let (sw, sh) = effective_sizing(node, parent.is_some());
    let (w, h) = match group_size {
        Some(s) => (s.width as f32, s.height as f32),
        None => (node.width.max(0.0), node.height.max(0.0)),
    };
    let dim = |s: Sizing, v: f32| match s {
        Sizing::Fixed => Dimension::length(v),
        Sizing::Hug | Sizing::Fill => Dimension::auto(),
    };
    let mut s = Style {
        size: taffy::Size {
            width: dim(sw, w),
            height: dim(sh, h),
        },
        ..Style::default()
    };
    if let Some(p) = parent {
        let (main, cross) = match p.direction {
            Direction::Row => (sw, sh),
            Direction::Column => (sh, sw),
        };
        if main == Sizing::Fill {
            s.flex_grow = 1.0;
            s.flex_shrink = 1.0;
            s.flex_basis = Dimension::length(0.0);
        } else {
            s.flex_grow = 0.0;
            s.flex_shrink = 0.0;
        }
        s.align_self = if cross == Sizing::Fill {
            Some(AlignItems::STRETCH)
        } else {
            node.layout_child.align_self.map(align_items)
        };
        let lc = &node.layout_child;
        s.min_size = taffy::Size {
            width: lpa(lc.min_width),
            height: lpa(lc.min_height),
        };
        s.max_size = taffy::Size {
            width: lpa(lc.max_width),
            height: lpa(lc.max_height),
        };
    }
    if let NodeKind::Frame(f) = &node.kind {
        if let Some(al) = &f.layout {
            let (gap, cross_gap) = resolve_gaps(doc, al);
            let lp = LengthPercentage::length;
            s.display = Display::Flex;
            s.flex_direction = match al.direction {
                Direction::Row => FlexDirection::Row,
                Direction::Column => FlexDirection::Column,
            };
            s.flex_wrap = if al.wrap {
                FlexWrap::Wrap
            } else {
                FlexWrap::NoWrap
            };
            s.gap = match al.direction {
                Direction::Row => taffy::Size {
                    width: lp(gap),
                    height: lp(cross_gap),
                },
                Direction::Column => taffy::Size {
                    width: lp(cross_gap),
                    height: lp(gap),
                },
            };
            let pad = |i: usize| lp(resolve_space(doc, &al.padding[i]));
            s.padding = taffy::Rect {
                top: pad(0),
                right: pad(1),
                bottom: pad(2),
                left: pad(3),
            };
            s.justify_content = match al.justify {
                Justify::Start => JustifyContent::FLEX_START,
                Justify::Center => JustifyContent::CENTER,
                Justify::End => JustifyContent::FLEX_END,
                Justify::SpaceBetween => JustifyContent::SPACE_BETWEEN,
            };
            s.align_items = align_items(al.align);
            // Wrapped lines pack at the start, as in design tools.
            s.align_content = AlignContent::FLEX_START;
        }
    }
    s
}

/// The measure inputs of a text node.
pub(crate) fn text_leaf(doc: &Document, node: &Node) -> Option<TextLeaf> {
    let NodeKind::Text(t) = &node.kind else {
        return None;
    };
    let style = crate::text::effective_text_style(doc, node)?;
    Some(TextLeaf {
        content: t.content.clone(),
        family: crate::text::resolve_family(doc, style),
        style: style.clone(),
        align: t.align,
        wrap: text_wraps(node),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use doc::{Frame, Id, IdKind, Page, Rect as RectKind};
    use kurbo::Rect;

    use super::*;
    use crate::{size_writeback, LayoutEngine, SizeWrite};

    fn id(n: u64) -> NodeId {
        Id::from_entropy(IdKind::Node, n)
    }

    fn rect(n: u64, w: f32, h: f32) -> Node {
        Node::new(id(n), "r", NodeKind::Rect(RectKind::default())).with_geometry(0.0, 0.0, w, h)
    }

    fn row(n: u64, x: f32, y: f32, children: Vec<Node>) -> Node {
        Node::new(
            id(n),
            "f",
            NodeKind::Frame(Frame {
                layout: Some(AutoLayout {
                    direction: Direction::Row,
                    wrap: false,
                    gap: SpaceRef::Token(doc::TokenId::new("space.gap")),
                    cross_gap: None,
                    padding: Default::default(),
                    justify: Justify::Start,
                    align: Align::Start,
                    width: Sizing::Hug,
                    height: Sizing::Hug,
                    extra: Default::default(),
                }),
                children,
                ..Default::default()
            }),
        )
        .with_geometry(x, y, 1.0, 1.0)
    }

    /// Two hug rows: 1 = [10, 11], 2 = [20, 21]; gap token 10.
    fn doc() -> Document {
        let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
        page.children = vec![
            row(
                1,
                0.0,
                0.0,
                vec![rect(10, 20.0, 20.0), rect(11, 30.0, 20.0)],
            ),
            row(
                2,
                0.0,
                100.0,
                vec![rect(20, 20.0, 20.0), rect(21, 30.0, 20.0)],
            ),
        ];
        let mut d = Document::new("t", page);
        d.tokens
            .spacing
            .insert(doc::TokenId::new("space.gap"), 10.0);
        d
    }

    fn bounds(g: &crate::Geometry, n: u64) -> Rect {
        g.get(&id(n)).unwrap().bounds
    }

    fn touched(ids: &[u64]) -> BTreeSet<NodeId> {
        ids.iter().map(|n| id(*n)).collect()
    }

    #[test]
    fn update_resyncs_only_touched_roots() {
        let mut d = doc();
        let mut e = LayoutEngine::new();
        let g = e.compute(&d);
        assert_eq!(bounds(&g, 1), Rect::new(0.0, 0.0, 60.0, 20.0));
        assert_eq!(bounds(&g, 11), Rect::new(30.0, 0.0, 60.0, 20.0));
        assert_eq!(e.flex.len(), 6);

        d.node_mut(&id(10)).unwrap().width = 50.0;
        let g = e.update(&d, &touched(&[10]));
        assert!(e.flex.was_synced(&id(1)));
        assert!(!e.flex.was_synced(&id(2)));
        assert_eq!(bounds(&g, 1), Rect::new(0.0, 0.0, 90.0, 20.0));
        assert_eq!(bounds(&g, 11), Rect::new(60.0, 0.0, 90.0, 20.0));
        assert_eq!(bounds(&g, 2), Rect::new(0.0, 100.0, 60.0, 120.0));

        // Nothing touched: nothing re-synced, same result.
        let again = e.update(&d, &BTreeSet::new());
        assert!(!e.flex.was_synced(&id(1)) && !e.flex.was_synced(&id(2)));
        assert_eq!(bounds(&again, 11), bounds(&g, 11));
    }

    #[test]
    fn token_changes_relayout_everything() {
        let mut d = doc();
        let mut e = LayoutEngine::new();
        e.compute(&d);
        d.tokens.spacing.insert(doc::TokenId::new("space.gap"), 4.0);
        let g = e.update(&d, &BTreeSet::new());
        assert_eq!(bounds(&g, 11).x0, 24.0);
        assert_eq!(bounds(&g, 21).x0, 24.0);
    }

    #[test]
    fn deleting_moving_and_turning_off_auto_layout() {
        let mut d = doc();
        let mut e = LayoutEngine::new();
        e.compute(&d);

        // Delete 10: 11 moves to the start, the mirror drops it.
        let (parent, i) = d.parent_of(&id(10)).unwrap();
        d.children_of_mut(&parent).unwrap().remove(i);
        d.rebuild_index();
        let g = e.update(&d, &touched(&[10]));
        assert_eq!(bounds(&g, 11), Rect::new(0.0, 0.0, 30.0, 20.0));
        assert_eq!(e.flex.len(), 5);

        // Move 21 from row 2 into row 1 (touches 21 and row 1).
        let (parent, i) = d.parent_of(&id(21)).unwrap();
        let n = d.children_of_mut(&parent).unwrap().remove(i);
        d.node_mut(&id(1)).unwrap().children_mut().unwrap().push(n);
        d.rebuild_index();
        let g = e.update(&d, &touched(&[21, 1]));
        assert_eq!(bounds(&g, 21), Rect::new(40.0, 0.0, 70.0, 20.0));
        assert_eq!(bounds(&g, 2), Rect::new(0.0, 100.0, 20.0, 120.0));
        assert_eq!(e.flex.len(), 5);

        // Turn auto-layout off on row 1: children go back to stored x/y.
        if let NodeKind::Frame(f) = &mut d.node_mut(&id(1)).unwrap().kind {
            f.layout = None;
        }
        let g = e.update(&d, &touched(&[1]));
        assert_eq!(bounds(&g, 21), Rect::new(0.0, 0.0, 30.0, 20.0));
        assert_eq!(bounds(&g, 1), Rect::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(e.flex.len(), 2);
    }

    #[test]
    fn hidden_children_leave_and_rejoin_the_flow() {
        let mut d = doc();
        let mut e = LayoutEngine::new();
        e.compute(&d);
        d.node_mut(&id(10)).unwrap().visible = false;
        let g = e.update(&d, &touched(&[10]));
        assert_eq!(bounds(&g, 11).x0, 0.0);
        d.node_mut(&id(10)).unwrap().visible = true;
        let g = e.update(&d, &touched(&[10]));
        assert_eq!(bounds(&g, 11).x0, 30.0);
    }

    #[test]
    fn writeback_reports_rounded_changes_only() {
        let mut d = doc();
        let mut e = LayoutEngine::new();
        let g = e.compute(&d);
        let w = size_writeback(&d, &g);
        assert_eq!(
            w,
            vec![
                SizeWrite {
                    id: id(1),
                    width: 60.0,
                    height: 20.0
                },
                SizeWrite {
                    id: id(2),
                    width: 60.0,
                    height: 20.0
                },
            ]
        );
        for s in &w {
            let n = d.node_mut(&s.id).unwrap();
            n.width = s.width;
            n.height = s.height;
        }
        assert!(size_writeback(&d, &e.update(&d, &touched(&[1, 2]))).is_empty());
        // A difference below the file's 0.01 precision is not written.
        d.node_mut(&id(1)).unwrap().width = 60.004;
        assert!(size_writeback(&d, &e.update(&d, &touched(&[1]))).is_empty());
    }

    #[test]
    fn sizing_rules() {
        use Sizing::*;
        let mut r = rect(1, 10.0, 10.0);
        assert_eq!(effective_sizing(&r, true), (Fixed, Fixed));
        r.layout_child.width = Fill;
        r.layout_child.height = Hug;
        assert_eq!(effective_sizing(&r, true), (Fill, Fixed));
        assert_eq!(effective_sizing(&r, false), (Fixed, Fixed));
        let mut f = row(2, 0.0, 0.0, vec![]);
        assert_eq!(effective_sizing(&f, false), (Hug, Hug));
        f.layout_child.width = Fill;
        assert_eq!(effective_sizing(&f, true), (Fill, Hug));
        assert_eq!(effective_sizing(&f, false), (Hug, Hug));
    }

    /// A hug row holding one auto-width text node, measured with `update`.
    fn text_row_doc(style: TextStyle, text_style: Option<doc::StyleId>) -> Document {
        let t = Node::new(
            id(30),
            "t",
            NodeKind::Text(doc::Text {
                content: "Hello, wide world".into(),
                style,
                text_style,
                ..Default::default()
            }),
        )
        .with_geometry(0.0, 0.0, 1.0, 1.0);
        let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
        page.children = vec![row(1, 0.0, 0.0, vec![t])];
        Document::new("t", page)
    }

    fn row_width(engine: &mut LayoutEngine, d: &Document) -> f64 {
        let g = engine.update(d, &BTreeSet::new());
        g.get(&id(1)).unwrap().bounds.width()
    }

    #[test]
    fn font_token_change_remeasures_hug_text_without_touched_nodes() {
        let token = doc::TokenId::new("font.body");
        let style = TextStyle {
            font_family: "{font.body}".into(),
            ..Default::default()
        };
        let mut d = text_row_doc(style, None);
        d.tokens
            .fonts
            .insert(token.clone(), "Liberation Sans".into());
        let mut engine = LayoutEngine::new();
        // Independent of installed fonts: check the family measured with,
        // not the pixel width.
        let family = |d: &Document| text_leaf(d, d.node(&id(30)).unwrap()).unwrap().family;
        assert_eq!(family(&d), "Liberation Sans");
        row_width(&mut engine, &d);
        d.tokens.fonts.insert(token, "Liberation Mono".into());
        assert_eq!(family(&d), "Liberation Mono");
        // The flex tree notices the token change and re-syncs every root
        // with no touched nodes.
        engine.flex.begin_pass(&d, Some(&BTreeSet::new()));
        assert!(engine.flex.needs_sync(&id(1)));
        row_width(&mut engine, &d);
    }

    #[test]
    fn text_layout_cache_is_keyed_by_the_resolved_family() {
        let style = TextStyle {
            font_family: "{font.body}".into(),
            ..Default::default()
        };
        let mut m = crate::TextMeasurer::new();
        let sans = m.layout("Hi", "Liberation Sans", &style, TextAlign::Left, None);
        let mono = m.layout("Hi", "Liberation Mono", &style, TextAlign::Left, None);
        // Same token string in the style, different resolved family: no
        // stale layout is reused.
        assert!(!std::sync::Arc::ptr_eq(&sans, &mono));
        let again = m.layout("Hi", "Liberation Sans", &style, TextAlign::Left, None);
        assert!(std::sync::Arc::ptr_eq(&sans, &again));
    }

    #[test]
    fn text_style_change_remeasures_hug_text_without_touched_nodes() {
        let sid = Id::from_entropy(IdKind::Style, 7);
        let def = |size: f32| DocStyle {
            id: sid.clone(),
            name: "Body".into(),
            kind: doc::StyleKind::Text(doc::TextStyleDef {
                style: TextStyle {
                    font_size: size,
                    ..Default::default()
                },
                extra: Default::default(),
            }),
        };
        let mut d = text_row_doc(TextStyle::default(), Some(sid.clone()));
        d.styles.push(def(12.0));
        let mut engine = LayoutEngine::new();
        let small = row_width(&mut engine, &d);
        d.styles[0] = def(24.0);
        let big = row_width(&mut engine, &d);
        assert!(big > small * 1.5, "{small} vs {big}");
    }
}
