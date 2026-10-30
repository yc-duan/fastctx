//! Candidate collection and the existing account visibility policy for live activity.

use super::{AccountName, Afoot, Arc, BTreeSet, MachineName, ResidentId, Seen};
use fastctx_net::directory_index::Indexed;
use std::collections::{HashMap, HashSet};

/// Candidates include every way an account can see a member; the policy still
/// decides visibility, including locked accounts and expiring assistance.
pub(super) fn candidates<'a>(
    directory: &Indexed<'_>,
    logins: &BTreeSet<AccountName>,
    told: &'a HashMap<MachineName, Afoot>,
) -> Vec<&'a MachineName> {
    if logins.iter().any(|account| directory.is_super(account)) {
        return told.keys().collect();
    }
    let mut positions = HashSet::new();
    for account in logins {
        positions.extend(directory.owned(account).iter().copied());
        positions.extend(directory.logged(account).iter().copied());
        for &at in directory.shares_by_account(account) {
            positions.extend(directory.position(&directory.all.shares[at].machine));
        }
        for &at in directory.assists_by_account(account) {
            positions.extend(directory.position(&directory.all.assists[at].machine));
        }
    }
    positions
        .into_iter()
        .filter_map(|at| told.get_key_value(&directory.all.members[at].name).map(|(name, _)| name))
        .collect()
}

/// Whether a member logged in to `logins` sees `machine` at all, as its
/// view of the directory has it (`crate::views`).
pub(super) fn sees(directory: &Indexed<'_>, logins: &BTreeSet<AccountName>, machine: &MachineName) -> bool {
    let Some(member) = directory.member(machine) else { return false };
    logins.iter().any(|account| {
        directory.is_super(account) || member.logins.contains(account) || directory.rights(account, machine).any()
    })
}

/// What a member logged in to `logins` may see of what is afoot on `machine`;
/// of the conversations being deleted there (`hidden`), nothing.
pub(super) fn seen(
    directory: &Indexed<'_>,
    logins: &BTreeSet<AccountName>,
    machine: &MachineName,
    afoot: &Afoot,
    hidden: Option<&BTreeSet<ResidentId>>,
) -> Seen {
    let theirs = |accounts: &[AccountName]| accounts.iter().any(|account| logins.contains(account));
    let shown = |id: &ResidentId| hidden.is_none_or(|hidden| !hidden.contains(id));
    let owner =
        directory.member(machine).and_then(|member| member.home.as_ref()).is_some_and(|home| logins.contains(home));
    let mut seen = Seen::default();
    let projected = Arc::make_mut(&mut seen.afoot);
    for (id, resident) in &afoot.residents {
        if theirs(&resident.accounts) && shown(id) {
            projected.residents.insert(id.clone(), resident.resident.clone());
        }
    }
    for (id, job) in &afoot.jobs {
        let of_hidden =
            job.job.owner.resident().is_some_and(|owner| owner.machine == *machine && !shown(&owner.resident));
        if theirs(&job.accounts) && !of_hidden {
            projected.jobs.insert(id.clone(), job.job.clone());
        }
    }
    // What works there for accounts that may work on the machine counts;
    // what its logins do assisting other accounts does not.
    seen.busy = owner.then(|| {
        let counted = afoot
            .residents
            .iter()
            .filter(|(id, _)| shown(id))
            .map(|(_, resident)| resident)
            .filter(|resident| resident.resident.in_progress())
            .filter(|resident| resident.accounts.iter().any(|account| directory.rights(account, machine).hands))
            .count();
        u32::try_from(counted).unwrap_or(u32::MAX)
    });
    seen
}
