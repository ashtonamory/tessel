//! The canvas widget (§7.1, §9): pan/zoom, tools, selection, move/resize,
//! inline text editing, the vello texture and the selection chrome.

use doc::{
    ColorRef, Command, Frame, Id, IdKind, Node, NodeId, NodeKind, NodePatch, Paint, ParentRef,
    Rect as RectKind, Shape, Text, TextSizing,
};
use egui::{
    Color32, CornerRadius, CursorIcon, FontId, Key, PointerButton, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextEdit, Ui, Vec2,
};
use kurbo::{Affine, Point};
use layout::{hit_test, marquee_select, HitOptions};
use render::{CanvasTarget, ImageCache, SceneCache, SceneOptions, SceneRenderer};

use crate::auto_layout;
use crate::components_panel::{self, ComponentDrag};
use crate::editor::{Editor, SceneDirty};
use crate::guides::{self, Measure};
use crate::layout_handles::{self, Reorder, SpacingDrag};
use crate::snap::{self, Edges, Guide, SnapOptions, Targets};

pub const SELECTION: Color32 = Color32::from_rgb(0x0d, 0x99, 0xff);
const HANDLE: f32 = 8.0;
/// How far past a corner handle (screen points) dragging rotates.
const ROTATE_ZONE: f32 = 16.0;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 256.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Move,
    Frame,
    Rect,
    Ellipse,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::Move,
        Tool::Frame,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Text,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Move => "Move",
            Tool::Frame => "Frame",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Text => "Text",
        }
    }

    pub fn key(self) -> Key {
        match self {
            Tool::Move => Key::V,
            Tool::Frame => Key::F,
            Tool::Rect => Key::R,
            Tool::Ellipse => Key::O,
            Tool::Text => Key::T,
        }
    }
}

/// Page → screen mapping: `screen = canvas_origin + offset + page * zoom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub offset: Vec2,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            offset: Vec2::new(40.0, 40.0),
            zoom: 1.0,
        }
    }
}

impl View {
    pub fn to_screen(&self, origin: Pos2, p: Point) -> Pos2 {
        origin + self.offset + Vec2::new(p.x as f32, p.y as f32) * self.zoom
    }

    pub fn to_page(&self, origin: Pos2, s: Pos2) -> Point {
        let v = (s - origin - self.offset) / self.zoom;
        Point::new(v.x as f64, v.y as f64)
    }

    pub fn rect_to_screen(&self, origin: Pos2, r: kurbo::Rect) -> Rect {
        Rect::from_two_pos(
            self.to_screen(origin, Point::new(r.x0, r.y0)),
            self.to_screen(origin, Point::new(r.x1, r.y1)),
        )
    }

    /// Zoom by `factor`, keeping the page point under `anchor` in place.
    pub fn zoom_around(&mut self, origin: Pos2, anchor: Pos2, factor: f32) {
        let page = self.to_page(origin, anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = anchor - origin - Vec2::new(page.x as f32, page.y as f32) * self.zoom;
    }

    /// Show `bounds` (page units) centered in a canvas of `size`.
    pub fn fit(&mut self, size: Vec2, bounds: kurbo::Rect, max_zoom: f32) {
        let (w, h) = (
            bounds.width().max(1.0) as f32,
            bounds.height().max(1.0) as f32,
        );
        self.zoom = (size.x / w)
            .min(size.y / h)
            .mul_add(0.9, 0.0)
            .clamp(MIN_ZOOM, max_zoom);
        let c = bounds.center();
        self.offset = size / 2.0 - Vec2::new(c.x as f32, c.y as f32) * self.zoom;
    }

    /// Page → texture pixels for a canvas at `pixels_per_point`.
    pub fn affine(&self, pixels_per_point: f32) -> Affine {
        Affine::scale(pixels_per_point as f64)
            * Affine::translate((self.offset.x as f64, self.offset.y as f64))
            * Affine::scale(self.zoom as f64)
    }
}

/// A resize handle, as fractions of the selection box.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Handle {
    fx: f64,
    fy: f64,
}

const HANDLES: [Handle; 8] = [
    Handle { fx: 0.0, fy: 0.0 },
    Handle { fx: 0.5, fy: 0.0 },
    Handle { fx: 1.0, fy: 0.0 },
    Handle { fx: 1.0, fy: 0.5 },
    Handle { fx: 1.0, fy: 1.0 },
    Handle { fx: 0.5, fy: 1.0 },
    Handle { fx: 0.0, fy: 1.0 },
    Handle { fx: 0.0, fy: 0.5 },
];

impl Handle {
    fn cursor(self) -> CursorIcon {
        match (self.fx, self.fy) {
            (0.5, _) => CursorIcon::ResizeVertical,
            (_, 0.5) => CursorIcon::ResizeHorizontal,
            (0.0, 0.0) | (1.0, 1.0) => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        }
    }
}

#[derive(Clone, Debug)]
struct Original {
    id: NodeId,
    bounds: kurbo::Rect,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rotation: f32,
    /// The node's rotation pivot in its local box.
    pivot: Point,
    /// Parent-local → page.
    parent_tf: Affine,
}

enum Drag {
    Pan,
    Marquee {
        start: Point,
        current: Point,
        base: Vec<NodeId>,
    },
    Move {
        start: Point,
        bounds: kurbo::Rect,
        targets: Targets,
        originals: Vec<Original>,
    },
    Resize {
        handle: Handle,
        start: Point,
        bounds: kurbo::Rect,
        targets: Targets,
        originals: Vec<Original>,
        /// The resized nodes as they were, for constraints (§3.5).
        snapshots: Vec<Node>,
    },
    Rotate {
        center: Point,
        start: Point,
        originals: Vec<Original>,
        /// The angle applied so far, in degrees, for the label.
        angle: f64,
    },
    /// A padding or gap handle of an auto-layout frame.
    Spacing(SpacingDrag),
    /// A flow child moving among its siblings.
    Reorder(Reorder),
    Create {
        start: Point,
        id: NodeId,
        /// Page → parent-local coordinates.
        to_local: Affine,
    },
}

struct TextEditing {
    id: NodeId,
    buffer: String,
    /// Created by the text tool; empty text is dropped on commit.
    is_new: bool,
    focused: bool,
}

enum Gpu {
    Uninit,
    Ready {
        renderer: Box<SceneRenderer>,
        target: Option<(CanvasTarget, egui::TextureId)>,
    },
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct RenderKey {
    revision: u64,
    view: View,
    size: (u32, u32),
    pixels_per_point: f32,
    hidden: Option<NodeId>,
}

pub struct Canvas {
    pub view: View,
    pub tool: Tool,
    /// Cursor position on the page, for the status bar.
    pub cursor: Option<Point>,
    drag: Option<Drag>,
    hover: Option<NodeId>,
    editing: Option<TextEditing>,
    /// What the drag in progress snapped to, and the distances to show.
    guides: Vec<Guide>,
    measures: Vec<Measure>,
    /// Where to draw the rotate cursor this frame.
    rotate_cursor: Option<Pos2>,
    gpu: Gpu,
    scenes: SceneCache,
    last_render: Option<RenderKey>,
    /// Fit the content into view on the next frame.
    pub fit_pending: bool,
    /// Where the canvas was laid out last frame, in screen points.
    pub rect: Rect,
    size: Vec2,
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas::new()
    }
}

impl Canvas {
    pub fn new() -> Canvas {
        Canvas {
            view: View::default(),
            tool: Tool::Move,
            cursor: None,
            drag: None,
            hover: None,
            editing: None,
            guides: Vec::new(),
            measures: Vec::new(),
            rotate_cursor: None,
            gpu: Gpu::Uninit,
            scenes: SceneCache::new(),
            last_render: None,
            fit_pending: true,
            rect: Rect::NOTHING,
            size: Vec2::new(800.0, 600.0),
        }
    }

    /// The visible part of the page, in page coordinates.
    pub fn visible_page_rect(&self) -> kurbo::Rect {
        let a = self.view.to_page(self.rect.min, self.rect.min);
        let b = self.view.to_page(self.rect.min, self.rect.max);
        kurbo::Rect::new(a.x, a.y, b.x, b.y)
    }

    /// Forget per-document state after a new document is opened.
    pub fn reset(&mut self) {
        self.drag = None;
        self.hover = None;
        self.editing = None;
        self.scenes.invalidate_all();
        self.last_render = None;
        self.fit_pending = true;
        self.tool = Tool::Move;
    }

    pub fn is_editing_text(&self) -> bool {
        self.editing.is_some()
    }

    /// The text node being edited inline, if any.
    pub fn editing_node(&self) -> Option<&NodeId> {
        self.editing.as_ref().map(|e| &e.id)
    }

    /// The texture the canvas renders into and its egui texture ID, once
    /// the GPU renderer is running.
    pub fn target(&self) -> Option<(&CanvasTarget, egui::TextureId)> {
        match &self.gpu {
            Gpu::Ready {
                target: Some((t, id)),
                ..
            } => Some((t, *id)),
            _ => None,
        }
    }

    pub fn gpu_error(&self) -> Option<&str> {
        match &self.gpu {
            Gpu::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// Zoom so everything (or the selection) is visible.
    pub fn zoom_to_fit(&mut self, ed: &Editor, selection_only: bool) {
        let bounds = if selection_only {
            ed.geometry.union_bounds(&ed.selection)
        } else {
            ed.geometry.page_bounds(ed.page())
        };
        match bounds {
            Some(b) => self
                .view
                .fit(self.size, b, if selection_only { 8.0 } else { 1.0 }),
            None => self.view = View::default(),
        }
    }

    /// Set zoom to `zoom`, keeping the canvas center in place.
    pub fn zoom_to(&mut self, zoom: f32) {
        let center = Pos2::ZERO + self.size / 2.0;
        self.view
            .zoom_around(Pos2::ZERO, center, zoom / self.view.zoom);
    }

    /// Commit an inline text edit in progress.
    pub fn finish_text_edit(&mut self, ed: &mut Editor) {
        let Some(te) = self.editing.take() else {
            return;
        };
        let current = match ed.shown_node(&te.id).map(|n| n.kind) {
            Some(NodeKind::Text(t)) => t.content,
            _ => {
                ed.cancel_gesture();
                return;
            }
        };
        if te.is_new {
            if te.buffer.trim().is_empty() {
                ed.cancel_gesture();
                return;
            }
            if let Some(mut patch) = ed.text_patch(&te.id, &te.buffer) {
                patch.name = Some(text_layer_name(&te.buffer));
                if let Err(e) = ed.gesture_apply(Command::SetProps {
                    id: te.id.clone(),
                    patch,
                }) {
                    ed.status = Some(format!("Text: {e}"));
                }
            }
            ed.end_gesture();
        } else if te.buffer != current {
            if let Some(patch) = ed.text_patch(&te.id, &te.buffer) {
                let sel = ed.selection.clone();
                let _ = ed.commit(
                    "Edit text",
                    vec![Command::SetProps {
                        id: te.id.clone(),
                        patch,
                    }],
                    sel,
                );
            }
        }
        // The edited node was hidden from the scene; draw it again.
        self.last_render = None;
    }

    /// End any drag or text edit in progress, keeping its changes; called
    /// before undo, redo, save and document switches.
    pub fn interrupt(&mut self, ed: &mut Editor) {
        self.finish_text_edit(ed);
        if self.drag.take().is_some() {
            ed.end_gesture();
        }
    }

    /// Whether Escape has something to cancel here: a text edit, a drag or
    /// a drawing tool.
    pub fn is_busy(&self) -> bool {
        self.editing.is_some() || self.drag.is_some() || self.tool != Tool::Move
    }

    /// Abandon gestures and edits, e.g. on Escape.
    pub fn cancel(&mut self, ed: &mut Editor) {
        if self.editing.is_some() {
            self.finish_text_edit(ed);
        } else if self.drag.take().is_some() {
            ed.cancel_gesture();
        } else if self.tool != Tool::Move {
            self.tool = Tool::Move;
        } else {
            ed.selection.clear();
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, frame: &eframe::Frame, ed: &mut Editor) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.size = rect.size();
        self.rect = rect;
        let origin = rect.min;
        if self.fit_pending {
            self.fit_pending = false;
            self.zoom_to_fit(ed, false);
        }

        self.handle_input(ui, &resp, rect, ed);

        if !self.scenes.images().assets().same(&ed.assets) {
            self.scenes = SceneCache::with_images(ImageCache::new(ed.assets.clone()));
        }
        match std::mem::take(&mut ed.scene_dirty) {
            SceneDirty::Clean => {}
            SceneDirty::Nodes(ids) => self.scenes.invalidate(&ed.doc, &ids),
            SceneDirty::All => self.scenes.invalidate_all(),
        }

        self.paint_document(ui, frame, ed, rect);
        self.paint_chrome(ui, ed, rect);
        self.text_editor(ui, ed, origin, rect);
    }

    fn page_point(&self, origin: Pos2, p: Pos2) -> Point {
        self.view.to_page(origin, p)
    }

    fn handle_input(&mut self, ui: &mut Ui, resp: &egui::Response, rect: Rect, ed: &mut Editor) {
        let origin = rect.min;
        let ctx = ui.ctx().clone();
        let (hover_pos, scroll, zoom, mods, space) = ui.input(|i| {
            (
                i.pointer.hover_pos(),
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.modifiers,
                i.key_down(Key::Space),
            )
        });
        let space_pan = space && !ctx.egui_wants_keyboard_input();
        let pointer_in = hover_pos.filter(|p| rect.contains(*p));
        self.cursor = pointer_in.map(|p| self.page_point(origin, p));

        // Pan and zoom: two-finger scroll pans; pinch and Ctrl+scroll zoom
        // around the cursor (§7.8).
        if let Some(p) = pointer_in {
            if zoom != 1.0 {
                self.view.zoom_around(origin, p, zoom);
            } else if scroll != Vec2::ZERO {
                self.view.offset += scroll;
            }
        }

        let started = resp.drag_started();
        if (started || resp.clicked()) && self.editing.is_some() {
            self.finish_text_edit(ed);
        }

        if started {
            let press = ctx
                .input(|i| i.pointer.press_origin())
                .or(hover_pos)
                .unwrap_or(origin);
            if resp.dragged_by(PointerButton::Middle) || space_pan {
                self.drag = Some(Drag::Pan);
            } else if resp.dragged_by(PointerButton::Primary) {
                self.start_drag(ed, origin, press, mods);
            }
        }

        if resp.dragged() {
            if let Some(p) = hover_pos.or(ctx.input(|i| i.pointer.interact_pos())) {
                let page = self.page_point(origin, p);
                match &self.drag {
                    Some(Drag::Pan) => self.view.offset += resp.drag_delta(),
                    Some(_) => self.update_drag(ed, page, mods),
                    None => {}
                }
            }
        }

        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Create { .. }) => {
                    ed.end_gesture();
                    self.tool = Tool::Move;
                }
                Some(
                    Drag::Move { .. }
                    | Drag::Resize { .. }
                    | Drag::Rotate { .. }
                    | Drag::Spacing(_),
                ) => ed.end_gesture(),
                Some(Drag::Reorder(r)) => r.finish(ed),
                _ => {}
            }
        }

        if resp.clicked() {
            if let Some(p) = hover_pos {
                self.click(ed, origin, p, mods);
            }
        }
        if resp.double_clicked() && self.tool == Tool::Move {
            if let Some(p) = hover_pos {
                self.double_click(ed, origin, p);
            }
        }
        if let Some(drag) = resp.dnd_release_payload::<ComponentDrag>() {
            if let Some(p) = pointer_in {
                let at = self.page_point(origin, p);
                let view = self.visible_page_rect();
                components_panel::drop_on_canvas(ed, &drag, at, view);
                self.tool = Tool::Move;
            }
        }

        // Hover outline and cursor.
        self.hover = None;
        if self.drag.is_none() {
            if let Some(p) = pointer_in {
                if self.tool == Tool::Move {
                    let opts = HitOptions {
                        tolerance: 3.0 / self.view.zoom as f64,
                        selected: &ed.selection,
                        deep: mods.command,
                    };
                    self.hover =
                        hit_test(&ed.geometry, ed.page(), self.page_point(origin, p), opts);
                }
            }
        }
        let rotating = match (&self.drag, self.tool) {
            (Some(Drag::Rotate { .. }), _) => true,
            (None, Tool::Move) if !space_pan => {
                pointer_in.is_some_and(|p| self.rotate_at(ed, origin, p))
            }
            _ => false,
        };
        self.rotate_cursor = if rotating { hover_pos } else { None };
        let cursor = match (&self.drag, self.tool) {
            _ if rotating => Some(CursorIcon::None),
            (Some(Drag::Pan), _) => Some(CursorIcon::Grabbing),
            (Some(Drag::Resize { handle, .. }), _) => Some(handle.cursor()),
            _ if space_pan && pointer_in.is_some() => Some(CursorIcon::Grab),
            (None, Tool::Move) => pointer_in.and_then(|p| {
                self.handle_at(ed, origin, p)
                    .map(Handle::cursor)
                    .or_else(|| {
                        layout_handles::handle_at(ed, &self.view, origin, p)
                            .map(|(_, s)| s.cursor())
                    })
            }),
            (None, Tool::Text) if pointer_in.is_some() => Some(CursorIcon::Text),
            (None, _) if pointer_in.is_some() => Some(CursorIcon::Crosshair),
            _ => None,
        };
        if let Some(c) = cursor {
            ctx.set_cursor_icon(c);
        }
    }

    /// Selection bounds in page coordinates.
    fn selection_bounds(&self, ed: &Editor) -> Option<kurbo::Rect> {
        ed.geometry.union_bounds(&ed.selection)
    }

    fn handle_at(&self, ed: &Editor, origin: Pos2, p: Pos2) -> Option<Handle> {
        if ed.read_only || self.editing.is_some() {
            return None;
        }
        let b = self.selection_bounds(ed)?;
        HANDLES.into_iter().find(|h| {
            let hp = self.view.to_screen(
                origin,
                Point::new(b.x0 + b.width() * h.fx, b.y0 + b.height() * h.fy),
            );
            (hp - p).abs().max_elem() <= HANDLE * 0.75
        })
    }

    /// Just outside a corner handle of the selection: dragging there
    /// rotates.
    fn rotate_at(&self, ed: &Editor, origin: Pos2, p: Pos2) -> bool {
        if self.handle_at(ed, origin, p).is_some() {
            return false;
        }
        let Some(b) = self.selection_bounds(ed) else {
            return false;
        };
        if ed.read_only || self.editing.is_some() {
            return false;
        }
        let sr = self.view.rect_to_screen(origin, b);
        if sr.contains(p) {
            return false;
        }
        [
            sr.left_top(),
            sr.right_top(),
            sr.right_bottom(),
            sr.left_bottom(),
        ]
        .into_iter()
        .any(|c| (p - c).length() <= HANDLE * 0.75 + ROTATE_ZONE)
    }

    fn originals(ed: &Editor) -> Vec<Original> {
        ed.top_selected()
            .iter()
            .filter_map(|id| Some((ed.node(id)?, ed.geometry.get(id)?)))
            .filter(|(n, _)| !n.locked)
            .map(|(n, g)| {
                // Layout rotates about the center of the laid-out size;
                // groups keep their own width and height for that.
                let size = match n.kind {
                    NodeKind::Group(_) => kurbo::Size::new(n.width as f64, n.height as f64),
                    _ => g.size,
                };
                Original {
                    id: n.id.clone(),
                    bounds: g.bounds,
                    x: n.x,
                    y: n.y,
                    width: n.width,
                    height: n.height,
                    rotation: n.rotation,
                    pivot: Point::new(size.width / 2.0, size.height / 2.0),
                    parent_tf: parent_transform(ed, &n.id),
                }
            })
            .collect()
    }

    fn start_drag(&mut self, ed: &mut Editor, origin: Pos2, press: Pos2, mods: egui::Modifiers) {
        let start = self.page_point(origin, press);
        match self.tool {
            Tool::Move => {
                self.guides.clear();
                self.measures.clear();
                if let Some(handle) = self.handle_at(ed, origin, press) {
                    let Some(bounds) = self.selection_bounds(ed) else {
                        return;
                    };
                    if ed.begin_gesture("Resize").is_ok() {
                        let originals = Self::originals(ed);
                        let ids: Vec<NodeId> = originals.iter().map(|o| o.id.clone()).collect();
                        let snapshots = ids.iter().filter_map(|id| ed.node(id).cloned()).collect();
                        self.drag = Some(Drag::Resize {
                            handle,
                            start,
                            bounds,
                            targets: Targets::collect(ed, &ids),
                            originals,
                            snapshots,
                        });
                    }
                    return;
                }
                if let Some((frame, s)) = layout_handles::handle_at(ed, &self.view, origin, press) {
                    self.drag = SpacingDrag::start(ed, frame, s, start).map(Drag::Spacing);
                    return;
                }
                if self.rotate_at(ed, origin, press) {
                    let Some(bounds) = self.selection_bounds(ed) else {
                        return;
                    };
                    let originals = Self::originals(ed);
                    let label = match originals.len() {
                        0 => return,
                        1 => "Rotate layer".to_string(),
                        n => format!("Rotate {n} layers"),
                    };
                    if ed.begin_gesture(&label).is_ok() {
                        self.drag = Some(Drag::Rotate {
                            center: bounds.center(),
                            start,
                            originals,
                            angle: 0.0,
                        });
                    }
                    return;
                }
                let opts = HitOptions {
                    tolerance: 3.0 / self.view.zoom as f64,
                    selected: &ed.selection,
                    deep: mods.command,
                };
                match hit_test(&ed.geometry, ed.page(), start, opts) {
                    Some(hit) => {
                        if !ed.selection.contains(&hit) {
                            if mods.shift {
                                ed.selection.push(hit);
                            } else {
                                ed.selection = vec![hit];
                            }
                        }
                        // A flow child of an auto-layout frame moves among its
                        // siblings; Alt takes it out instead.
                        if let ([one], false) = (ed.top_selected().as_slice(), mods.alt) {
                            if let Some(r) = Reorder::start(ed, one) {
                                self.drag = Some(Drag::Reorder(r));
                                return;
                            }
                        }
                        let label = if ed.selection.len() > 1 {
                            format!("Move {} layers", ed.selection.len())
                        } else {
                            "Move layer".to_string()
                        };
                        if ed.begin_gesture(&label).is_ok() {
                            if mods.alt {
                                for id in ed.top_selected() {
                                    if let Err(e) = layout_handles::move_out(ed, &id) {
                                        ed.status = Some(e.to_string());
                                    }
                                }
                            }
                            let originals = Self::originals(ed);
                            let ids: Vec<NodeId> = originals.iter().map(|o| o.id.clone()).collect();
                            let bounds = ed.geometry.union_bounds(&ids).unwrap_or_default();
                            self.drag = Some(Drag::Move {
                                start,
                                bounds,
                                targets: Targets::collect(ed, &ids),
                                originals,
                            });
                        }
                    }
                    None => {
                        let base = if mods.shift {
                            ed.selection.clone()
                        } else {
                            Vec::new()
                        };
                        ed.selection = base.clone();
                        self.drag = Some(Drag::Marquee {
                            start,
                            current: start,
                            base,
                        });
                    }
                }
            }
            Tool::Rect | Tool::Ellipse | Tool::Frame => {
                let (parent, to_local) = Self::parent_at(ed, start);
                let local = to_local * start;
                let node = self.new_shape(ed, self.tool, local, 0.0, 0.0);
                let id = node.id.clone();
                if ed
                    .begin_gesture(&format!("Create {}", self.tool.label().to_lowercase()))
                    .is_err()
                {
                    return;
                }
                match ed.gesture_create(parent, node) {
                    Ok(()) => {
                        ed.selection = vec![id.clone()];
                        self.drag = Some(Drag::Create {
                            start,
                            id,
                            to_local,
                        });
                    }
                    Err(e) => {
                        ed.cancel_gesture();
                        ed.status = Some(e.to_string());
                    }
                }
            }
            // Text boxes are placed with a click.
            Tool::Text => {}
        }
    }

    fn update_drag(&mut self, ed: &mut Editor, page: Point, mods: egui::Modifiers) {
        let mut cmds = Vec::new();
        // Ctrl turns snapping off, the whole-pixel grid included.
        let snap_opts = SnapOptions::new(self.view.zoom as f64, !mods.command);
        let round = |v: f64| if snap_opts.enabled { v.round() } else { v };
        match &mut self.drag {
            Some(Drag::Marquee {
                start,
                current,
                base,
            }) => {
                *current = page;
                let r = kurbo::Rect::from_points(*start, *current);
                let mut sel = base.clone();
                for id in marquee_select(&ed.geometry, ed.page(), r) {
                    if !sel.contains(&id) {
                        sel.push(id);
                    }
                }
                ed.selection = sel;
            }
            Some(Drag::Move {
                start,
                bounds,
                targets,
                originals,
            }) => {
                let mut d = page - *start;
                let (mut xs, mut ys) = (Edges::ALL, Edges::ALL);
                if mods.shift {
                    // Constrain to the dominant axis.
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0;
                        ys = Edges::NONE;
                    } else {
                        d.x = 0.0;
                        xs = Edges::NONE;
                    }
                }
                let snapped = snap::snap(*bounds + d, xs, ys, &targets.all(), snap_opts);
                d += snapped.delta;
                self.guides = snapped.guides;
                self.measures = guides::distances(*bounds + d, &targets.siblings, targets.parent);
                for o in originals.iter() {
                    cmds.push(Command::SetGeometry {
                        id: o.id.clone(),
                        x: round(o.x as f64 + d.x) as f32,
                        y: round(o.y as f64 + d.y) as f32,
                        width: o.width,
                        height: o.height,
                        rotation: o.rotation,
                    });
                }
            }
            Some(Drag::Resize {
                handle,
                start,
                bounds,
                targets,
                originals,
                snapshots,
            }) => {
                let mut d = page - *start;
                // Snap the dragged edges, where they would be before any
                // flip or aspect correction.
                let edges = |f: f64| match f {
                    0.0 => Edges::MIN,
                    1.0 => Edges::MAX,
                    _ => Edges::NONE,
                };
                let dragged = kurbo::Rect::new(
                    bounds.x0 + if handle.fx == 0.0 { d.x } else { 0.0 },
                    bounds.y0 + if handle.fy == 0.0 { d.y } else { 0.0 },
                    bounds.x1 + if handle.fx == 1.0 { d.x } else { 0.0 },
                    bounds.y1 + if handle.fy == 1.0 { d.y } else { 0.0 },
                );
                let snapped = snap::snap(
                    dragged,
                    edges(handle.fx),
                    edges(handle.fy),
                    &targets.all(),
                    snap_opts,
                );
                d += snapped.delta;
                self.guides = snapped.guides;
                let nb = resize_bounds(*bounds, *handle, d, mods.shift, mods.alt);
                self.measures = guides::distances(nb, &targets.siblings, targets.parent);
                let sx = nb.width() / bounds.width().max(1e-6);
                let sy = nb.height() / bounds.height().max(1e-6);
                for o in originals.iter() {
                    let nx0 = nb.x0 + (o.bounds.x0 - bounds.x0) * sx;
                    let ny0 = nb.y0 + (o.bounds.y0 - bounds.y0) * sy;
                    let w = (o.width as f64 * sx).max(1.0);
                    let h = (o.height as f64 * sy).max(1.0);
                    // Bounds include rotation, so place the box's center
                    // where the scaled bounds put theirs.
                    let bw = o.bounds.width() * sx;
                    let bh = o.bounds.height() * sy;
                    let dx = nx0 - o.bounds.x0 + (bw - o.bounds.width()) / 2.0
                        - (w - o.width as f64) / 2.0;
                    let dy = ny0 - o.bounds.y0 + (bh - o.bounds.height()) / 2.0
                        - (h - o.height as f64) / 2.0;
                    cmds.push(Command::SetGeometry {
                        id: o.id.clone(),
                        x: round(o.x as f64 + dx) as f32,
                        y: round(o.y as f64 + dy) as f32,
                        width: round(w).max(1.0) as f32,
                        height: round(h).max(1.0) as f32,
                        rotation: o.rotation,
                    });
                }
                cmds.extend(auto_layout::resize_effects(ed, snapshots, &cmds));
                // Resizing auto-width text fixes its width (it then wraps).
                for o in originals.iter() {
                    if let Some(NodeKind::Text(t)) = ed.node(&o.id).map(|n| &n.kind) {
                        if t.sizing == TextSizing::AutoWidth {
                            cmds.push(Command::SetProps {
                                id: o.id.clone(),
                                patch: NodePatch {
                                    sizing: Some(TextSizing::Fixed),
                                    ..Default::default()
                                },
                            });
                        }
                    }
                }
            }
            Some(Drag::Rotate {
                center,
                start,
                originals,
                angle,
            }) => {
                let a0 = (*start - *center).atan2();
                let a1 = (page - *center).atan2();
                let mut theta = (a1 - a0).to_degrees();
                if mods.shift {
                    // A single layer lands on a multiple of 15°; several
                    // turn together by one.
                    theta = match originals.as_slice() {
                        [o] => snap_angle(o.rotation as f64 + theta) - o.rotation as f64,
                        _ => snap_angle(theta),
                    };
                }
                *angle = theta;
                let turn = Affine::rotate_about(theta.to_radians(), *center);
                for o in originals.iter() {
                    let local = Affine::translate((o.x as f64, o.y as f64))
                        * Affine::rotate_about((o.rotation as f64).to_radians(), o.pivot);
                    let c = turn * (o.parent_tf * local * o.pivot);
                    let q = o.parent_tf.inverse() * c;
                    cmds.push(Command::SetGeometry {
                        id: o.id.clone(),
                        x: round2(q.x - o.pivot.x) as f32,
                        y: round2(q.y - o.pivot.y) as f32,
                        width: o.width,
                        height: o.height,
                        rotation: round2(normalize_degrees(o.rotation as f64 + theta)) as f32,
                    });
                }
            }
            Some(Drag::Create {
                start,
                id,
                to_local,
                ..
            }) => {
                let mut end = page;
                let mut d = end - *start;
                if mods.shift {
                    let m = d.x.abs().max(d.y.abs());
                    d = kurbo::Vec2::new(m.copysign(d.x), m.copysign(d.y));
                    end = *start + d;
                }
                let r = if mods.alt {
                    kurbo::Rect::from_points(*start - d, end)
                } else {
                    kurbo::Rect::from_points(*start, end)
                };
                let a = *to_local * Point::new(r.x0, r.y0);
                cmds.push(Command::SetGeometry {
                    id: id.clone(),
                    x: a.x.round() as f32,
                    y: a.y.round() as f32,
                    width: r.width().round() as f32,
                    height: r.height().round() as f32,
                    rotation: 0.0,
                });
            }
            Some(Drag::Spacing(s)) => cmds.push(s.update(page, snap_opts.enabled)),
            Some(Drag::Reorder(r)) => r.update(ed, page),
            Some(Drag::Pan) | None => {}
        }
        for c in cmds {
            if let Err(e) = ed.gesture_apply(c) {
                ed.status = Some(e.to_string());
            }
        }
    }

    fn click(&mut self, ed: &mut Editor, origin: Pos2, p: Pos2, mods: egui::Modifiers) {
        let page = self.page_point(origin, p);
        match self.tool {
            Tool::Move => {
                let opts = HitOptions {
                    tolerance: 3.0 / self.view.zoom as f64,
                    selected: &ed.selection,
                    deep: mods.command,
                };
                match hit_test(&ed.geometry, ed.page(), page, opts) {
                    Some(hit) if mods.shift => {
                        if let Some(i) = ed.selection.iter().position(|s| s == &hit) {
                            ed.selection.remove(i);
                        } else {
                            ed.selection.push(hit);
                        }
                    }
                    Some(hit) => ed.selection = vec![hit],
                    None if !mods.shift => ed.selection.clear(),
                    None => {}
                }
            }
            Tool::Rect | Tool::Ellipse | Tool::Frame => {
                let (parent, to_local) = Self::parent_at(ed, page);
                let (w, h) = if self.tool == Tool::Frame {
                    (400.0, 300.0)
                } else {
                    (100.0, 100.0)
                };
                let local = to_local * page;
                let node = self.new_shape(ed, self.tool, local, w, h);
                let id = node.id.clone();
                let label = format!("Create {}", self.tool.label().to_lowercase());
                let _ = ed.commit(
                    &label,
                    vec![Command::CreateNode {
                        parent,
                        index: None,
                        node,
                    }],
                    vec![id],
                );
                self.tool = Tool::Move;
            }
            Tool::Text => {
                let (parent, to_local) = Self::parent_at(ed, page);
                let local = to_local * page;
                let id = Id::new(IdKind::Node);
                let node = Node::new(
                    id.clone(),
                    "Text",
                    NodeKind::Text(Text {
                        fills: vec![Paint::solid(ColorRef::literal("#000000"))],
                        ..Default::default()
                    }),
                )
                .with_geometry(
                    local.x.round() as f32,
                    local.y.round() as f32,
                    0.0,
                    0.0,
                );
                if ed.begin_gesture("Create text").is_err() {
                    return;
                }
                if let Err(e) = ed.gesture_create(parent, node) {
                    ed.cancel_gesture();
                    ed.status = Some(e.to_string());
                    return;
                }
                ed.selection = vec![id.clone()];
                self.editing = Some(TextEditing {
                    id,
                    buffer: String::new(),
                    is_new: true,
                    focused: false,
                });
                self.tool = Tool::Move;
            }
        }
    }

    fn double_click(&mut self, ed: &mut Editor, origin: Pos2, p: Pos2) {
        let page = self.page_point(origin, p);
        let opts = HitOptions {
            tolerance: 3.0 / self.view.zoom as f64,
            selected: &ed.selection,
            deep: false,
        };
        let Some(hit) = hit_test(&ed.geometry, ed.page(), page, opts) else {
            return;
        };
        let shown = ed.shown_node(&hit);
        let content = match shown.as_ref().map(|n| &n.kind) {
            Some(NodeKind::Text(t)) => Some(t.content.clone()),
            _ => None,
        };
        let is_group = matches!(shown.as_ref().map(|n| &n.kind), Some(NodeKind::Group(_)));
        let is_instance = matches!(shown.as_ref().map(|n| &n.kind), Some(NodeKind::Instance(_)));
        match content {
            Some(content) if !ed.read_only => {
                ed.selection = vec![hit.clone()];
                self.editing = Some(TextEditing {
                    id: hit,
                    buffer: content,
                    is_new: false,
                    focused: false,
                });
            }
            None if is_group => {
                // Enter the group: select the child under the pointer.
                let deep = HitOptions { deep: true, ..opts };
                if let Some(child) = hit_test(&ed.geometry, ed.page(), page, deep) {
                    ed.selection = vec![child];
                }
            }
            None if is_instance => {
                // Enter the instance: select the derived child under the
                // pointer, one level down.
                let entered = [hit.clone()];
                let inside = HitOptions {
                    selected: &entered,
                    ..opts
                };
                if let Some(child) = hit_test(&ed.geometry, ed.page(), page, inside) {
                    ed.selection = vec![child];
                }
            }
            _ => {}
        }
    }

    /// The innermost visible, unlocked frame under `p` (new shapes go in
    /// it), or the page (the master root while a component is edited),
    /// with the page → parent-local transform.
    pub(crate) fn parent_at(ed: &Editor, p: Point) -> (ParentRef, Affine) {
        fn find(ed: &Editor, nodes: &[Node], p: Point) -> Option<NodeId> {
            nodes.iter().rev().find_map(|n| {
                if !n.visible || n.locked {
                    return None;
                }
                let NodeKind::Frame(f) = &n.kind else {
                    return None;
                };
                let g = ed.geometry.get(&n.id)?;
                g.local_rect()
                    .contains(g.to_local(p))
                    .then(|| find(ed, &f.children, p).unwrap_or_else(|| n.id.clone()))
            })
        }
        match find(ed, &ed.page().children, p) {
            Some(id) => {
                let tf = ed
                    .geometry
                    .get(&id)
                    .map_or(Affine::IDENTITY, |g| g.transform.inverse());
                (ParentRef::Node(id), tf)
            }
            None => (ed.insert_parent(), Affine::IDENTITY),
        }
    }

    fn new_shape(&self, ed: &Editor, tool: Tool, at: Point, w: f32, h: f32) -> Node {
        let fill = |c: &str| Shape {
            fills: vec![Paint::solid(ColorRef::literal(c))],
            ..Default::default()
        };
        let (name, kind) = match tool {
            Tool::Frame => (
                "Frame",
                NodeKind::Frame(Frame {
                    shape: fill("#ffffff"),
                    ..Default::default()
                }),
            ),
            Tool::Ellipse => (
                "Ellipse",
                NodeKind::Ellipse(doc::Ellipse {
                    shape: fill("#d9d9d9"),
                    ..Default::default()
                }),
            ),
            _ => (
                "Rectangle",
                NodeKind::Rect(RectKind {
                    shape: fill("#d9d9d9"),
                    ..Default::default()
                }),
            ),
        };
        Node::new(Id::new(IdKind::Node), ed.next_name(name), kind).with_geometry(
            at.x.round() as f32,
            at.y.round() as f32,
            w,
            h,
        )
    }

    /// Create the vello renderer on the app's device, once.
    fn start_gpu(&mut self, rs: &egui_wgpu::RenderState) {
        if let Gpu::Uninit = self.gpu {
            self.gpu = match SceneRenderer::new(&rs.device) {
                Ok(renderer) => {
                    let renderer = Box::new(renderer);
                    if rs.adapter.get_info().backend != wgpu::Backend::Vulkan {
                        tracing::warn!(
                            "canvas is running on {:?}, not Vulkan; it may be slow",
                            rs.adapter.get_info().backend
                        );
                    }
                    Gpu::Ready {
                        renderer,
                        target: None,
                    }
                }
                Err(e) => {
                    tracing::error!("cannot start the vello renderer: {e}");
                    Gpu::Failed(e.to_string())
                }
            };
        }
    }

    /// Render `id` offscreen on the canvas's device and encode it as PNG
    /// (§7.6): what the canvas draws, without selection chrome.
    pub fn render_png(
        &mut self,
        frame: &eframe::Frame,
        ed: &Editor,
        id: &NodeId,
        opts: &export::PngOptions,
    ) -> Result<Vec<u8>, String> {
        let rs = frame
            .wgpu_render_state()
            .ok_or("offscreen rendering needs the wgpu renderer")?;
        self.start_gpu(rs);
        if !self.scenes.images().assets().same(&ed.assets) {
            self.scenes = SceneCache::with_images(ImageCache::new(ed.assets.clone()));
        }
        let Gpu::Ready { renderer, .. } = &mut self.gpu else {
            return Err(format!(
                "the renderer is unavailable: {}",
                self.gpu_error().unwrap_or("")
            ));
        };
        let mut gpu = export::GpuContext {
            device: &rs.device,
            queue: &rs.queue,
            renderer,
            images: self.scenes.images(),
        };
        export::export_png(&ed.doc, &ed.geometry, id, opts, &mut gpu).map_err(|e| e.to_string())
    }

    fn paint_document(&mut self, ui: &mut Ui, frame: &eframe::Frame, ed: &Editor, rect: Rect) {
        let painter = ui.painter_at(rect);
        let Some(rs) = frame.wgpu_render_state() else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "The canvas needs the wgpu renderer.",
                FontId::proportional(14.0),
                Color32::GRAY,
            );
            return;
        };
        self.start_gpu(rs);
        let Gpu::Ready { renderer, target } = &mut self.gpu else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!(
                    "Canvas renderer unavailable: {}",
                    self.gpu_error().unwrap_or("")
                ),
                FontId::proportional(14.0),
                Color32::LIGHT_RED,
            );
            return;
        };
        let ppp = ui.ctx().pixels_per_point();
        let w = ((rect.width() * ppp).round() as u32).max(1);
        let h = ((rect.height() * ppp).round() as u32).max(1);
        if target
            .as_ref()
            .is_none_or(|(t, _)| t.width != w || t.height != h)
        {
            let t = CanvasTarget::new(&rs.device, w, h);
            let mut egui_renderer = rs.renderer.write();
            let id = match target.take() {
                Some((_, id)) => {
                    egui_renderer.update_egui_texture_from_wgpu_texture(
                        &rs.device,
                        &t.view,
                        wgpu::FilterMode::Linear,
                        id,
                    );
                    id
                }
                None => egui_renderer.register_native_texture(
                    &rs.device,
                    &t.view,
                    wgpu::FilterMode::Linear,
                ),
            };
            *target = Some((t, id));
            self.last_render = None;
        }
        let Some((t, texture_id)) = target.as_ref() else {
            return;
        };
        let hidden = self.editing.as_ref().map(|e| e.id.clone());
        let key = RenderKey {
            revision: ed.revision,
            view: self.view,
            size: (w, h),
            pixels_per_point: ppp,
            hidden: hidden.clone(),
        };
        if self.last_render.as_ref() != Some(&key) {
            let page = ed.page();
            let viewport = kurbo::Rect::from_points(
                self.view.to_page(rect.min, rect.min),
                self.view.to_page(rect.min, rect.max),
            );
            let scene = self.scenes.build(
                &ed.doc,
                &ed.geometry,
                page,
                self.view.affine(ppp),
                viewport,
                &SceneOptions { hidden },
            );
            let bg = render::parse_hex(&page.background.0).unwrap_or(render::peniko::Color::WHITE);
            if let Err(e) = renderer.render(&rs.device, &rs.queue, &scene, t, bg) {
                tracing::error!("canvas render failed: {e}");
                self.gpu = Gpu::Failed(e.to_string());
                return;
            }
            self.last_render = Some(key);
        }
        painter.image(
            *texture_id,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    fn paint_chrome(&self, ui: &Ui, ed: &Editor, rect: Rect) {
        let painter = ui.painter_at(rect);
        let origin = rect.min;
        let outline = |id: &NodeId, width: f32| {
            if let Some(g) = ed.geometry.get(id) {
                let r = g.local_rect();
                let pts: Vec<Pos2> = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
                    .into_iter()
                    .map(|(x, y)| self.view.to_screen(origin, g.transform * Point::new(x, y)))
                    .collect();
                painter.add(egui::Shape::closed_line(pts, Stroke::new(width, SELECTION)));
            }
        };
        if let Some(h) = &self.hover {
            if !ed.selection.contains(h) {
                outline(h, 1.5);
            }
        }
        for id in &ed.selection {
            outline(id, 1.0);
        }
        self.paint_auto_layout(&painter, ed, origin);
        if let Some(b) = self.selection_bounds(ed) {
            let sr = self.view.rect_to_screen(origin, b);
            if ed.selection.len() > 1 {
                painter.rect_stroke(
                    sr,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, SELECTION),
                    StrokeKind::Middle,
                );
            }
            if !ed.read_only && self.editing.is_none() && self.tool == Tool::Move {
                for h in HANDLES {
                    let c = Pos2::new(
                        sr.min.x + sr.width() * h.fx as f32,
                        sr.min.y + sr.height() * h.fy as f32,
                    );
                    let hr = Rect::from_center_size(c, Vec2::splat(HANDLE));
                    painter.rect_filled(hr, CornerRadius::ZERO, Color32::WHITE);
                    painter.rect_stroke(
                        hr,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, SELECTION),
                        StrokeKind::Inside,
                    );
                }
            }
            let label = format!("{} × {}", fmt_num(b.width()), fmt_num(b.height()));
            let galley = painter.layout_no_wrap(label, FontId::proportional(11.0), Color32::WHITE);
            let pos = Pos2::new(sr.center().x - galley.size().x / 2.0, sr.max.y + 8.0);
            let bg = Rect::from_min_size(pos, galley.size()).expand2(Vec2::new(4.0, 2.0));
            painter.rect_filled(bg, CornerRadius::same(3), SELECTION);
            painter.galley(pos, galley, Color32::WHITE);
        }
        match &self.drag {
            Some(Drag::Move { .. } | Drag::Resize { .. }) => {
                guides::paint(&painter, &self.view, origin, &self.guides, &self.measures);
            }
            Some(Drag::Rotate {
                angle, originals, ..
            }) => {
                if let (Some(p), [o]) = (self.rotate_cursor, originals.as_slice()) {
                    let total = normalize_degrees(o.rotation as f64 + angle);
                    guides::paint_angle(&painter, p, total);
                }
            }
            _ => {}
        }
        if let Some(p) = self.rotate_cursor {
            guides::paint_rotate_cursor(&painter, p);
        }
        if let Some(Drag::Marquee { start, current, .. }) = &self.drag {
            let r = Rect::from_two_pos(
                self.view.to_screen(origin, *start),
                self.view.to_screen(origin, *current),
            );
            painter.rect_filled(r, CornerRadius::ZERO, SELECTION.gamma_multiply(0.12));
            painter.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.0, SELECTION),
                StrokeKind::Inside,
            );
        }
    }

    /// Padding and gap of the auto-layout frame under the pointer, with
    /// handles when it is selected, and the insertion line of a reorder.
    fn paint_auto_layout(&self, painter: &egui::Painter, ed: &Editor, origin: Pos2) {
        if let Some(Drag::Reorder(r)) = &self.drag {
            layout_handles::paint_reorder(painter, &self.view, origin, r);
            return;
        }
        if self.tool != Tool::Move || self.editing.is_some() {
            return;
        }
        let over = |id: &NodeId| {
            self.cursor
                .zip(ed.geometry.get(id))
                .is_some_and(|(p, g)| g.local_rect().contains(g.to_local(p)))
        };
        let selected = ed.selected_auto_layout().map(|n| n.id.clone());
        match (&self.drag, &selected) {
            (Some(Drag::Spacing(_)), Some(id)) => {
                layout_handles::paint(painter, &self.view, origin, ed, id, true)
            }
            (None, Some(id)) if over(id) => {
                layout_handles::paint(painter, &self.view, origin, ed, id, !ed.read_only)
            }
            (None, _) => {
                let hovered = self
                    .hover
                    .as_ref()
                    .filter(|h| ed.node(h).and_then(auto_layout::auto_layout).is_some());
                if let Some(h) = hovered {
                    layout_handles::paint(painter, &self.view, origin, ed, h, false);
                }
            }
            _ => {}
        }
    }

    /// The inline text editor: an egui `TextEdit` over the node, at its
    /// position and scale; the node itself is hidden from the scene.
    fn text_editor(&mut self, ui: &mut Ui, ed: &mut Editor, origin: Pos2, rect: Rect) {
        let Some(te) = self.editing.as_mut() else {
            return;
        };
        let node = ed.shown_node(&te.id);
        let (Some(node), Some(g)) = (node.as_ref(), ed.geometry.get(&te.id)) else {
            self.editing = None;
            ed.cancel_gesture();
            return;
        };
        let NodeKind::Text(t) = &node.kind else {
            self.editing = None;
            return;
        };
        let style = layout::effective_text_style(&ed.doc, node).expect("text node");
        let size = style.font_size * self.view.zoom;
        let color = t
            .fills
            .iter()
            .find_map(|p| match p {
                Paint::Solid(s) => Some(render::resolve_color(&ed.doc, &s.color)),
                _ => None,
            })
            .map(|c| {
                let [r, g, b, a] = c.to_rgba8().to_u8_array();
                Color32::from_rgba_unmultiplied(r, g, b, a)
            })
            .unwrap_or(Color32::BLACK);
        let top_left = self.view.to_screen(origin, g.origin());
        let width = match t.sizing {
            TextSizing::AutoWidth => (rect.right() - top_left.x).max(40.0),
            _ => (g.size.width as f32 * self.view.zoom).max(10.0),
        };
        let height = (g.size.height as f32 * self.view.zoom).max(size * 1.4);
        let area = Rect::from_min_size(top_left, Vec2::new(width, height));
        let align = match t.align {
            doc::TextAlign::Center => egui::Align::Center,
            doc::TextAlign::Right => egui::Align::Max,
            _ => egui::Align::Min,
        };
        let edit = TextEdit::multiline(&mut te.buffer)
            .font(FontId::proportional(size))
            .text_color(color)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_rows(1)
            .desired_width(width)
            .horizontal_align(align);
        let out = ui.put(area, edit);
        if !te.focused {
            out.request_focus();
            te.focused = true;
        } else if out.lost_focus() {
            self.finish_text_edit(ed);
            return;
        }
        let r = out.rect;
        ui.painter_at(rect).rect_stroke(
            r.expand(1.0),
            CornerRadius::ZERO,
            Stroke::new(1.0, SELECTION),
            StrokeKind::Outside,
        );
    }
}

/// New text layers are named after their first line, like other design tools.
fn text_layer_name(content: &str) -> String {
    let first = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Text")
        .trim();
    let mut name: String = first.chars().take(40).collect();
    if first.chars().count() > 40 {
        name.push('…');
    }
    name
}

/// The transform from `id`'s parent-local coordinates to the page, as
/// layout builds it (a group's geometry is its bounds, not this).
fn parent_transform(ed: &Editor, id: &NodeId) -> Affine {
    let Some(parent) = ed.geometry.get(id).and_then(|g| g.parent.clone()) else {
        return Affine::IDENTITY;
    };
    match ed.node(&parent) {
        Some(
            p @ Node {
                kind: NodeKind::Group(_),
                ..
            },
        ) => {
            let center = Point::new(
                p.width.max(0.0) as f64 / 2.0,
                p.height.max(0.0) as f64 / 2.0,
            );
            parent_transform(ed, &parent)
                * Affine::translate((p.x as f64, p.y as f64))
                * Affine::rotate_about((p.rotation as f64).to_radians(), center)
        }
        _ => ed
            .geometry
            .get(&parent)
            .map_or(Affine::IDENTITY, |g| g.transform),
    }
}

/// The nearest multiple of 15°.
fn snap_angle(deg: f64) -> f64 {
    (deg / 15.0).round() * 15.0
}

/// Degrees in `(-180, 180]`.
pub(crate) fn normalize_degrees(deg: f64) -> f64 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn fmt_num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Selection bounds after dragging `handle` by `d`. `keep_aspect` (Shift)
/// keeps the width/height ratio; `from_center` (Alt) resizes symmetrically.
fn resize_bounds(
    b: kurbo::Rect,
    handle: Handle,
    d: kurbo::Vec2,
    keep_aspect: bool,
    from_center: bool,
) -> kurbo::Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (b.x0, b.y0, b.x1, b.y1);
    match handle.fx {
        0.0 => x0 += d.x,
        1.0 => x1 += d.x,
        _ => {}
    }
    match handle.fy {
        0.0 => y0 += d.y,
        1.0 => y1 += d.y,
        _ => {}
    }
    if from_center {
        if handle.fx == 0.0 {
            x1 -= x0 - b.x0;
        } else if handle.fx == 1.0 {
            x0 -= x1 - b.x1;
        }
        if handle.fy == 0.0 {
            y1 -= y0 - b.y0;
        } else if handle.fy == 1.0 {
            y0 -= y1 - b.y1;
        }
    }
    let mut r = kurbo::Rect::new(x0, y0, x1, y1).abs();
    if keep_aspect && b.width() > 0.0 && b.height() > 0.0 {
        let ratio = b.width() / b.height();
        let (mut w, mut h) = (r.width(), r.height());
        if handle.fx == 0.5 {
            w = h * ratio;
        } else if handle.fy == 0.5 || w / ratio > h {
            h = w / ratio;
        } else {
            w = h * ratio;
        }
        // Anchor on the side opposite the handle (or the center).
        let ax = if from_center || handle.fx == 0.5 {
            r.center().x - w / 2.0
        } else if handle.fx == 0.0 {
            r.x1 - w
        } else {
            r.x0
        };
        let ay = if from_center || handle.fy == 0.5 {
            r.center().y - h / 2.0
        } else if handle.fy == 0.0 {
            r.y1 - h
        } else {
            r.y0
        };
        r = kurbo::Rect::new(ax, ay, ax + w, ay + h);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_round_trips_and_zooms_around_the_anchor() {
        let mut v = View::default();
        let origin = Pos2::new(100.0, 50.0);
        let p = Point::new(10.0, 20.0);
        let s = v.to_screen(origin, p);
        assert_eq!(v.to_page(origin, s), p);
        let anchor = Pos2::new(300.0, 200.0);
        let before = v.to_page(origin, anchor);
        v.zoom_around(origin, anchor, 2.5);
        assert_eq!(v.zoom, 2.5);
        let after = v.to_page(origin, anchor);
        assert!((before - after).hypot() < 1e-3);
        v.zoom_around(origin, anchor, 1e9);
        assert_eq!(v.zoom, MAX_ZOOM);
    }

    #[test]
    fn fit_centers_bounds() {
        let mut v = View::default();
        v.fit(
            Vec2::new(1000.0, 500.0),
            kurbo::Rect::new(0.0, 0.0, 100.0, 100.0),
            1.0,
        );
        assert_eq!(v.zoom, 1.0);
        let c = v.to_screen(Pos2::ZERO, Point::new(50.0, 50.0));
        assert_eq!(c, Pos2::new(500.0, 250.0));
        v.fit(
            Vec2::new(1000.0, 500.0),
            kurbo::Rect::new(0.0, 0.0, 10000.0, 100.0),
            1.0,
        );
        assert!((v.zoom - 0.09).abs() < 1e-6);
    }

    #[test]
    fn resize_from_corners_edges_and_center() {
        let b = kurbo::Rect::new(0.0, 0.0, 100.0, 50.0);
        let se = Handle { fx: 1.0, fy: 1.0 };
        let r = resize_bounds(b, se, kurbo::Vec2::new(10.0, 20.0), false, false);
        assert_eq!(r, kurbo::Rect::new(0.0, 0.0, 110.0, 70.0));
        let nw = Handle { fx: 0.0, fy: 0.0 };
        let r = resize_bounds(b, nw, kurbo::Vec2::new(10.0, 10.0), false, false);
        assert_eq!(r, kurbo::Rect::new(10.0, 10.0, 100.0, 50.0));
        // Dragging past the opposite edge flips instead of going negative.
        let e = Handle { fx: 1.0, fy: 0.5 };
        let r = resize_bounds(b, e, kurbo::Vec2::new(-150.0, 0.0), false, false);
        assert_eq!(r, kurbo::Rect::new(-50.0, 0.0, 0.0, 50.0));
        // Alt: symmetric about the center.
        let r = resize_bounds(b, e, kurbo::Vec2::new(10.0, 0.0), false, true);
        assert_eq!(r, kurbo::Rect::new(-10.0, 0.0, 110.0, 50.0));
        // Shift: keep 2:1.
        let r = resize_bounds(b, se, kurbo::Vec2::new(100.0, 0.0), true, false);
        assert_eq!(r, kurbo::Rect::new(0.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn text_layer_names() {
        assert_eq!(text_layer_name("\n  Hello\nworld"), "Hello");
        assert_eq!(text_layer_name(&"x".repeat(50)).chars().count(), 41);
        assert_eq!(fmt_num(12.0), "12");
        assert_eq!(fmt_num(12.345), "12.35");
    }
}
