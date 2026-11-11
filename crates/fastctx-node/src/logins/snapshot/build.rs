//! Incremental projection passes with a shared deadline for external reads.
use super::{part, row};
use crate::{
    Node,
    logins::{self, Context, kept},
};
use fastctx_hosts::inventory::walk;
use fastctx_types::{
    AgentName,
    agents::{AgentHome, HomeFound, Integration},
    ipc::Failure,
    logins::{AgentLogins, Found, Login, MachineLogins},
};
use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

const ITEM: Duration = Duration::from_secs(2);
const TOTAL: Duration = Duration::from_secs(10);
const CHUNK: usize = 128;
pub(super) type Aliases = Vec<(PathBuf, jiff::Timestamp, Vec<String>)>;
pub(super) struct Built {
    pub value: MachineLogins,
    pub next: usize,
    pub more: bool,
    pub keys: Vec<String>,
}
struct Source {
    context: Arc<Context>,
    record: Arc<kept::Kept>,
    chosen: kept::Added,
}

pub(super) async fn read<T: Send + 'static>(
    node: &Arc<Node>,
    deadline: Instant,
    what: &'static str,
    work: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let within = deadline.saturating_duration_since(Instant::now()).min(ITEM);
    if within.is_zero() {
        return Err(Failure::new("the login projection reached its ten-second budget"));
    }
    part(node, what, within, work).await
}
fn side<'a>(value: &'a mut MachineLogins, agent: &str) -> &'a mut AgentLogins {
    match agent {
        "codex" => &mut value.codex,
        "claude" => &mut value.claude,
        _ => unreachable!("known agent"),
    }
}

fn unavailable(mut login: Login, reason: &str) -> Login {
    login.launcher = None;
    login.limits = fastctx_types::agents::Fact::failed(reason);
    login.models.clear();
    login.unattended = None;
    login.drift = Some(fastctx_types::logins::Drift::Changed {
        location: login.dir.display().to_string(),
        reason: format!("the connection state is the previous reading; {reason}"),
    });
    login
}
pub(super) fn failure(node: &Node, what: &str, error: &Failure) {
    node.log.line(format_args!("reading {what} for the login list: {}", error.message));
}
fn put(value: &mut AgentLogins, login: Login) {
    if login.integration == Integration::In {
        value.listed.push(login);
    } else {
        value.candidates.push(login);
    }
}

pub(super) async fn all(
    node: &Arc<Node>,
    prior: Option<&MachineLogins>,
    cursor: usize,
    previous_keys: &[String],
) -> Result<Built, Failure> {
    let deadline = Instant::now() + TOTAL;
    let loading = node.clone();
    let Source { context, record, chosen } = read(node, deadline, "reading the login record", move || {
        Ok(Source {
            context: Arc::new(Context::of(&loading)?),
            record: Arc::new(kept::read(&loading).map_err(|error| Failure::new(error.to_string()))?),
            chosen: kept::chosen(&loading).map_err(|error| Failure::new(error.to_string()))?,
        })
    })
    .await?;
    let mut value = MachineLogins {
        machine: node.machine(),
        scan: record.scan.clone(),
        codex: AgentLogins::default(),
        claude: AgentLogins::default(),
        by_login: true,
        others: prior.map(|prior| prior.others.clone()).unwrap_or_default(),
        folders: prior.map(|prior| prior.folders.clone()).unwrap_or_default(),
    };
    let mut previous = std::collections::HashMap::new();
    if let Some(prior) = prior {
        for agent in logins::AGENTS {
            let side = prior.of(agent).expect("a known agent");
            previous.extend(
                side.listed
                    .iter()
                    .chain(&side.candidates)
                    .map(|login| ((agent.to_string(), walk::key(&login.dir)), login)),
            );
        }
    }
    let mut work = Vec::new();
    let mut homes = Vec::new();
    for agent in logins::AGENTS {
        let (reading, ctx, kept) = (node.clone(), context.clone(), record.clone());
        let dirs =
            read(node, deadline, "reading the login directories", move || logins::dirs(&reading, &ctx, &kept, agent))
                .await;
        let dirs = match dirs {
            Ok(dirs) => dirs,
            Err(error) => {
                failure(node, "directories", &error);
                let mut dirs = vec![(context.default_of(agent).to_path_buf(), Found::Default)];
                dirs.extend(record.of(agent).logins.iter().map(|login| (login.dir.clone(), login.found)));
                dirs.extend(
                    chosen
                        .added
                        .iter()
                        .filter(|one| one.agent.as_str() == agent)
                        .map(|one| (one.dir.clone(), Found::Added)),
                );
                if let Some(old) = prior.and_then(|prior| prior.of(agent)) {
                    dirs.extend(old.listed.iter().chain(&old.candidates).map(|login| (login.dir.clone(), login.found)));
                }
                let mut keys = HashSet::new();
                dirs.retain(|(dir, _)| keys.insert(walk::key(dir)));
                dirs
            }
        };
        if dirs.len() > 4096 {
            return Err(Failure::new("the login directory list exceeds 4096 rows"));
        }
        side(&mut value, agent).programs = record.of(agent).programs.clone();
        if agent == "claude" {
            homes = dirs.iter().map(|(dir, _)| AgentHome { dir: dir.clone(), found: HomeFound::Added }).collect();
        }
        work.extend(dirs.into_iter().map(|(dir, found)| (agent, dir, found)));
    }
    let homes = Arc::new(homes);
    let reading = node.clone();
    let aliases = match read(node, deadline, "reading Claude model aliases", move || {
        Ok(fastctx_hosts::claude::exchange::aliases(reading.world.path()))
    })
    .await
    {
        Ok(aliases) => Some(Arc::new(aliases)),
        Err(error) => {
            failure(node, "Claude model aliases", &error);
            None
        }
    };
    let others = fastctx_install::agents::all();
    let count = work.len() + others.len();
    let keys: Vec<_> = work
        .iter()
        .map(|(agent, dir, _)| format!("{agent}/{}", walk::key(dir)))
        .chain(others.iter().map(|agent| format!("{}/", agent.name)))
        .collect();
    let start = if keys == previous_keys && cursor < count { cursor } else { 0 };
    let mut next = start;
    for index in start..count.min(start + CHUNK) {
        if Instant::now() >= deadline {
            break;
        }
        if index < work.len() {
            let (agent, dir, found) = &work[index];
            let old = previous.get(&(agent.to_string(), walk::key(dir))).copied();
            let login =
                match row::one(node, &context, &record, homes.clone(), aliases.clone(), agent, dir, *found, deadline)
                    .await
                {
                    Ok(login) => Some(login),
                    Err(error) => {
                        failure(node, "connection state", &error);
                        old.cloned().map(|login| unavailable(login, &error.message))
                    }
                };
            if let Some(login) = login {
                put(side(&mut value, agent), login);
            }
        } else {
            let agent = others[index - work.len()];
            let Ok(name) = AgentName::try_from(agent.name) else {
                next = index + 1;
                continue;
            };
            let (reading, ctx, kept) = (node.clone(), context.clone(), record.clone());
            let shown = read(node, deadline, "reading another agent's login", move || {
                logins::others::side(&reading, &ctx, &kept, agent, &[])
            })
            .await;
            match shown {
                Ok(shown) if !shown.listed.is_empty() || !shown.candidates.is_empty() => {
                    value.others.insert(name, shown);
                }
                Ok(_) => {
                    value.others.remove(&name);
                }
                Err(error) => {
                    failure(node, agent.name, &error);
                    if let Some(old) = value.others.get_mut(&name) {
                        for login in old.listed.iter_mut().chain(&mut old.candidates) {
                            *login = unavailable(login.clone(), &error.message);
                        }
                    }
                }
            }
        }
        next = index + 1;
    }
    // Rows outside this batch retain their last projection until their turn.
    for (index, (agent, dir, _)) in work.iter().enumerate() {
        if (start..next).contains(&index) {
            continue;
        }
        if let Some(login) = previous.get(&(agent.to_string(), walk::key(dir))) {
            put(side(&mut value, agent), (*login).clone());
        }
    }
    let known: HashSet<_> = work
        .iter()
        .map(|(_, dir, _)| walk::key(dir))
        .chain(
            value
                .others
                .values()
                .flat_map(|side| side.listed.iter().chain(&side.candidates).map(|login| walk::key(&login.dir))),
        )
        .collect();
    let (reading, kept) = (node.clone(), record.clone());
    match read(node, deadline, "reading folder search results", move || {
        Ok(logins::folders::shown(&reading, &kept, &chosen.folders, &known))
    })
    .await
    {
        Ok(folders) => value.folders = folders,
        Err(error) => failure(node, "folder searches", &error),
    }
    let positions: std::collections::HashMap<_, _> =
        work.iter().enumerate().map(|(index, (agent, dir, _))| ((agent.to_string(), walk::key(dir)), index)).collect();
    for agent in logins::AGENTS {
        let side = side(&mut value, agent);
        side.listed.sort_by_key(|login| positions.get(&(agent.to_string(), walk::key(&login.dir))).copied());
        side.candidates.sort_by_key(|login| positions.get(&(agent.to_string(), walk::key(&login.dir))).copied());
    }
    Ok(Built { value, next: if next < count { next } else { 0 }, more: next < count, keys })
}

pub(super) async fn quotas(
    node: &Arc<Node>,
    mut value: MachineLogins,
    changed: BTreeSet<(String, PathBuf)>,
) -> Result<MachineLogins, Failure> {
    let deadline = Instant::now() + TOTAL;
    let reading = node.clone();
    let record = Arc::new(
        read(node, deadline, "reading the quota record", move || {
            kept::read(&reading).map_err(|error| Failure::new(error.to_string()))
        })
        .await?,
    );
    for (agent, dir) in changed {
        let Some(side) = value.of_mut(&agent) else { continue };
        let Some(login) =
            side.listed.iter_mut().chain(&mut side.candidates).find(|login| walk::key(&login.dir) == walk::key(&dir))
        else {
            continue;
        };
        let (reading, kept, name) = (node.clone(), record.clone(), agent.clone());
        login.limits = read(node, deadline, "reading updated login usage", move || {
            Ok(if name == "codex" {
                logins::limits::of(&reading, &kept, &dir)
            } else {
                logins::claude_limits(&reading, &dir)
            })
        })
        .await
        .unwrap_or_else(|error| fastctx_types::agents::Fact::failed(error.message));
    }
    Ok(value)
}
