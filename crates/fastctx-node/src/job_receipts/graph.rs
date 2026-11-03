//! Locks only the jobs participating in one causal receipt. Discovery leases
//! their files, then takes their gates in path order. It checks the edges again
//! with those gates held: a newly bound child extends the set and retries,
//! never acquiring a child gate while holding its parent's out of order.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Read {
    Snapshot,
    Frozen,
    Confirm,
    ConfirmExit,
}

impl Read {
    fn child(self, through_exit: bool) -> Self {
        match (self, through_exit) {
            (Self::Snapshot, false) => Self::Snapshot,
            (Self::Snapshot, true) => Self::Frozen,
            (_, false) => Self::Confirm,
            (_, true) => Self::ConfirmExit,
        }
    }
}

struct Entry {
    reads: BTreeSet<Read>,
    _files: crate::job_reader::Lease,
    gate: crate::job_files::Gate,
}

pub(super) struct Graph {
    entries: BTreeMap<PathBuf, Entry>,
    jobs: HashMap<JobId, Vec<PathBuf>>,
}

impl Graph {
    fn add(&mut self, node: &Node, path: PathBuf, read: Read) -> bool {
        if let Some(entry) = self.entries.get_mut(&path) {
            return entry.reads.insert(read);
        }
        let Some(files) = node.job_files.hold(&path) else { return false };
        let Some(job) = path.file_stem().and_then(|name| name.to_str()).and_then(|name| name.parse().ok()) else {
            return false;
        };
        self.jobs.entry(job).or_default().push(path.clone());
        let gate = node.job_files.gate(&path);
        self.entries.insert(path, Entry { reads: BTreeSet::from([read]), _files: files, gate });
        true
    }

    pub(super) fn paths(&self, job: &JobId) -> &[PathBuf] {
        self.jobs.get(job).map_or(&[], Vec::as_slice)
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }
}

pub(super) fn with<T>(
    node: &Node,
    root: &Path,
    read: Read,
    action: impl FnOnce(&Graph) -> io::Result<T>,
) -> io::Result<T> {
    let mut graph = Graph { entries: BTreeMap::new(), jobs: HashMap::new() };
    let mut pending = VecDeque::new();
    if graph.add(node, root.to_path_buf(), read) {
        pending.push_back((root.to_path_buf(), read));
    }
    loop {
        // Discover a stable graph in one traversal before validating it as a
        // whole, rather than rescanning every ancestor for each deeper level.
        while let Some((path, read)) = pending.pop_front() {
            let gate = graph.entries[&path].gate.clone();
            let held = gate.lock();
            let missing = discover(node, &graph, &path, read)?;
            drop(held);
            for (path, _files, read) in missing {
                if graph.add(node, path.clone(), read) {
                    pending.push_back((path, read));
                }
            }
        }
        // Rank compares the displayed subjects, not Path's component ordering
        // (a directory name that prefixes another can order differently).
        let mut gates: Vec<_> =
            graph.entries.iter().map(|(path, entry)| (path.to_string_lossy(), &entry.gate)).collect();
        gates.sort_by(|left, right| left.0.cmp(&right.0));
        let guards: Vec<_> = gates.iter().map(|(_, gate)| gate.lock()).collect();
        let mut missing = Vec::new();
        for (path, entry) in &graph.entries {
            for read in &entry.reads {
                missing.extend(discover(node, &graph, path, *read)?);
            }
        }
        if missing.is_empty() {
            return action(&graph);
        }
        drop(guards);
        drop(gates);
        // Keep the discovered incarnation alive until its registry entry owns
        // a lease, including when another collector is trying to retire it.
        for (path, _files, read) in missing {
            if graph.add(node, path.clone(), read) {
                pending.push_back((path, read));
            }
        }
    }
}

fn discover(
    node: &Node,
    graph: &Graph,
    path: &Path,
    read: Read,
) -> io::Result<Vec<(PathBuf, crate::job_reader::Lease, Read)>> {
    let job = match record::read(path) {
        Ok(job) => job,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut missing = Vec::new();
    let children = match read {
        Read::Frozen => return Ok(Vec::new()),
        Read::Confirm | Read::ConfirmExit if job.output_lost.is_some() || job.stderr_lost.is_some() => {
            return Ok(Vec::new());
        }
        Read::ConfirmExit => {
            storage::frozen(path)?
                .ok_or_else(|| io::Error::other("the partial on end has no frozen child receipts"))?
                .children
        }
        Read::Snapshot | Read::Confirm => storage::children(path)?,
    };
    for child in children {
        if !node.is_here(&child.destination) {
            continue;
        }
        let next = read.child(child.through_exit);
        let origin = if read == Read::Snapshot { None } else { child.origin.as_ref() };
        for candidate in candidates(node, &child.completed.job, origin, &job.owner)? {
            if graph.entries.get(&candidate).is_some_and(|entry| entry.reads.contains(&next)) {
                continue;
            }
            let Some(files) = node.job_files.hold(&candidate) else { continue };
            let target = match record::read(&candidate) {
                Ok(target) => target,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if matches(&target, &job.owner, &child) {
                missing.push((candidate, files, next));
            }
        }
    }
    Ok(missing)
}

pub(super) fn candidates(
    node: &Node,
    job: &JobId,
    origin: Option<&OnOrigin>,
    owner: &fastctx_types::run::JobOwner,
) -> io::Result<Vec<PathBuf>> {
    if let Some(origin) = origin {
        return Ok(vec![node.world.job_record(&node.names(), origin.town.as_ref(), owner, job)]);
    }
    let mut paths = Vec::new();
    for parent in node.world.job_parents()? {
        for directory in crate::retention::entries(&parent)? {
            paths.push(directory.join("jobs").join(format!("{job}.json")));
        }
    }
    Ok(paths)
}

pub(super) fn matches(
    target: &fastctx_types::run::JobRecord,
    owner: &fastctx_types::run::JobOwner,
    child: &Child,
) -> bool {
    target.owner == *owner
        && target.id == child.completed.job
        && (child.completed.output.is_empty() || target.output == child.completed.output)
        && child
            .origin
            .as_ref()
            .is_none_or(|origin| target.on.as_ref().and_then(|on| on.request.as_ref()) == origin.request.as_ref())
}
