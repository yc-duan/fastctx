//! Who came into an account a member is logged in to (`world-network.md`,
//! 登录与入网): a device that logged in to it, or a machine registered under
//! it. Each time the directory changes, a member of one of these accounts
//! not seen there before is noted, with when it was noticed, until the
//! user has seen it, so an interface can say who came in and offer 「不是
//! 我」. The directory, as the hub pushes it, is the only source; the first
//! look at an account counts everyone there as seen.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use fastctx_types::accounts::Arrival;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::directory::View;
use crate::sign::PublicKey;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct File {
    /// Every member of each account that was noticed.
    seen: BTreeMap<AccountName, BTreeSet<(MachineName, PublicKey)>>,
    /// Those the user has not been shown yet.
    fresh: Vec<Arrival>,
}

/// Notes, in `path`, who came into the accounts `view`'s member is logged
/// in to; returns those noticed now.
pub fn notice(path: &Path, view: &View) -> io::Result<Vec<Arrival>> {
    let Some(me) = view.me() else { return Ok(Vec::new()) };
    let mut file: File = crate::link::state::read_or(path, File::default())?;
    let mut changed = false;
    let mut noticed = Vec::new();
    for account in &me.logins {
        let members: BTreeSet<(MachineName, PublicKey)> = view
            .members
            .iter()
            .filter(|member| member.key != me.key)
            .filter(|member| member.logins.contains(account) || member.home.as_ref() == Some(account))
            .map(|member| (member.name.clone(), member.key))
            .collect();
        let first = !file.seen.contains_key(account);
        let seen = file.seen.entry(account.clone()).or_default();
        for (name, key) in members {
            if seen.insert((name.clone(), key)) {
                changed = true;
                if !first {
                    let found = view.member(&name);
                    let device = found.is_some_and(|member| !member.is_machine());
                    // Managed machines of one code are told together.
                    let code = found
                        .filter(|member| member.managed)
                        .and_then(|member| member.code)
                        .map(|code| code.to_string());
                    noticed.push(Arrival {
                        hub: view.hub.clone(),
                        account: account.clone(),
                        name,
                        device,
                        noticed_at: Timestamp::now(),
                        code,
                    });
                }
            }
        }
        changed |= first;
    }
    file.fresh.extend(noticed.iter().cloned());
    if changed {
        crate::link::state::write(path, &file)?;
    }
    Ok(noticed)
}

/// Who came in and was not shown yet.
pub fn fresh(path: &Path) -> io::Result<Vec<Arrival>> {
    crate::link::state::read_or(path, File::default()).map(|file: File| file.fresh)
}

/// The user has seen that `name` came into `account`.
pub fn shown(path: &Path, account: &AccountName, name: &MachineName) -> io::Result<()> {
    let mut file: File = crate::link::state::read_or(path, File::default())?;
    file.fresh.retain(|arrival| !(arrival.account == *account && arrival.name == *name));
    crate::link::state::write(path, &file)
}
