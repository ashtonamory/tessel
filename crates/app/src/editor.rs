//! The open document and everything that edits it: history, selection,
//! gestures, file I/O and computed geometry. No UI code, so it is testable
//! without a window. Every change goes through `doc` commands (§5).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use doc::assets::assets_dir_for;
use doc::{
    Assets, Command, CommandError, Document, History, Id, IdKind, Node, NodeId, NodeKind,
    NodePatch, Origin, Page, PageId, ParentRef, Problem, Selection, Transaction,
};
use layout::{Geometry, LayoutEngine};

use crate::activity::{layers_label, refine_gesture_label, Clock, NudgeState};

/// What the scene cache must drop after a change.
#[derive(Debug, Default, PartialEq)]
pub enum SceneDirty {
    #[default]
    Clean,
    Nodes(BTreeSet<NodeId>),
    All,
}

impl SceneDirty {
    fn add(&mut self, ids: BTreeSet<NodeId>) {
        match self {
            SceneDirty::Clean => *self = SceneDirty::Nodes(ids),
            SceneDirty::Nodes(set) => set.extend(ids),
            SceneDirty::All => {}
        }
    }
}

/// A drag or inline edit applied live and recorded as one transaction when
/// it ends (§5.3, "gestures coalesce").
pub(crate) struct Gesture {
    label: String,
    selection_before: Selection,
    forward: Vec<Command>,
    inverse: Vec<Command>,
    /// A node the gesture created; the transaction then records its final
    /// state as one `CreateNode`.
    created: Option<(ParentRef, usize, NodeId)>,
}

pub struct Editor {
    pub doc: Document,
    pub history: History,
    pub selection: Vec<NodeId>,
    /// The page being edited (see `pages.rs`); the first page when `None` or
    /// when it no longer exists.
    pub current_page: Option<PageId>,
    pub path: Option<PathBuf>,
    /// Problems found when loading; a document with problems opens
    /// read-only (§3.9).
    pub problems: Vec<Problem>,
    pub read_only: bool,
    pub geometry: Geometry,
    pub layout: LayoutEngine,
    /// Bumped on every document change.
    pub revision: u64,
    saved_revision: u64,
    pub scene_dirty: SceneDirty,
    /// Tokens and styles as of the last change: when they differ, any node
    /// may draw differently, so every scene is dropped (§3.6).
    look: (doc::Tokens, Vec<doc::Style>),
    /// Image bytes of the document (§3.8), written next to it on save.
    pub assets: Assets,
    gesture: Option<Gesture>,
    /// Time source for history stamps and nudge merging (injectable).
    pub clock: Clock,
    nudge: Option<NudgeState>,
    /// The last error or notice, for the status bar.
    pub status: Option<String>,
    /// The component being edited on the Components page, if any (see
    /// `master_edit.rs`). While set, `doc` is a view holding just the
    /// master and the document itself lives here.
    pub(crate) master: Option<crate::master_edit::MasterEdit>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    /// A new untitled document with one empty page.
    pub fn new() -> Editor {
        let page = Page::new(Id::new(IdKind::Page), "Page 1");
        let mut doc = Document::new("Untitled", page);
        doc.meta.created_with = Some(created_with());
        Editor::with_document(doc, None, Vec::new())
    }

    fn with_document(doc: Document, path: Option<PathBuf>, problems: Vec<Problem>) -> Editor {
        let mut layout = LayoutEngine::new();
        let geometry = layout.compute(&doc);
        let look = (doc.tokens.clone(), doc.styles.clone());
        Editor {
            read_only: !problems.is_empty(),
            doc,
            history: History::default(),
            selection: Vec::new(),
            current_page: None,
            path,
            problems,
            geometry,
            layout,
            revision: 0,
            saved_revision: 0,
            scene_dirty: SceneDirty::All,
            look,
            assets: Assets::new(),
            gesture: None,
            clock: Clock::system(),
            nudge: None,
            status: None,
            master: None,
        }
    }

    pub fn open(path: &Path) -> anyhow::Result<Editor> {
        let loaded = doc::load_file(path)?;
        let mut ed = Editor::with_document(loaded.doc, Some(path.to_path_buf()), loaded.problems);
        ed.assets.set_dir(Some(assets_dir_for(path)));
        if ed.doc.pages.is_empty() {
            // M1 works on one page; give a page-less file one to draw on.
            let page = Page::new(Id::new(IdKind::Page), "Page 1");
            ed.commit(
                "Add page",
                vec![Command::CreatePage { index: None, page }],
                Vec::new(),
            )?;
            ed.history.clear();
            ed.saved_revision = ed.revision;
        }
        Ok(ed)
    }

    /// Save to `path` (canonical form, atomic) and remember it.
    pub fn save_as(&mut self, path: &Path) -> anyhow::Result<()> {
        self.end_gesture();
        // Assets first: a document that references a missing asset is worse
        // than an asset nobody references yet.
        self.assets.save(&assets_dir_for(path), self.document())?;
        doc::save_file(path, self.document())?;
        self.path = Some(path.to_path_buf());
        self.saved_revision = self.revision;
        Ok(())
    }

    /// Mark the document as having unsaved changes without editing it (a
    /// restored recovery file is unsaved work).
    pub fn mark_dirty(&mut self) {
        self.revision += 1;
        self.scene_dirty = SceneDirty::All;
    }

    /// Take over `fresh` (the same file, re-read from disk) in place:
    /// selection and page are kept where they still exist, and the document
    /// counts as saved. History starts over.
    pub fn replace_with_reloaded(&mut self, fresh: Editor) {
        self.end_gesture();
        let selection = std::mem::take(&mut self.selection);
        let page = self.current_page.take();
        let revision = self.revision + 1;
        let clock = self.clock.clone();
        *self = fresh;
        self.clock = clock;
        self.revision = revision;
        self.saved_revision = revision;
        self.selection = selection
            .into_iter()
            .filter(|id| self.doc.node(id).is_some())
            .collect();
        self.current_page = page.filter(|id| self.doc.page(id).is_some());
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// File name for the window title.
    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// The page being edited.
    pub fn page(&self) -> &Page {
        self.current_page
            .as_ref()
            .and_then(|id| self.doc.page(id))
            .unwrap_or(&self.doc.pages[0])
    }

    /// Index of `page()` in `doc.pages`.
    pub fn current_page_index(&self) -> usize {
        let id = &self.page().id;
        self.doc.page_index(id).unwrap_or(0)
    }

    pub fn page_ref(&self) -> ParentRef {
        ParentRef::Page(self.page().id.clone())
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.doc.node(id)
    }

    pub(crate) fn changed(&mut self, touched: BTreeSet<NodeId>) {
        self.revision += 1;
        self.geometry = self.layout.update(&self.doc, &touched);
        self.scene_dirty.add(touched);
        if self.look.0 != self.doc.tokens || self.look.1 != self.doc.styles {
            self.look = (self.doc.tokens.clone(), self.doc.styles.clone());
            self.scene_dirty = SceneDirty::All;
        }
        // Drop selected nodes that no longer exist (or, inside instances,
        // are no longer shown).
        let (doc, geometry) = (&self.doc, &self.geometry);
        self.selection
            .retain(|id| doc.node(id).is_some() || (id.is_derived() && geometry.get(id).is_some()));
    }

    /// `SetGeometry`s that store the sizes layout computed (§3.5: the file
    /// shows the size the user sees), for the transaction being committed.
    pub(crate) fn size_writes(&self) -> Vec<Command> {
        layout::size_writeback(&self.doc, &self.geometry)
            .into_iter()
            .filter_map(|w| {
                let n = self.doc.node(&w.id)?;
                Some(Command::SetGeometry {
                    id: w.id,
                    x: n.x,
                    y: n.y,
                    width: w.width,
                    height: w.height,
                    rotation: n.rotation,
                })
            })
            .collect()
    }

    /// Bring geometry up to date after a change that does not move anything
    /// (a size write-back), without counting a new revision.
    pub(crate) fn relayout(&mut self, touched: &BTreeSet<NodeId>) {
        self.geometry = self.layout.update(&self.doc, touched);
        self.scene_dirty.add(touched.clone());
    }

    pub(crate) fn touched(cmds: &[Command]) -> BTreeSet<NodeId> {
        cmds.iter().flat_map(|c| c.touched_ids()).collect()
    }

    pub(crate) fn check_writable(&mut self) -> Result<(), CommandError> {
        if self.read_only {
            let e = CommandError::InvalidPatch(
                "the document is read-only because it has problems".into(),
            );
            self.status = Some(e.to_string());
            return Err(e);
        }
        Ok(())
    }

    /// Apply `commands` as one undoable transaction.
    pub fn commit(
        &mut self,
        label: &str,
        commands: Vec<Command>,
        selection_after: Vec<NodeId>,
    ) -> Result<(), CommandError> {
        self.end_gesture();
        self.check_writable()?;
        if self.master.is_some() {
            return self.master_commit(label, commands, selection_after);
        }
        self.apply_transaction(label, Origin::User, commands, selection_after)
    }

    /// Apply an agent's edit as one undoable transaction labelled with its
    /// client (§5.3). Unlike [`Editor::commit`] this leaves a user gesture
    /// on other nodes running; the caller checks for conflicts (§5.4).
    pub fn commit_agent(
        &mut self,
        label: &str,
        client: &str,
        commands: Vec<Command>,
        select: Option<Vec<NodeId>>,
    ) -> Result<(), CommandError> {
        // Agents edit the document, not the component view.
        self.leave_component();
        let after = select.unwrap_or_else(|| self.selection.clone());
        let origin = Origin::Agent {
            client: client.to_string(),
        };
        self.apply_transaction(label, origin, commands, after)
    }

    fn apply_transaction(
        &mut self,
        label: &str,
        origin: Origin,
        commands: Vec<Command>,
        selection_after: Vec<NodeId>,
    ) -> Result<(), CommandError> {
        let before = Selection {
            ids: self.selection.clone(),
        };
        let after = Selection {
            ids: selection_after.clone(),
        };
        match self
            .history
            .commit(&mut self.doc, label, origin, commands, before, after)
        {
            Ok(tx) => {
                let touched = Self::touched(&tx.forward)
                    .into_iter()
                    .chain(Self::touched(&tx.inverse))
                    .collect();
                if let Some(last) = self.history.last_mut() {
                    last.at = self.clock.now();
                }
                self.selection = selection_after;
                self.changed(touched);
                let writes = self.size_writes();
                if !writes.is_empty() {
                    let after = Selection {
                        ids: self.selection.clone(),
                    };
                    let touched = Self::touched(&writes);
                    match self.history.amend_last(&mut self.doc, writes, after) {
                        Ok(_) => self.relayout(&touched),
                        Err(e) => tracing::error!("{label}: size write-back: {e}"),
                    }
                }
                Ok(())
            }
            Err(e) => {
                tracing::warn!("{label}: {e}");
                self.status = Some(format!("{label}: {e}"));
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) {
        self.history_step(true);
    }

    pub fn redo(&mut self) {
        self.history_step(false);
    }

    fn history_step(&mut self, undo: bool) {
        self.end_gesture();
        let doc = match self.master.as_mut() {
            Some(m) => m.document_mut(),
            None => &mut self.doc,
        };
        let step = if undo {
            self.history.undo(doc)
        } else {
            self.history.redo(doc)
        };
        match step {
            Ok(Some(tx)) => {
                let touched = Self::touched(&tx.forward)
                    .into_iter()
                    .chain(Self::touched(&tx.inverse))
                    .collect();
                let selection = if undo {
                    tx.selection_before.ids.clone()
                } else {
                    tx.selection_after.ids.clone()
                };
                match self.master.as_ref().map(|m| m.owns(tx)) {
                    Some(owns) => self.master_history_step(owns, selection),
                    None => {
                        self.selection = selection;
                        self.changed(touched);
                    }
                }
            }
            Ok(None) => {}
            Err(e) if undo => self.status = Some(format!("Undo failed: {e}")),
            Err(e) => self.status = Some(format!("Redo failed: {e}")),
        }
    }

    /// Insert encoded image bytes as a new Image node at its natural size,
    /// scaled down to fit and centered in `view` (page coordinates). The
    /// bytes are stored as an asset (§3.8); the node is created by one
    /// undoable command and selected.
    pub fn insert_image(
        &mut self,
        bytes: Vec<u8>,
        ext: &str,
        name: &str,
        view: kurbo::Rect,
    ) -> Result<NodeId, String> {
        let (w, h) = render::natural_size(&bytes)
            .filter(|(w, h)| *w > 0 && *h > 0)
            .ok_or_else(|| "That file is not a supported image".to_string())?;
        let (w, h) = (w as f32, h as f32);
        let scale = (view.width() as f32 / w)
            .min(view.height() as f32 / h)
            .min(1.0);
        let (w, h) = ((w * scale).round().max(1.0), (h * scale).round().max(1.0));
        let x = (view.center().x as f32 - w / 2.0).round();
        let y = (view.center().y as f32 - h / 2.0).round();
        let asset = self.assets.add(bytes, ext);
        let id = Id::new(IdKind::Node);
        let node = Node::new(
            id.clone(),
            name,
            NodeKind::Image(doc::Image {
                asset,
                fit: doc::ImageFit::Fill,
                corner_radius: Default::default(),
                extra: Default::default(),
            }),
        )
        .with_geometry(x, y, w, h);
        let cmd = Command::CreateNode {
            parent: self.insert_parent(),
            index: None,
            node,
        };
        self.commit("Insert image", vec![cmd], vec![id.clone()])
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    // --- Gestures ---------------------------------------------------------

    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// The nodes the gesture in progress works on: those it has changed or
    /// created, and the selection it started with.
    pub fn gesture_nodes(&self) -> BTreeSet<NodeId> {
        let Some(g) = &self.gesture else {
            return BTreeSet::new();
        };
        let mut out = Self::touched(&g.forward);
        out.extend(g.selection_before.ids.iter().cloned());
        out.extend(g.created.iter().map(|c| c.2.clone()));
        out
    }

    /// Start a gesture; ends any gesture in progress first.
    pub fn begin_gesture(&mut self, label: &str) -> Result<(), CommandError> {
        self.end_gesture();
        self.check_writable()?;
        self.gesture = Some(Gesture {
            label: refine_gesture_label(label, self.selection.len()),
            selection_before: Selection {
                ids: self.selection.clone(),
            },
            forward: Vec::new(),
            inverse: Vec::new(),
            created: None,
        });
        Ok(())
    }

    /// Apply a command live as part of the current gesture. Repeated
    /// `SetGeometry`s of one node coalesce into one, and so do repeated
    /// `SetProps`s of one node that set the same fields.
    pub fn gesture_apply(&mut self, cmd: Command) -> Result<(), CommandError> {
        let Some(g) = self.gesture.as_mut() else {
            return Err(CommandError::InvalidPatch("no gesture in progress".into()));
        };
        let touched = cmd.touched_ids();
        // Only the node's last command may be replaced: a later one could
        // overlap it.
        let last = |id: &NodeId| {
            g.forward
                .iter()
                .rposition(|c| c.touched_ids().contains(id))
                .map(|i| (i, &g.forward[i]))
        };
        let slot = match &cmd {
            Command::SetGeometry { id, .. } => match last(id) {
                Some((i, Command::SetGeometry { .. })) => Some(i),
                _ => None,
            },
            Command::SetProps { id, patch } => match last(id) {
                Some((i, Command::SetProps { patch: p, .. }))
                    if patch_fields(p) == patch_fields(patch) =>
                {
                    Some(i)
                }
                _ => None,
            },
            _ => None,
        };
        let inverse = cmd.clone().apply(&mut self.doc)?;
        match slot {
            Some(i) => g.forward[i] = cmd,
            None => {
                g.forward.push(cmd);
                g.inverse.push(inverse);
            }
        }
        self.changed(touched);
        Ok(())
    }

    /// Create a node live as part of the current gesture.
    pub fn gesture_create(&mut self, parent: ParentRef, node: Node) -> Result<(), CommandError> {
        let id = node.id.clone();
        let index = self
            .doc
            .children_of(&parent)
            .map(|c| c.len())
            .ok_or_else(|| CommandError::NotFound(format!("{parent:?}")))?;
        self.gesture_apply(Command::CreateNode {
            parent: parent.clone(),
            index: Some(index),
            node,
        })?;
        if let Some(g) = self.gesture.as_mut() {
            g.created = Some((parent, index, id));
        }
        Ok(())
    }

    /// End the gesture and record it as one transaction, unless it changed
    /// nothing.
    pub fn end_gesture(&mut self) {
        let Some(g) = self.gesture.take() else {
            return;
        };
        // Store computed sizes in the same transaction (§3.5).
        let mut writes = Vec::new();
        for w in self.size_writes() {
            match w.clone().apply(&mut self.doc) {
                Ok(inv) => writes.push((w, inv)),
                Err(e) => tracing::error!("{}: size write-back: {e}", g.label),
            }
        }
        if !writes.is_empty() {
            let touched = Self::touched(&writes.iter().map(|w| w.0.clone()).collect::<Vec<_>>());
            self.relayout(&touched);
        }
        let (mut forward, mut inverse) = match &g.created {
            Some((parent, index, id)) => match self.doc.node(id) {
                Some(node) => (
                    vec![Command::CreateNode {
                        parent: parent.clone(),
                        index: Some(*index),
                        node: node.clone(),
                    }],
                    vec![Command::DeleteNode { id: id.clone() }],
                ),
                None => return,
            },
            None => {
                let mut f = Vec::new();
                let mut i = Vec::new();
                for (fwd, inv) in g.forward.into_iter().zip(g.inverse) {
                    if fwd != inv {
                        f.push(fwd);
                        i.push(inv);
                    }
                }
                (f, i)
            }
        };
        // The created node's final state already holds its own size.
        let created = g.created.as_ref().map(|c| &c.2);
        for (fwd, inv) in writes {
            if !matches!(&fwd, Command::SetGeometry { id, .. } if Some(id) == created) {
                forward.push(fwd);
                inverse.push(inv);
            }
        }
        if forward.is_empty() {
            return;
        }
        if self.master.is_some() {
            if let Err(e) = self.master_record(&g.label, inverse, g.selection_before, false) {
                self.status = Some(format!("{}: {e}", g.label));
            }
            return;
        }
        self.history.record(Transaction {
            label: g.label,
            origin: Origin::User,
            forward,
            inverse,
            selection_before: g.selection_before,
            selection_after: Selection {
                ids: self.selection.clone(),
            },
            at: self.clock.now(),
        });
    }

    /// Undo everything the gesture applied and forget it.
    pub fn cancel_gesture(&mut self) {
        let Some(g) = self.gesture.take() else {
            return;
        };
        let touched = Self::touched(&g.forward);
        for inv in g.inverse.into_iter().rev() {
            if let Err(e) = inv.apply(&mut self.doc) {
                tracing::error!("cancelling gesture: {e}");
            }
        }
        self.selection = g.selection_before.ids;
        self.changed(touched);
    }

    // --- Edits ------------------------------------------------------------

    /// Delete the selected nodes (skipping those inside other selected nodes).
    pub fn delete_selection(&mut self) {
        let ids: Vec<NodeId> = self
            .top_selected()
            .into_iter()
            .filter(|id| !id.is_derived())
            .collect();
        if ids.is_empty() {
            if self.selection.iter().any(|id| id.is_derived()) {
                self.status =
                    Some("Layers inside an instance cannot be deleted; hide them instead".into());
            }
            return;
        }
        let label = if ids.len() == 1 {
            "Delete layer".to_string()
        } else {
            format!("Delete {} layers", ids.len())
        };
        let cmds = ids
            .into_iter()
            .map(|id| Command::DeleteNode { id })
            .collect();
        let _ = self.commit(&label, vec![Command::Batch(cmds)], Vec::new());
    }

    /// Selected nodes that are not inside another selected node.
    pub fn top_selected(&self) -> Vec<NodeId> {
        self.selection
            .iter()
            .filter(|id| {
                !layout::ancestors(&self.doc, id)
                    .iter()
                    .any(|a| self.selection.contains(a))
            })
            .cloned()
            .collect()
    }

    pub fn rename(&mut self, id: &NodeId, name: String) {
        if self.node(id).is_none_or(|n| n.name == name) {
            return;
        }
        let patch = NodePatch {
            name: Some(name),
            ..Default::default()
        };
        let sel = self.selection.clone();
        let _ = self.commit(
            "Rename layer",
            vec![Command::SetProps {
                id: id.clone(),
                patch,
            }],
            sel,
        );
    }

    pub fn set_visible(&mut self, id: &NodeId, visible: bool) {
        let patch = NodePatch {
            visible: Some(visible),
            ..Default::default()
        };
        let sel = self.selection.clone();
        let label = if visible { "Show layer" } else { "Hide layer" };
        let _ = self.commit(
            label,
            vec![Command::SetProps {
                id: id.clone(),
                patch,
            }],
            sel,
        );
    }

    /// Move a node to `index` (in the final child list) of `parent`.
    pub fn move_node(&mut self, id: &NodeId, parent: ParentRef, index: usize) {
        if self.doc.parent_of(id) == Some((parent.clone(), index)) {
            return;
        }
        let sel = self.selection.clone();
        let _ = self.commit(
            "Reorder layer",
            vec![Command::MoveNode {
                id: id.clone(),
                parent,
                index: Some(index),
            }],
            sel,
        );
    }

    /// Bring the selection forward (`+1`) or send it backward (`-1`) one step
    /// among its siblings.
    pub fn shift_selection(&mut self, delta: isize) {
        self.reorder(if delta > 0 {
            crate::structure::ZOrder::Forward
        } else {
            crate::structure::ZOrder::Backward
        });
    }

    /// Show another page (not an undoable edit, and not a document change).
    pub fn switch_page(&mut self, id: &PageId) {
        if self.doc.page(id).is_none() || self.page().id == *id {
            return;
        }
        self.end_gesture();
        self.current_page = Some(id.clone());
        self.selection.clear();
        self.view_switched();
    }

    /// Redraw everything after showing another page or view, which is not
    /// a document change.
    pub(crate) fn view_switched(&mut self) {
        self.scene_dirty = SceneDirty::All;
        // The canvas redraws when the revision changes; keep a clean
        // document clean.
        let clean = !self.is_dirty();
        self.revision += 1;
        if clean {
            self.saved_revision = self.revision;
        }
    }

    /// Move the selection by `(dx, dy)` page units as one transaction.
    /// Nudges within a second of the previous one on the same selection
    /// merge into that transaction (§5.3).
    pub fn nudge(&mut self, dx: f32, dy: f32) {
        self.end_gesture();
        let cmds: Vec<Command> = self
            .top_selected()
            .iter()
            .filter_map(|id| self.node(id))
            .filter(|n| !n.locked)
            .map(|n| Command::SetGeometry {
                id: n.id.clone(),
                x: n.x + dx,
                y: n.y + dy,
                width: n.width,
                height: n.height,
                rotation: n.rotation,
            })
            .collect();
        if cmds.is_empty() {
            return;
        }
        let now = self.clock.now();
        let sel = self.selection.clone();
        let label = layers_label("Nudge", cmds.len());
        let batch = vec![Command::Batch(cmds)];
        // Inside a component, each nudge is its own component update.
        let merge = self.master.is_none()
            && self
                .nudge
                .as_ref()
                .is_some_and(|n| n.can_merge(&sel, self.revision, now));
        let done = if merge && self.check_writable().is_ok() {
            let touched = Self::touched(&batch);
            let after = Selection { ids: sel.clone() };
            match self.history.amend_last(&mut self.doc, batch, after) {
                Ok(true) => {
                    self.changed(touched);
                    true
                }
                _ => false,
            }
        } else {
            self.commit(&label, batch, sel.clone()).is_ok()
        };
        self.nudge = done.then_some(NudgeState {
            selection: sel,
            at: now,
            revision: self.revision,
        });
    }

    /// The patch that sets a text node's content and stores the size it
    /// will show (§3.5: the file shows the size the user sees).
    pub fn text_patch(&mut self, id: &NodeId, content: &str) -> Option<NodePatch> {
        if id.is_derived() {
            // Inside an instance only the content can change (an override);
            // the instance lays it out.
            return Some(NodePatch {
                content: Some(content.to_string()),
                ..Default::default()
            });
        }
        let mut node = self.node(id)?.clone();
        let NodeKind::Text(t) = &mut node.kind else {
            return None;
        };
        t.content = content.to_string();
        let size = self.layout.measure_text(&self.doc, &node)?;
        let round = |v: f64| ((v * 100.0).round() / 100.0) as f32;
        Some(NodePatch {
            content: Some(content.to_string()),
            width: Some(round(size.width)),
            height: Some(round(size.height)),
            ..Default::default()
        })
    }

    /// A unique default name such as "Rectangle 3".
    pub fn next_name(&self, base: &str) -> String {
        let mut n = 0;
        self.doc.walk_nodes(&mut |node| {
            if let Some(rest) = node.name.strip_prefix(base) {
                if let Ok(k) = rest.trim().parse::<u32>() {
                    n = n.max(k);
                }
            }
        });
        format!("{base} {}", n + 1)
    }
}

/// The names of the fields a patch sets.
fn patch_fields(p: &NodePatch) -> Vec<String> {
    match serde_json::to_value(p) {
        Ok(serde_json::Value::Object(m)) => m.into_iter().map(|(k, _)| k).collect(),
        _ => Vec::new(),
    }
}

fn created_with() -> String {
    format!("tessel {}", env!("CARGO_PKG_VERSION"))
}

/// Where to insert when dropping layer `moving` relative to sibling list
/// position `target` of the same or another parent: the index in the final
/// child list (after `moving` is taken out). `in_front` means drop on the
/// front side (above it in the layers panel).
pub fn drop_index(moving_index: Option<usize>, target: usize, in_front: bool) -> usize {
    let target = match moving_index {
        Some(m) if m < target => target - 1,
        _ => target,
    };
    if in_front {
        target + 1
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Rect, TextSizing};

    fn rect(name: &str, x: f32) -> Node {
        Node::new(Id::new(IdKind::Node), name, NodeKind::Rect(Rect::default()))
            .with_geometry(x, 0.0, 10.0, 10.0)
    }

    fn add(ed: &mut Editor, n: Node) -> NodeId {
        let id = n.id.clone();
        let parent = ed.page_ref();
        ed.commit(
            "Add",
            vec![Command::CreateNode {
                parent,
                index: None,
                node: n,
            }],
            vec![id.clone()],
        )
        .unwrap();
        id
    }

    fn names(ed: &Editor) -> Vec<String> {
        ed.page().children.iter().map(|n| n.name.clone()).collect()
    }

    #[test]
    fn commit_undo_redo_restore_document_and_selection() {
        let mut ed = Editor::new();
        let a = add(&mut ed, rect("a", 0.0));
        assert_eq!(ed.selection, vec![a.clone()]);
        assert!(ed.geometry.get(&a).is_some());
        ed.delete_selection();
        assert!(ed.node(&a).is_none());
        assert!(ed.selection.is_empty());
        ed.undo();
        assert!(ed.node(&a).is_some());
        assert_eq!(ed.selection, vec![a.clone()]);
        ed.redo();
        assert!(ed.node(&a).is_none());
        assert!(ed.is_dirty());
    }

    #[test]
    fn drag_gesture_is_one_undo_step() {
        let mut ed = Editor::new();
        let a = add(&mut ed, rect("a", 0.0));
        let undo_depth = ed.history.transactions().len();
        ed.begin_gesture("Move").unwrap();
        for x in 1..=20 {
            ed.gesture_apply(Command::SetGeometry {
                id: a.clone(),
                x: x as f32,
                y: 0.0,
                width: 10.0,
                height: 10.0,
                rotation: 0.0,
            })
            .unwrap();
        }
        assert_eq!(ed.node(&a).unwrap().x, 20.0);
        ed.end_gesture();
        assert_eq!(ed.history.transactions().len(), undo_depth + 1);
        assert_eq!(ed.history.undo_label(), Some("Move"));
        ed.undo();
        assert_eq!(ed.node(&a).unwrap().x, 0.0);
        ed.redo();
        assert_eq!(ed.node(&a).unwrap().x, 20.0);
    }

    #[test]
    fn gesture_that_changes_nothing_is_not_recorded() {
        let mut ed = Editor::new();
        let a = add(&mut ed, rect("a", 5.0));
        let depth = ed.history.transactions().len();
        ed.begin_gesture("Move").unwrap();
        ed.gesture_apply(Command::SetGeometry {
            id: a.clone(),
            x: 9.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            rotation: 0.0,
        })
        .unwrap();
        ed.gesture_apply(Command::SetGeometry {
            id: a,
            x: 5.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            rotation: 0.0,
        })
        .unwrap();
        ed.end_gesture();
        assert_eq!(ed.history.transactions().len(), depth);
    }

    #[test]
    fn create_gesture_records_final_node_and_cancel_removes_it() {
        let mut ed = Editor::new();
        let n = rect("r", 0.0);
        let id = n.id.clone();
        ed.begin_gesture("Create rectangle").unwrap();
        ed.gesture_create(ed.page_ref(), n).unwrap();
        ed.gesture_apply(Command::SetGeometry {
            id: id.clone(),
            x: 3.0,
            y: 4.0,
            width: 50.0,
            height: 60.0,
            rotation: 0.0,
        })
        .unwrap();
        ed.selection = vec![id.clone()];
        ed.end_gesture();
        let tx = ed.history.transactions().last().unwrap();
        assert!(
            matches!(&tx.forward[..], [Command::CreateNode { node, .. }] if node.width == 50.0)
        );
        ed.undo();
        assert!(ed.node(&id).is_none());
        ed.redo();
        assert_eq!(ed.node(&id).unwrap().height, 60.0);

        let n = rect("s", 0.0);
        let id2 = n.id.clone();
        ed.begin_gesture("Create rectangle").unwrap();
        ed.gesture_create(ed.page_ref(), n).unwrap();
        ed.cancel_gesture();
        assert!(ed.node(&id2).is_none());
        assert_eq!(ed.history.undo_label(), Some("Create rectangle"));
    }

    #[test]
    fn reorder_rename_and_visibility_are_commands() {
        let mut ed = Editor::new();
        let a = add(&mut ed, rect("a", 0.0));
        add(&mut ed, rect("b", 0.0));
        add(&mut ed, rect("c", 0.0));
        // Bring "a" (index 0) in front of "c" (index 2).
        let idx = drop_index(Some(0), 2, true);
        ed.move_node(&a, ed.page_ref(), idx);
        assert_eq!(names(&ed), ["b", "c", "a"]);
        // Put it back behind "b".
        let idx = drop_index(Some(2), 0, false);
        ed.move_node(&a, ed.page_ref(), idx);
        assert_eq!(names(&ed), ["a", "b", "c"]);
        ed.selection = vec![a.clone()];
        ed.shift_selection(1);
        assert_eq!(names(&ed), ["b", "a", "c"]);
        ed.rename(&a, "renamed".into());
        ed.set_visible(&a, false);
        assert!(!ed.node(&a).unwrap().visible);
        assert_eq!(ed.node(&a).unwrap().name, "renamed");
        ed.undo();
        ed.undo();
        ed.undo();
        assert_eq!(names(&ed), ["a", "b", "c"]);
    }

    #[test]
    fn drop_index_math() {
        // Moving within the same parent, from behind the target.
        assert_eq!(drop_index(Some(0), 2, true), 2);
        assert_eq!(drop_index(Some(0), 2, false), 1);
        // From in front of the target.
        assert_eq!(drop_index(Some(3), 1, true), 2);
        assert_eq!(drop_index(Some(3), 1, false), 1);
        // From another parent.
        assert_eq!(drop_index(None, 1, true), 2);
        assert_eq!(drop_index(None, 1, false), 1);
    }

    #[test]
    fn save_reopen_save_is_byte_identical() {
        let mut ed = Editor::new();
        add(&mut ed, rect("a", 1.234));
        let text = Node::new(
            Id::new(IdKind::Node),
            "t",
            NodeKind::Text(doc::Text {
                content: "Hello".into(),
                sizing: TextSizing::AutoWidth,
                ..Default::default()
            }),
        );
        let tid = add(&mut ed, text);
        let patch = ed.text_patch(&tid, "Hello world").unwrap();
        ed.commit(
            "Edit text",
            vec![Command::SetProps {
                id: tid.clone(),
                patch,
            }],
            vec![],
        )
        .unwrap();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("app-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.tessel.json");
        ed.save_as(&path).unwrap();
        assert!(!ed.is_dirty());
        let first = std::fs::read(&path).unwrap();
        let mut again = Editor::open(&path).unwrap();
        assert!(again.problems.is_empty(), "{:?}", again.problems);
        // Values are rounded to 2 decimals on disk (§4.3), so compare the
        // canonical text rather than the in-memory documents.
        assert_eq!(doc::save_string(&again.doc), doc::save_string(&ed.doc));
        again.save_as(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_only_documents_refuse_edits() {
        let mut ed = Editor::new();
        ed.read_only = true;
        let parent = ed.page_ref();
        let r = ed.commit(
            "Add",
            vec![Command::CreateNode {
                parent,
                index: None,
                node: rect("a", 0.0),
            }],
            vec![],
        );
        assert!(r.is_err());
        assert!(ed.page().children.is_empty());
        assert!(ed.begin_gesture("Move").is_err());
    }

    #[test]
    fn default_names_count_up() {
        let mut ed = Editor::new();
        assert_eq!(ed.next_name("Rectangle"), "Rectangle 1");
        add(&mut ed, rect("Rectangle 1", 0.0));
        add(&mut ed, rect("Rectangle 7", 0.0));
        assert_eq!(ed.next_name("Rectangle"), "Rectangle 8");
    }
}

#[cfg(test)]
mod nudge_tests {
    use super::*;
    use doc::Rect;
    use std::time::Duration;

    fn setup() -> (Editor, Clock, Vec<NodeId>) {
        let clock = Clock::manual();
        let mut ed = Editor::new();
        ed.clock = clock.clone();
        let mut ids = Vec::new();
        for i in 0..3 {
            let id = Id::new(IdKind::Node);
            let n = Node::new(id.clone(), format!("R{i}"), NodeKind::Rect(Rect::default()))
                .with_geometry(i as f32 * 20.0, 0.0, 10.0, 10.0);
            let parent = ed.page_ref();
            ed.commit(
                "Add",
                vec![Command::CreateNode {
                    parent,
                    index: None,
                    node: n,
                }],
                vec![id.clone()],
            )
            .unwrap();
            ids.push(id);
        }
        ed.selection = ids.clone();
        (ed, clock, ids)
    }

    fn xs(ed: &Editor) -> Vec<f32> {
        ed.page().children.iter().map(|n| n.x).collect()
    }

    #[test]
    fn nudges_within_a_second_are_one_undo_step() {
        let (mut ed, clock, _) = setup();
        let before = ed.history.transactions().len();
        ed.nudge(1.0, 0.0);
        clock.advance(Duration::from_millis(800));
        ed.nudge(1.0, 0.0);
        clock.advance(Duration::from_millis(800));
        ed.nudge(10.0, 0.0);
        assert_eq!(ed.history.transactions().len(), before + 1);
        assert_eq!(ed.history.undo_label(), Some("Nudge 3 layers"));
        assert_eq!(xs(&ed), vec![12.0, 32.0, 52.0]);
        ed.undo();
        assert_eq!(xs(&ed), vec![0.0, 20.0, 40.0]);
        ed.redo();
        assert_eq!(xs(&ed), vec![12.0, 32.0, 52.0]);
    }

    #[test]
    fn nudges_a_second_apart_stay_separate() {
        let (mut ed, clock, _) = setup();
        let before = ed.history.transactions().len();
        ed.nudge(1.0, 0.0);
        clock.advance(Duration::from_millis(1100));
        ed.nudge(1.0, 0.0);
        assert_eq!(ed.history.transactions().len(), before + 2);
        ed.undo();
        assert_eq!(xs(&ed), vec![1.0, 21.0, 41.0]);
    }

    #[test]
    fn a_different_selection_or_another_edit_ends_the_merge() {
        let (mut ed, _, ids) = setup();
        let before = ed.history.transactions().len();
        ed.nudge(1.0, 0.0);
        ed.selection = vec![ids[0].clone()];
        ed.nudge(1.0, 0.0);
        assert_eq!(ed.history.transactions().len(), before + 2);
        assert_eq!(ed.history.undo_label(), Some("Nudge layer"));
        // Undo then nudge must not merge into the step before.
        ed.undo();
        ed.nudge(1.0, 0.0);
        assert_eq!(ed.history.transactions().len(), before + 2);
    }
}
