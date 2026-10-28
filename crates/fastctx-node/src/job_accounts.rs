//! Whom each job running here works for, and how many each account runs
//! (`world-network.md`, 共享机器与智能体协助): the owner of a machine may
//! let an account it is shared with run at most so many jobs at once, and
//! the machine tells who runs how many. The machine's own jobs, its owner's,
//! are under no account and under no limit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::run::{JobOwner, JobRecord};
use fastctx_types::stats::AccountJobs;

use crate::Node;

#[derive(Default)]
pub(crate) struct JobAccounts(Mutex<Running>);

#[derive(Default)]
struct Running {
    /// Each running job's record, with the account it works for.
    jobs: HashMap<PathBuf, Option<HubAccount>>,
    /// Jobs let start whose record is not written yet.
    starting: HashMap<u64, Option<HubAccount>>,
    next: u64,
}

/// A job let start, counted until its record is known or it fails to start.
pub(crate) struct Starting {
    node: Arc<Node>,
    id: u64,
}

impl Starting {
    /// The job started with its record at `path`: counted by it from now,
    /// unless it ended already.
    pub(crate) fn started(self, path: &Path) {
        let ended = fastctx_hands::run::record::read(path).is_ok_and(|record| record.end.is_some());
        let mut running = self.node.job_accounts.0.lock().expect("never poisoned");
        if let Some(account) = running.starting.remove(&self.id)
            && !ended
        {
            running.jobs.insert(path.to_path_buf(), account);
        }
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        self.node.job_accounts.0.lock().expect("never poisoned").starting.remove(&self.id);
    }
}

/// The account a job of `owner` works for: the one another machine sent it
/// for, or the one its resident or page works for here; none for the
/// machine's own.
pub(crate) fn of(node: &Node, owner: &JobOwner, sent_for: Option<&HubAccount>) -> Option<HubAccount> {
    if let Some(account) = sent_for {
        return Some(account.clone());
    }
    let acting = match owner {
        JobOwner::Resident(address) if node.is_me(&address.machine) => {
            node.residents.about(&address.resident).map(|(about, _)| about.acting)
        }
        JobOwner::Resident(_) => None,
        JobOwner::Page { page } => crate::pages::identity::acting(node, page).ok(),
    };
    match acting {
        Some(Acting::Accounts { accounts }) => accounts.first().cloned(),
        _ => None,
    }
}

/// The name of the cgroup an account's jobs share the processors in.
pub(crate) fn share_key(account: Option<&HubAccount>) -> String {
    match account {
        Some(account) => format!("{}-{}", account.hub, account.account),
        None => "own".to_string(),
    }
}

/// Lets a job for `account` start when the owner's limit for it leaves room;
/// it counts as running until it started or failed to.
pub(crate) fn start(node: &Arc<Node>, account: Option<HubAccount>) -> Result<Starting, Failure> {
    if let Some(account) = &account
        && let Some(most) = most(node, account)
    {
        prune(node, Some(account));
        let running = node.job_accounts.0.lock().expect("never poisoned");
        let count =
            running.jobs.values().chain(running.starting.values()).filter(|one| one.as_ref() == Some(account)).count();
        if count >= usize::from(most) {
            return Err(Failure::not_run(format!(
                "the owner of {} lets {} run at most {most} jobs at once there, and {count} run now; one has to end first",
                node.machine(),
                account.account
            )));
        }
    }
    let mut running = node.job_accounts.0.lock().expect("never poisoned");
    running.next += 1;
    let id = running.next;
    running.starting.insert(id, account);
    Ok(Starting { node: node.clone(), id })
}

/// A job's record as it stands: counted while it runs, no longer once it
/// ended. A node starting again learns of what runs this way.
pub(crate) fn seen(node: &Node, path: &Path, record: &JobRecord) {
    let mut running = node.job_accounts.0.lock().expect("never poisoned");
    if record.end.is_some() {
        running.jobs.remove(path);
    } else if !running.jobs.contains_key(path) {
        drop(running);
        let sent_for = record.on.as_ref().and_then(|on| on.account.as_ref()).or(record.account.as_ref());
        let account = of(node, &record.owner, sent_for);
        node.job_accounts.0.lock().expect("never poisoned").jobs.insert(path.to_path_buf(), account);
    }
}

/// How many jobs each account runs here now, with the limit the owner set
/// for it; the machine's own under none. Reads the records of what runs.
pub(crate) fn counts(node: &Node) -> Vec<AccountJobs> {
    prune(node, None);
    let running = node.job_accounts.0.lock().expect("never poisoned");
    let mut counts: HashMap<Option<HubAccount>, u32> = HashMap::new();
    for account in running.jobs.values().chain(running.starting.values()) {
        *counts.entry(account.clone()).or_default() += 1;
    }
    drop(running);
    let mut counts: Vec<AccountJobs> = counts
        .into_iter()
        .map(|(account, running)| {
            let most = account.as_ref().and_then(|account| most(node, account));
            AccountJobs { account, running, most }
        })
        .collect();
    counts.sort_by(|a, b| b.running.cmp(&a.running).then_with(|| a.account.cmp(&b.account)));
    counts
}

/// The limit the owner set for `account` on this machine, with its share.
fn most(node: &Node, account: &HubAccount) -> Option<u16> {
    let link = node.link_of(&account.hub)?;
    let view = link.view();
    let here = view.me()?.name.clone();
    view.directory
        .shares
        .iter()
        .find(|share| share.account == account.account && view.directory.same(&share.machine, &here))
        .and_then(|share| share.jobs)
}

/// Lets go of the jobs of `account`, or of every account, whose records say
/// they ended or are gone: a supervisor that ended without telling the node
/// is found this way before an account is held to its limit.
fn prune(node: &Node, account: Option<&HubAccount>) {
    let paths: Vec<PathBuf> = {
        let running = node.job_accounts.0.lock().expect("never poisoned");
        running
            .jobs
            .iter()
            .filter(|(_, one)| account.is_none_or(|account| one.as_ref() == Some(account)))
            .map(|(path, _)| path.clone())
            .collect()
    };
    let ended: Vec<PathBuf> = paths
        .into_iter()
        .filter(|path| {
            fastctx_hands::run::record::read(path)
                .map_or_else(|error| error.kind() == std::io::ErrorKind::NotFound, |record| record.end.is_some())
        })
        .collect();
    if ended.is_empty() {
        return;
    }
    let mut running = node.job_accounts.0.lock().expect("never poisoned");
    for path in ended {
        running.jobs.remove(&path);
    }
}
