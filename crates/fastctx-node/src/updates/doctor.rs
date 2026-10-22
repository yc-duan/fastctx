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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{OutputPiece, RunRequest};
use fastctx_types::update::{Channel, UpdateCall, release};
use fastctx_types::{MachineName, ResidentId};

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

/// Takes the newer release's program out of the package the machine
/// brought and stages it with that program, which ends a coordinator only
/// holding the gate and takes over. Run by the machine's own shell (bash,
/// Git for Windows' on Windows); `@VERSION@` is the release.
const SCRIPT: &str = r#"version='@VERSION@'
root="$HOME/.fastctx"
dir="$root/updates/packages/v$version"
name=$(ls "$dir" 2>/dev/null | grep '^fastctx-' | head -n 1)
if [ -z "$name" ]; then echo "no package of $version here yet"; exit 3; fi
package="$dir/$name"
work=$(mktemp -d)
case "$name" in
  *windows*)
    archiver="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe"
    "$archiver" -xf "$(cygpath -w "$package")" -C "$(cygpath -w "$work")" fastctx.exe || exit 4
    program="$work/fastctx.exe" ;;
  *)
    tar -xzf "$package" -C "$work" fastctx || exit 4
    program="$work/fastctx" ;;
esac
"$program" renew stage --dir "$root" --package "$package"
status=$?
rm -rf "$work"
exit $status
"#;

/// When each machine was first seen stuck, and when it was last helped.
#[derive(Default)]
struct Seen {
    since: BTreeMap<MachineName, Instant>,
    helped: BTreeMap<MachineName, Instant>,
}

/// Looks the network's machines over once a minute while the node runs.
pub(super) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        let mut seen = Seen::default();
        loop {
            tokio::time::sleep(LOOK).await;
            let Some(node) = weak.upgrade() else { return };
            look(&node, &mut seen).await;
        }
    });
}

/// A machine to help: where it is, the account it is registered under, and
/// the release to bring it to.
struct Stuck {
    machine: Written,
    acting: Acting,
    version: String,
}

async fn look(node: &Arc<Node>, seen: &mut Seen) {
    let stuck = stuck(node);
    let now = Instant::now();
    seen.since.retain(|machine, _| stuck.iter().any(|one| &one.machine.name == machine));
    for one in stuck {
        let since = *seen.since.entry(one.machine.name.clone()).or_insert(now);
        let helped = seen.helped.get(&one.machine.name).is_some_and(|at| now.duration_since(*at) < AGAIN);
        if now.duration_since(since) < PATIENCE || helped {
            continue;
        }
        seen.helped.insert(one.machine.name.clone(), now);
        help(node, &one).await;
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
                machine: Written { name: member.name.clone(), hub: None },
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
    let Stuck { machine, acting, version } = stuck;
    let asked = UpdateCall::Kernel { version: Some(version.clone()) };
    match super::machine(node, acting, machine.clone(), asked).await {
        Ok(state) if state.renewing.as_ref().is_some_and(|renewing| &renewing.version == version) => {
            node.log.line(format_args!("{machine} takes {version} by itself"));
            return;
        }
        Ok(_) => {}
        Err(failure) => node.log.line(format_args!("{machine} did not take {version} by itself: {failure}")),
    }
    let resident = ResidentId::try_from(HELPER).expect("a resident id");
    let request = RunRequest {
        call_id: None,
        script: SCRIPT.replace("@VERSION@", version),
        cwd: None,
        writes_off: false,
        wait: Some(600),
    };
    let call = RemoteCall::Run { resident, town: None, request };
    match node.call(acting, machine, Purpose::Hands, call).await {
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
            node.log.line(format_args!(
                "staged {version} on {machine} with its own program: {:?}; {}",
                reply.state,
                said.trim()
            ));
        }
        Ok(Ok(_)) => node.log.line(format_args!("{machine} answered staging {version} with something else")),
        Ok(Err(failure)) => node.log.line(format_args!("staging {version} on {machine}: {failure}")),
        Err(failure) => node.log.line(format_args!("staging {version} on {machine}: {failure}")),
    }
}
