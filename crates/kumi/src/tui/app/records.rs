//! HISTORY's records: Kumi's changes and what it kept, with a revision every change to them bumps. HISTORY lays its
//! rows out again only when the revision moves, so nothing changes them except through these methods.
use super::Kept;
use kumi_runtime::core::contracts::ChangeRecord;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
pub(super) struct Records {
    changes: Vec<ChangeRecord>,
    kept: Vec<Rc<RefCell<Kept>>>,
    revision: u64,
}

impl Records {
    pub(super) fn changes(&self) -> &[ChangeRecord] {
        &self.changes
    }
    pub(super) fn kept(&self) -> &[Rc<RefCell<Kept>>] {
        &self.kept
    }
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    /// The changes, to change in any way.
    pub(super) fn changes_mut(&mut self) -> &mut Vec<ChangeRecord> {
        self.revision += 1;
        &mut self.changes
    }
    /// What's kept, to change in any way.
    pub(super) fn kept_mut(&mut self) -> &mut Vec<Rc<RefCell<Kept>>> {
        self.revision += 1;
        &mut self.kept
    }
    /// A kept entry forgotten, through its own cell.
    pub(super) fn set_forgotten(&mut self, entry: &RefCell<Kept>) {
        self.revision += 1;
        entry.borrow_mut().forgotten = true;
    }
}
