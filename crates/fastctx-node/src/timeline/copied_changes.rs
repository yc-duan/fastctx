//! The machine changes an account reads, as its records' scope here holds
//! them, read while the hub is not reached (plan 3-5, design §28.27): the
//! hub writes each record the account reads into the scope by its number
//! (`machine.audit`, stream `audit`, `hub assets::told`), so the machine
//! log's pages are answered from it, newest first. Only what the account
//! itself reads is there: a super administrator's reading of the whole
//! network stays with the hub.

use crate::Node;
use fastctx_content::v2::{Kind as _, MachineAudit};
use fastctx_net::link::Link;
use fastctx_types::AccountName;
use fastctx_types::machine_log::{Page, Record};
use std::collections::VecDeque;

/// The most records one page carries, as the hub's own page at most.
const PAGE: usize = 256;

/// The page of `account`'s machine changes on the hub of `link` before
/// `before`, newest first, as its records' scope here holds them; none when
/// no such copy is here.
pub(crate) fn page(node: &Node, link: &Link, account: &AccountName, before: Option<u64>) -> Option<Page> {
    let library = &node.assets.as_ref()?.library;
    let prefix = format!("a/{}/{account}/records-", link.membership().id.as_str());
    // The account as opened last: the scope of an earlier one of its name is
    // not its own.
    let scope = library
        .held_scopes()
        .ok()?
        .into_iter()
        .filter(|scope| scope.key.starts_with(&prefix) && super::copy::records_here(node, scope))
        .max_by_key(|scope| scope.key[prefix.len()..].parse::<i64>().unwrap_or(i64::MIN))?;
    // Its records by number, oldest first: the last page's worth before
    // `before` kept.
    let (mut newest, mut older, mut after) = (VecDeque::new(), false, None);
    'pages: loop {
        let entries = library.page(&scope.key, MachineAudit::STREAM, after.take(), 500).ok()?;
        let full = entries.len() == 500;
        for entry in &entries {
            let body = super::copy::whole_body(library, entry)?;
            let Ok(record) = serde_json::from_slice::<Record>(&body) else { continue };
            if before.is_some_and(|before| record.id >= before) {
                break 'pages;
            }
            newest.push_back(record);
            if newest.len() > PAGE {
                newest.pop_front();
                older = true;
            }
        }
        match entries.last() {
            Some(last) if full => after = Some((last.order.clone(), last.item.clone())),
            _ => break,
        }
    }
    let records: Vec<Record> = newest.into_iter().rev().collect();
    let next_before = if older { records.last().map(|record| record.id) } else { None };
    Some(Page { records, next_before })
}
