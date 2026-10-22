//! A managed machine of a release before 1.0.1-beta.10 can sit for good on
//! an update it cannot take: a handover its guardian refused (a program of
//! 1.0.1-beta.4 or beta.6 carried no signature of its own) keeps the gate a
//! newer program needs, and the code that decides there is that release's,
//! which nothing sent to it changes. The node on the hub's own machine,
//! logged in to the account the machine is registered under, does there what
//! its owner may do (`world-upgrade.md`, 换版不要人确认): has it bring the
//! newer release, then runs that release's own program there, which takes
//! the stuck handover over (`fastctx_install::renewal::stage`); the
//! machine's guardian then takes the newer signed program by itself. Nobody
//! is asked: the network's policy says its managed machines update by
//! themselves. A machine of beta.10 or later takes over such a handover
//! itself and is left alone.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{ExitStatus, OutputPiece, RunRequest, RunState};
use fastctx_types::update::{Channel, UpdateCall, release};
use fastctx_types::{HubName, MachineName, ResidentId};

use crate::Node;

/// The first release whose staging takes over a stuck handover itself.
const TAKES_OVER: &str = "1.0.1-beta.10";
/// How long a machine is left to update by itself before it is helped.
const PATIENCE: Duration = Duration::from_secs(15 * 60);
/// How soon a machine helped is helped again, while it still runs the old.
const AGAIN: Duration = Duration::from_secs(60 * 60);
/// How often the machines are looked over.
const LOOK: Duration = Duration::from_secs(60);
/// Who the help runs as there: the World itself, on the hub's machine.
const HELPER: &str = "world-0000";

/// Takes the newer release's program out of the package for the machine's
/// own system, which the machine brought, and stages it with that program,
/// which ends a coordinator only holding the gate and takes over. Run by the
/// machine's own shell (bash, Git for Windows' on Windows) in its World's
/// environment: `FASTCTX_HOME` where the World has one, else the home the
/// node gives its commands. `@VERSION@` is the release, `@TARGET@` the
/// package the machine says its World program comes from.
const SCRIPT: &str = r#"version='@VERSION@'
target='@TARGET@'
root="${FASTCTX_HOME:-$HOME/.fastctx}"
if command -v cygpath >/dev/null 2>&1; then root=$(cygpath -u "$root"); fi
package="$root/updates/packages/v$version/$target"
if [ ! -f "$package" ]; then echo "no package $target of $version here yet"; exit 3; fi
work=$(mktemp -d) || exit 4
trap 'rm -rf "$work"' EXIT
case "$target" in
  *windows*)
    archiver="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe"
    "$archiver" -xf "$(cygpath -w "$package")" -C "$(cygpath -w "$work")" fastctx.exe || exit 4
    program="$work/fastctx.exe" ;;
  *)
    tar -xzf "$package" -C "$work" fastctx || exit 4
    program="$work/fastctx" ;;
esac
"$program" renew stage --dir "$root" --package "$package"
"#;

/// A machine of one network.
type Member = (HubName, MachineName);

/// When each machine was first seen stuck, when it was last helped, and the
/// ones being helped now.
#[derive(Default)]
struct Seen {
    since: BTreeMap<Member, Instant>,
    helped: BTreeMap<Member, Instant>,
    helping: BTreeSet<Member>,
}

/// Looks the network's machines over once a minute while the node runs.
pub(super) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        let seen = Arc::new(Mutex::new(Seen::default()));
        loop {
            tokio::time::sleep(LOOK).await;
            let Some(node) = weak.upgrade() else { return };
            look(&node, &seen);
        }
    });
}

/// A machine to help: where it is, the account it is registered under, and
/// the release to bring it to.
struct Stuck {
    member: Member,
    acting: Acting,
    version: String,
}

/// Starts helping each machine that has waited long enough; each on its
/// own, so a slow one holds none of the others up.
fn look(node: &Arc<Node>, seen: &Arc<Mutex<Seen>>) {
    let stuck = stuck(node);
    let now = Instant::now();
    let mut state = seen.lock().expect("never poisoned");
    state.since.retain(|member, _| stuck.iter().any(|one| &one.member == member));
    for one in stuck {
        let since = *state.since.entry(one.member.clone()).or_insert(now);
        let helped = state.helped.get(&one.member).is_some_and(|at| now.duration_since(*at) < AGAIN);
        if now.duration_since(since) < PATIENCE || helped || state.helping.contains(&one.member) {
            continue;
        }
        state.helped.insert(one.member.clone(), now);
        state.helping.insert(one.member.clone());
        let (node, seen) = (node.clone(), seen.clone());
        tokio::spawn(async move {
            help(&node, &one).await;
            seen.lock().expect("never poisoned").helping.remove(&one.member);
        });
    }
}

/// The managed machines of an account logged in here, on a network whose
/// hub runs here, that run a release before `TAKES_OVER` while a newer
/// release from it on is known on their line, online now.
fn stuck(node: &Node) -> Vec<Stuck> {
    let mut found = Vec::new();
    for link in node.links() {
        let Some(network) = link.versions() else { continue };
        let view = link.view();
        let Some(me) = view.me().map(|member| member.name.clone()) else { continue };
        if !network.machines.get(&me).is_some_and(|running| running.hub_here) || !network.policy.managed_auto {
            continue;
        }
        for member in &view.directory.members {
            let Some(owner) = member.home.as_ref() else { continue };
            if !member.managed || !member.online || member.name == me || !view.directory.logged_in(&me, owner) {
                continue;
            }
            let Some(running) = network.machines.get(&member.name).filter(|running| !running.away) else { continue };
            if release(&running.kernel) >= release(TAKES_OVER) {
                continue;
            }
            let Some(offer) = network.on(Channel::of(&running.kernel)) else { continue };
            if release(&offer.version) < release(TAKES_OVER) || release(&offer.version) <= release(&running.kernel) {
                continue;
            }
            found.push(Stuck {
                member: (link.hub_name(), member.name.clone()),
                acting: Acting::one(link.hub().clone(), owner.clone()),
                version: offer.version.clone(),
            });
        }
    }
    found
}

/// Has `stuck` bring its release, which it may stage itself; where its own
/// staging is refused, stages it there with the release's own program.
async fn help(node: &Arc<Node>, stuck: &Stuck) {
    let Stuck { member: (hub, name), acting, version } = stuck;
    let machine = Written { name: name.clone(), hub: Some(hub.clone()) };
    let asked = UpdateCall::Kernel { version: Some(version.clone()) };
    // A stuck machine brings the package and then refuses to stage it; the
    // package its World program comes from is the one it says it updates
    // from, which it says when asked how it stands.
    let state = match super::machine(node, acting, machine.clone(), asked).await {
        Ok(state) if state.renewing.as_ref().is_some_and(|renewing| &renewing.version == version) => {
            node.log.line(format_args!("{name} takes {version} by itself"));
            return;
        }
        Ok(state) => Ok(state),
        Err(failure) => {
            node.log.line(format_args!("{name} did not take {version} by itself: {failure}"));
            super::machine(node, acting, machine.clone(), UpdateCall::State).await
        }
    };
    let target = match state {
        Ok(state) => state.kernel_target,
        Err(failure) => {
            node.log.line(format_args!("{name} did not say how it stands, so {version} is not staged there: {failure}"));
            return;
        }
    };
    let resident = ResidentId::try_from(HELPER).expect("a resident id");
    let request = RunRequest {
        call_id: None,
        script: SCRIPT.replace("@VERSION@", version).replace("@TARGET@", &target),
        cwd: None,
        writes_off: false,
        wait: Some(600),
    };
    let call = RemoteCall::Run { resident, town: None, request };
    match node.call(acting, &machine, Purpose::Hands, call).await {
        Ok(Ok(RemoteReply::Run(reply))) => {
            let said: String = reply
                .output
                .pieces
                .iter()
                .filter_map(|piece| match piece {
                    OutputPiece::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let outcome = match reply.state {
                RunState::Ended { status: ExitStatus::Code(0) } | RunState::Exited { status: ExitStatus::Code(0), .. } => {
                    "staged".to_string()
                }
                ref other => format!("did not stage ({other:?})"),
            };
            node.log.line(format_args!("{name}: {outcome} {version} with its own program; {}", said.trim()));
        }
        Ok(Ok(_)) => node.log.line(format_args!("{name} answered staging {version} with something else")),
        Ok(Err(failure)) => node.log.line(format_args!("staging {version} on {name}: {failure}")),
        Err(failure) => node.log.line(format_args!("staging {version} on {name}: {failure}")),
    }
}
