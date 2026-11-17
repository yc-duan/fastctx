//! Derived indexes over the unchanged durable alias array.

use super::Alias;
use fastctx_draft::Id;
use fastctx_types::composer::CharacterId;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Aliases {
    operations: HashMap<String, HashMap<usize, Id>>,
    identities: HashMap<Id, usize>,
    order: Vec<String>,
    pub bytes: usize,
}
impl Aliases {
    pub fn insert(&mut self, alias: &Alias, index: usize) {
        let id = Id { client: alias.client, clock: alias.clock };
        if let Some(identities) = self.operations.get_mut(&alias.old.operation) {
            identities.entry(alias.old.index).or_insert(id);
        } else {
            self.order.push(alias.old.operation.clone());
            self.operations.insert(alias.old.operation.clone(), HashMap::from([(alias.old.index, id)]));
        }
        self.identities.insert(id, index);
        self.bytes += encoded(alias);
    }
    pub fn remove(&mut self, alias: &Alias) {
        let id = Id { client: alias.client, clock: alias.clock };
        self.identities.remove(&id);
        if let Some(identities) = self.operations.get_mut(&alias.old.operation) {
            identities.remove(&alias.old.index);
            if identities.is_empty() {
                self.operations.remove(&alias.old.operation);
            }
        }
        self.bytes -= encoded(alias);
    }
    pub fn moved(&mut self, alias: &Alias, index: usize) {
        self.identities.insert(Id { client: alias.client, clock: alias.clock }, index);
    }
    pub fn retain_operations(&mut self) {
        self.order.retain(|operation| self.operations.contains_key(operation));
    }
    pub fn resolve(&self, id: &CharacterId) -> Option<Id> {
        self.operations.get(&id.operation)?.get(&id.index).copied()
    }
    pub fn index(&self, id: Id) -> Option<usize> {
        self.identities.get(&id).copied()
    }
    pub fn contains(&self, operation: &str) -> bool {
        self.operations.contains_key(operation)
    }
    pub fn operations(&self) -> Vec<String> {
        self.order.clone()
    }
}

pub(super) fn encoded(alias: &Alias) -> usize {
    r#"{"old":{"operation":"","index":},"client":,"clock":}"#.len()
        + super::super::limits::escaped(&alias.old.operation)
        + digits(alias.old.index as u64)
        + digits(alias.client)
        + digits(alias.clock.into())
}
fn digits(number: u64) -> usize {
    if number == 0 { 1 } else { number.ilog10() as usize + 1 }
}
