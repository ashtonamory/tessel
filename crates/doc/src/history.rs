//! Transactions and the undo/redo history (§5.3).

use std::time::Instant;

use crate::command::{apply_all, Command, CommandError};
use crate::id::NodeId;
use crate::model::Document;

/// Who made a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Agent { client: String },
}

/// The selected nodes, restored by undo/redo.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub ids: Vec<NodeId>,
}

/// One undo step.
#[derive(Clone, Debug)]
pub struct Transaction {
    /// "Move 3 layers", "Agent: create pricing section".
    pub label: String,
    pub origin: Origin,
    pub forward: Vec<Command>,
    /// `inverse[i]` undoes `forward[i]`; undo applies them back to front.
    pub inverse: Vec<Command>,
    pub selection_before: Selection,
    pub selection_after: Selection,
    pub at: Instant,
}

impl Transaction {
    /// Apply `commands` atomically and return the transaction recording them.
    /// On error the document is unchanged.
    pub fn apply(
        doc: &mut Document,
        label: impl Into<String>,
        origin: Origin,
        commands: Vec<Command>,
        selection_before: Selection,
        selection_after: Selection,
    ) -> Result<Transaction, CommandError> {
        let inverse = apply_all(doc, commands.clone(), true)?;
        Ok(Transaction {
            label: label.into(),
            origin,
            forward: commands,
            inverse,
            selection_before,
            selection_after,
            at: Instant::now(),
        })
    }
}

pub const DEFAULT_HISTORY_LIMIT: usize = 500;

/// One linear history per document, shared by the user and agents.
#[derive(Clone, Debug)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        History::with_limit(DEFAULT_HISTORY_LIMIT)
    }
}

impl History {
    pub fn with_limit(limit: usize) -> History {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Apply `commands` as one transaction and record it.
    pub fn commit(
        &mut self,
        doc: &mut Document,
        label: impl Into<String>,
        origin: Origin,
        commands: Vec<Command>,
        selection_before: Selection,
        selection_after: Selection,
    ) -> Result<&Transaction, CommandError> {
        let tx = Transaction::apply(
            doc,
            label,
            origin,
            commands,
            selection_before,
            selection_after,
        )?;
        self.record(tx);
        Ok(self.undo.last().expect("just pushed"))
    }

    /// Record a transaction that has already been applied (e.g. a drag whose
    /// preview commands were applied live). Clears the redo stack.
    pub fn record(&mut self, tx: Transaction) {
        self.redo.clear();
        self.undo.push(tx);
        if self.undo.len() > self.limit {
            let excess = self.undo.len() - self.limit;
            self.undo.drain(..excess);
        }
    }

    /// Undo the most recent transaction, whatever its origin. Returns it (its
    /// `selection_before` is the selection to restore), or `None` if there
    /// is nothing to undo. On error the document and history are unchanged.
    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<&Transaction>, CommandError> {
        let Some(mut tx) = self.undo.pop() else {
            return Ok(None);
        };
        let back: Vec<Command> = tx.inverse.iter().rev().cloned().collect();
        match apply_all(doc, back, false) {
            Ok(mut redo) => {
                // The inverses of the inverses redo exactly this step.
                redo.reverse();
                tx.forward = redo;
                self.redo.push(tx);
                Ok(self.redo.last())
            }
            Err(e) => {
                self.undo.push(tx);
                Err(e)
            }
        }
    }

    /// Redo the most recently undone transaction. Its `selection_after` is
    /// the selection to restore.
    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<&Transaction>, CommandError> {
        let Some(mut tx) = self.redo.pop() else {
            return Ok(None);
        };
        match apply_all(doc, tx.forward.clone(), false) {
            Ok(inverse) => {
                tx.inverse = inverse;
                self.undo.push(tx);
                Ok(self.undo.last())
            }
            Err(e) => {
                self.redo.push(tx);
                Err(e)
            }
        }
    }

    /// Apply `commands` and fold them into the most recent transaction, so
    /// one undo reverts both (e.g. consecutive arrow-key nudges, §5.3).
    /// Returns `false`, doing nothing, when there is no transaction to extend.
    /// On error the document and history are unchanged.
    pub fn amend_last(
        &mut self,
        doc: &mut Document,
        commands: Vec<Command>,
        selection_after: Selection,
    ) -> Result<bool, CommandError> {
        let Some(tx) = self.undo.last_mut() else {
            return Ok(false);
        };
        let inverse = apply_all(doc, commands.clone(), true)?;
        tx.forward.extend(commands);
        tx.inverse.extend(inverse);
        tx.selection_after = selection_after;
        self.redo.clear();
        Ok(true)
    }

    /// The most recent transaction, for the caller to stamp (e.g. with an
    /// injected clock).
    pub fn last_mut(&mut self) -> Option<&mut Transaction> {
        self.undo.last_mut()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label for the "Undo …" menu item.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    /// Undoable transactions, oldest first (feeds the activity list).
    pub fn transactions(&self) -> &[Transaction] {
        &self.undo
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{Id, IdKind};
    use crate::model::{Node, NodeKind, Page, Rect};
    use crate::ParentRef;

    #[test]
    fn amend_last_folds_commands_into_one_undo_step() {
        let page = Page::new(Id::new(IdKind::Page), "P");
        let pid = page.id.clone();
        let mut doc = Document::new("T", page);
        let id = Id::new(IdKind::Node);
        let node = Node::new(id.clone(), "R", NodeKind::Rect(Rect::default()))
            .with_geometry(0.0, 0.0, 10.0, 10.0);
        let mut h = History::default();
        let sel = Selection::default();
        let mv = |x: f32| Command::SetGeometry {
            id: id.clone(),
            x,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            rotation: 0.0,
        };
        // Nothing to amend yet.
        assert!(!h
            .amend_last(&mut doc, vec![mv(1.0)], sel.clone())
            .is_ok_and(|b| b));
        h.commit(
            &mut doc,
            "Add",
            Origin::User,
            vec![Command::CreateNode {
                parent: ParentRef::Page(pid),
                index: None,
                node,
            }],
            sel.clone(),
            sel.clone(),
        )
        .unwrap();
        h.commit(
            &mut doc,
            "Nudge",
            Origin::User,
            vec![mv(1.0)],
            sel.clone(),
            sel.clone(),
        )
        .unwrap();
        assert!(h.amend_last(&mut doc, vec![mv(2.0)], sel.clone()).unwrap());
        assert!(h.amend_last(&mut doc, vec![mv(3.0)], sel).unwrap());
        assert_eq!(h.transactions().len(), 2);
        assert_eq!(doc.node(&id).unwrap().x, 3.0);
        h.undo(&mut doc).unwrap();
        assert_eq!(doc.node(&id).unwrap().x, 0.0);
        h.redo(&mut doc).unwrap();
        assert_eq!(doc.node(&id).unwrap().x, 3.0);
        h.undo(&mut doc).unwrap();
        assert_eq!(doc.node(&id).unwrap().x, 0.0);
    }
}
