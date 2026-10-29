//! Two-level round robin. Only accounts with pending work are visited.

use super::{Account, Bounds, Subject, count, decrease};
use std::collections::{HashMap, VecDeque};

pub(super) enum Decision {
    Run,
    Refuse,
    Wait,
}

#[derive(Default)]
pub(super) struct PoolQueue {
    order: VecDeque<Account>,
    accounts: HashMap<Account, AccountQueue>,
    running: HashMap<Account, usize>,
    all: usize,
}

#[derive(Default)]
struct AccountQueue {
    order: VecDeque<Subject>,
    subjects: HashMap<Subject, VecDeque<u64>>,
}

impl PoolQueue {
    pub fn push(&mut self, account: Account, subject: Subject, id: u64) {
        if !self.accounts.contains_key(&account) {
            self.order.push_back(account.clone());
        }
        let queue = self.accounts.entry(account).or_default();
        if !queue.subjects.contains_key(&subject) {
            queue.order.push_back(subject.clone());
        }
        queue.subjects.entry(subject).or_default().push_back(id);
    }

    pub fn remove(&mut self, account: &Account, subject: &Subject, id: u64) {
        let Some(queue) = self.accounts.get_mut(account) else { return };
        if let Some(ids) = queue.subjects.get_mut(subject) {
            ids.retain(|pending| *pending != id);
            if ids.is_empty() {
                queue.subjects.remove(subject);
                queue.order.retain(|key| key != subject);
            }
        }
        if queue.order.is_empty() {
            self.accounts.remove(account);
            self.order.retain(|key| key != account);
        }
    }

    pub fn release(&mut self, account: &Account) {
        self.all -= 1;
        decrease(&mut self.running, account.clone());
    }

    pub fn pump(&mut self, limits: Bounds, mut admit: impl FnMut(u64) -> Decision) {
        let mut missed = 0;
        while self.all < limits.all && missed < self.order.len() {
            let Some(account) = self.order.pop_front() else { break };
            let queue = self.accounts.get_mut(&account).expect("queued account");
            let mut chosen = None;
            if count(&self.running, &account) < limits.account {
                for _ in 0..queue.order.len() {
                    let subject = queue.order.pop_front().expect("queued subject");
                    let pending = queue.subjects.get_mut(&subject).expect("subject");
                    let decision = admit(*pending.front().expect("pending call"));
                    if !matches!(decision, Decision::Wait) {
                        pending.pop_front();
                        chosen = Some(decision);
                        if pending.is_empty() {
                            queue.subjects.remove(&subject);
                        } else {
                            queue.order.push_back(subject);
                        }
                        break;
                    }
                    queue.order.push_back(subject);
                }
            }
            if queue.order.is_empty() {
                self.accounts.remove(&account);
            } else {
                self.order.push_back(account.clone());
            }
            if let Some(decision) = chosen {
                if matches!(decision, Decision::Run) {
                    self.all += 1;
                    *self.running.entry(account).or_default() += 1;
                }
                missed = 0;
            } else {
                missed += 1;
            }
        }
    }
}
