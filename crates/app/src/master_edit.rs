//! Editing a component's master (§3.7): "Edit component" shows the master
//! on its own Components page, where the canvas, layers panel and inspector
//! work as on any page.
//!
//! Meanwhile `Editor::doc` is a view: the document with its pages replaced
//! by that one page, which holds a copy of the master root at the origin.
//! Every committed edit of the view becomes one `UpsertComponent` of the
//! document itself (kept in [`MasterEdit`]), so history, saving, autosave
//! and agents only ever see the real document, and every instance updates.
//! The master root's own position is not part of the definition (instances
//! place it), so moving it in the view is not recorded.

use std::mem;

use doc::expand::component;
use doc::{
    Command, CommandError, Component, ComponentId, Document, Id, IdKind, NodeId, Origin, Page,
    PageId, ParentRef, Selection, Transaction,
};
use layout::{Geometry, LayoutEngine};

use crate::editor::Editor;

pub struct MasterEdit {
    pub component: ComponentId,
    /// The document itself.
    real: Document,
    /// Layout of `real`, kept current for agents.
    real_layout: LayoutEngine,
    real_geometry: Geometry,
    /// Where the user was before, to go back to.
    return_page: Option<PageId>,
    return_selection: Vec<NodeId>,
}

impl MasterEdit {
    pub(crate) fn document_mut(&mut self) -> &mut Document {
        &mut self.real
    }

    /// Whether `tx` only changes the component being edited, so undoing or
    /// redoing it can stay in the component view.
    pub(crate) fn owns(&self, tx: &Transaction) -> bool {
        tx.forward.iter().chain(&tx.inverse).all(
            |c| matches!(c, Command::UpsertComponent { component } if component.id == self.component),
        )
    }
}

/// The ID of the Components page view. It is never stored in a document.
pub fn components_page_id() -> PageId {
    Id::from_entropy(IdKind::Page, 0x636f_6d70_6f6e_656e)
}

/// The view of `real` that shows component `id`'s master. The component
/// itself is left out of the view's component list, so its nodes exist
/// once (on the page) and can be deleted and re-created there.
fn view_of(real: &Document, id: &ComponentId) -> Option<Document> {
    let c = component(real, id)?;
    let mut root = c.root.clone();
    (root.x, root.y, root.rotation) = (0.0, 0.0, 0.0);
    let mut page = Page::new(components_page_id(), "Components");
    page.children = vec![root];
    let mut view = real.clone();
    view.pages = vec![page];
    view.components.retain(|c| &c.id != id);
    view.rebuild_index();
    Some(view)
}

/// Why an edit of the view cannot become a component update.
fn outside_master() -> CommandError {
    CommandError::InvalidPatch(
        "while editing a component, layers stay inside its frame; leave the component to \
         change anything else"
            .into(),
    )
}

impl Editor {
    /// The document itself, also while a component is being edited (when
    /// `doc` is a view of it).
    pub fn document(&self) -> &Document {
        self.master.as_ref().map_or(&self.doc, |m| &m.real)
    }

    /// Geometry of [`Editor::document`].
    pub fn document_geometry(&self) -> &Geometry {
        self.master
            .as_ref()
            .map_or(&self.geometry, |m| &m.real_geometry)
    }

    /// The document page being shown, or the one to go back to while a
    /// component is being edited.
    pub fn document_page(&self) -> &Page {
        match &self.master {
            Some(m) => m
                .return_page
                .as_ref()
                .and_then(|id| m.real.page(id))
                .unwrap_or(&m.real.pages[0]),
            None => self.page(),
        }
    }

    /// The component whose master is being edited.
    pub fn editing_component(&self) -> Option<&ComponentId> {
        self.master.as_ref().map(|m| &m.component)
    }

    /// The master root in the component view.
    pub fn master_root(&self) -> Option<&NodeId> {
        self.master.as_ref()?;
        self.doc.pages.first()?.children.first().map(|n| &n.id)
    }

    /// Where new layers go: the page, or the master root while a component
    /// is being edited (which sits at the origin, so page coordinates are
    /// its local coordinates).
    pub fn insert_parent(&self) -> ParentRef {
        match self.master_root() {
            Some(root) => ParentRef::Node(root.clone()),
            None => self.page_ref(),
        }
    }

    /// Show component `id`'s master on the Components page, with its root
    /// selected. Returns `false` if there is no such component.
    pub fn edit_component(&mut self, id: &ComponentId) -> bool {
        self.end_gesture();
        if self.editing_component() == Some(id) {
            return true;
        }
        self.leave_component();
        let Some(view) = view_of(&self.doc, id) else {
            self.status = Some(format!("Component {id} not found"));
            return false;
        };
        let root = view.pages[0].children[0].id.clone();
        let real = mem::replace(&mut self.doc, view);
        let real_layout = mem::take(&mut self.layout);
        let real_geometry = mem::replace(&mut self.geometry, self.layout.compute(&self.doc));
        self.master = Some(MasterEdit {
            component: id.clone(),
            real,
            real_layout,
            real_geometry,
            return_page: self.current_page.replace(components_page_id()),
            return_selection: mem::replace(&mut self.selection, vec![root]),
        });
        self.view_switched();
        true
    }

    /// Go back from the Components page to the page shown before.
    pub fn leave_component(&mut self) {
        if self.master.is_none() {
            return;
        }
        self.end_gesture();
        let Some(m) = self.master.take() else {
            return;
        };
        self.doc = m.real;
        self.layout = m.real_layout;
        self.geometry = self.layout.compute(&self.doc);
        self.current_page = m.return_page.filter(|p| self.doc.page(p).is_some());
        let doc = &self.doc;
        self.selection = m
            .return_selection
            .into_iter()
            .filter(|id| doc.node(id).is_some())
            .collect();
        self.view_switched();
    }

    /// Rebuild the view after the document changed underneath it; leave if
    /// the component is gone.
    fn refresh_master_view(&mut self) {
        let Some(m) = self.master.as_mut() else {
            return;
        };
        m.real_geometry = m.real_layout.compute(&m.real);
        match view_of(&m.real, &m.component) {
            Some(view) => {
                self.doc = view;
                self.geometry = self.layout.compute(&self.doc);
                let (doc, geometry) = (&self.doc, &self.geometry);
                self.selection.retain(|id| {
                    doc.node(id).is_some() || (id.is_derived() && geometry.get(id).is_some())
                });
                self.scene_dirty = crate::editor::SceneDirty::All;
                self.revision += 1;
            }
            None => self.leave_component(),
        }
    }

    /// After undo or redo in the component view: stay if the step was an
    /// edit of this component, else show the document again.
    pub(crate) fn master_history_step(&mut self, owns: bool, selection: Vec<NodeId>) {
        if owns {
            self.selection = selection;
            self.refresh_master_view();
        } else {
            self.leave_component();
            let doc = &self.doc;
            self.selection = selection
                .into_iter()
                .filter(|id| doc.node(id).is_some())
                .collect();
        }
    }

    /// `Editor::commit` in the component view: apply to the view, then
    /// record the new definition.
    pub(crate) fn master_commit(
        &mut self,
        label: &str,
        commands: Vec<Command>,
        selection_after: Vec<NodeId>,
    ) -> Result<(), CommandError> {
        let before = Selection {
            ids: self.selection.clone(),
        };
        let touched = Self::touched(&commands);
        let mut inverses = Vec::new();
        for c in commands {
            match c.apply(&mut self.doc) {
                Ok(inv) => inverses.push(inv),
                Err(e) => {
                    self.rollback_view(inverses, before.ids);
                    self.status = Some(format!("{label}: {e}"));
                    return Err(e);
                }
            }
        }
        self.selection = selection_after;
        self.changed(touched);
        self.write_sizes(&mut inverses);
        self.master_record(label, inverses, before, false)
            .inspect_err(|e| self.status = Some(format!("{label}: {e}")))
    }

    /// `amend_last` in the component view: fold `commands` into the last
    /// component update.
    pub(crate) fn master_amend(&mut self, commands: Vec<Command>) {
        let before = Selection {
            ids: self.selection.clone(),
        };
        let touched = Self::touched(&commands);
        let mut inverses = Vec::new();
        for c in commands {
            match c.apply(&mut self.doc) {
                Ok(inv) => inverses.push(inv),
                Err(e) => {
                    self.rollback_view(inverses, before.ids);
                    self.status = Some(e.to_string());
                    return;
                }
            }
        }
        self.changed(touched);
        self.write_sizes(&mut inverses);
        if let Err(e) = self.master_record("", inverses, before, true) {
            self.status = Some(e.to_string());
        }
    }

    /// Store computed sizes in the view, as commits do (§3.5).
    fn write_sizes(&mut self, inverses: &mut Vec<Command>) {
        let writes = self.size_writes();
        let touched = Self::touched(&writes);
        for w in writes {
            match w.apply(&mut self.doc) {
                Ok(inv) => inverses.push(inv),
                Err(e) => tracing::error!("size write-back: {e}"),
            }
        }
        if !touched.is_empty() {
            self.relayout(&touched);
        }
    }

    /// Undo edits of the view (`inverses` in the order they were made).
    fn rollback_view(&mut self, inverses: Vec<Command>, selection: Vec<NodeId>) {
        let touched = Self::touched(&inverses);
        for inv in inverses.into_iter().rev() {
            if let Err(e) = inv.apply(&mut self.doc) {
                tracing::error!("rolling back a component edit: {e}");
            }
        }
        self.selection = selection;
        self.changed(touched);
    }

    /// The component as the view now defines it.
    fn viewed_component(&self) -> Result<Component, CommandError> {
        let m = self.master.as_ref().expect("editing a component");
        let old = component(&m.real, &m.component)
            .ok_or_else(|| CommandError::NotFound(m.component.to_string()))?;
        let [root] = self.doc.pages[0].children.as_slice() else {
            return Err(outside_master());
        };
        if root.id != old.root.id {
            return Err(outside_master());
        }
        let others = m.real.components.iter().filter(|c| c.id != m.component);
        if !self.doc.components.iter().eq(others) {
            return Err(CommandError::InvalidPatch(
                "components cannot be created or changed while editing one".into(),
            ));
        }
        let mut root = root.clone();
        (root.x, root.y, root.rotation) = (old.root.x, old.root.y, old.root.rotation);
        Ok(Component {
            root,
            ..old.clone()
        })
    }

    /// Record the view's edits (undone by `inverses`, in the order they
    /// were made) as one update of the component, or fold them into the
    /// last step with `amend`. On error the view is rolled back.
    pub(crate) fn master_record(
        &mut self,
        label: &str,
        inverses: Vec<Command>,
        before: Selection,
        amend: bool,
    ) -> Result<(), CommandError> {
        let result = self.viewed_component().and_then(|c| {
            let m = self.master.as_mut().expect("editing a component");
            if component(&m.real, &m.component) == Some(&c) {
                return Ok(());
            }
            let cmds = vec![Command::UpsertComponent { component: c }];
            let after = Selection {
                ids: self.selection.clone(),
            };
            if amend {
                self.history.amend_last(&mut m.real, cmds, after)?;
            } else {
                self.history.commit(
                    &mut m.real,
                    label,
                    Origin::User,
                    cmds,
                    before.clone(),
                    after,
                )?;
                if let Some(last) = self.history.last_mut() {
                    last.at = self.clock.now();
                }
            }
            m.real_geometry = m.real_layout.compute(&m.real);
            Ok(())
        });
        if let Err(e) = &result {
            tracing::warn!("{label}: {e}");
            self.rollback_view(inverses, before.ids);
        }
        result
    }

    /// Replace a component's name or props (not its root, which the canvas
    /// edits) as one undoable step, also while a component is being edited.
    pub fn update_component(
        &mut self,
        label: &str,
        id: &ComponentId,
        f: impl FnOnce(&mut Component),
    ) -> Result<(), CommandError> {
        self.end_gesture();
        self.check_writable()?;
        let Some(mut c) = component(self.document(), id).cloned() else {
            return Err(CommandError::NotFound(id.to_string()));
        };
        let old = c.clone();
        f(&mut c);
        if c == old {
            return Ok(());
        }
        let cmds = vec![Command::UpsertComponent { component: c }];
        let Some(m) = self.master.as_mut() else {
            let sel = self.selection.clone();
            return self.commit(label, cmds, sel);
        };
        let sel = Selection {
            ids: self.selection.clone(),
        };
        if let Err(e) =
            self.history
                .commit(&mut m.real, label, Origin::User, cmds, sel.clone(), sel)
        {
            self.status = Some(format!("{label}: {e}"));
            return Err(e);
        }
        if let Some(last) = self.history.last_mut() {
            last.at = self.clock.now();
        }
        self.refresh_master_view();
        Ok(())
    }
}
