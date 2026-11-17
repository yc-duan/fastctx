//! Every component observes complete operation and transaction lifecycles.

use super::{Change, Guard, MigrationPurpose};
use crate::{
    Error, Result,
    authorization::{AllPolicies, WritePolicy},
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::Arc,
};

pub struct CompositeGuard {
    children: Vec<Box<dyn Guard>>,
}
impl CompositeGuard {
    pub fn new(children: Vec<Box<dyn Guard>>) -> Self {
        Self { children }
    }
    fn checked(&mut self, mut apply: impl FnMut(&mut dyn Guard) -> Result<()>) -> Result<()> {
        let mut failure = None;
        for child in &mut self.children {
            let result = catch_unwind(AssertUnwindSafe(|| apply(child.as_mut()))).unwrap_or_else(|_| {
                super::mark_panicked();
                Err(Error::new("a database guard component failed unexpectedly"))
            });
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    fn notify(&mut self, mut apply: impl FnMut(&mut dyn Guard)) {
        let mut failure = None;
        for child in &mut self.children {
            if let Err(error) = catch_unwind(AssertUnwindSafe(|| apply(child.as_mut()))) {
                failure.get_or_insert(error);
            }
        }
        // A broken rollback observer cannot be reused as a sound fence (2026-11-08).
        if let Some(error) = failure {
            resume_unwind(error);
        }
    }
}
impl Guard for CompositeGuard {
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        self.checked(|child| child.check(change))
    }
    fn check_migration(&mut self, change: &Change<'_>, purpose: MigrationPurpose) -> Result<()> {
        self.checked(|child| child.check_migration(change, purpose))
    }
    fn validate_operation(&mut self, connection: &rusqlite::Connection) -> Result<()> {
        self.checked(|child| child.validate_operation(connection))
    }
    fn write_policy(&self) -> Option<Arc<dyn WritePolicy>> {
        let policies: Vec<_> = self.children.iter().filter_map(|child| child.write_policy()).collect();
        (!policies.is_empty()).then(|| Arc::new(AllPolicies(policies)) as Arc<dyn WritePolicy>)
    }
    fn ended(&mut self, kept: bool) {
        self.notify(|child| child.ended(kept));
    }
    fn committed(&mut self, stood: bool) {
        self.notify(|child| child.committed(stood));
    }
}
