//! Delivering repair through operations shipped before repair was introduced.
//! Only official packages leave the helper; execution is the target owner's.

use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, MachineName, ResidentId,
    access::{Acting, Purpose, Written},
    remote::{RemoteCall, RemoteReply},
    repair::Report,
    run::{ExitStatus, OutputPiece, RunRequest, RunState},
    update::{RepairDue, UpdateCall, ViewCall},
};
use std::sync::Arc;

const ROOT: &str = r#"set -e
root="${FASTCTX_HOME:?the node did not provide its World directory}"
if command -v cygpath >/dev/null 2>&1; then root=$(cygpath -m "$root"); fi
printf 'FASTCTX_ROOT=%s\n' "$root"
"#;

const EXECUTE: &str = r#"set -e
root="${FASTCTX_HOME:?the node did not provide its World directory}"
if command -v cygpath >/dev/null 2>&1; then root=$(cygpath -u "$root"); fi
version=@VERSION@
target=@TARGET@
package="$root/updates/packages/v$version/$target"
mkdir -p "$root/updates/packages/v$version"
printf '%s' @MANIFEST@ > "$root/updates/packages/v$version/latest.json"
work=$(mktemp -d "$root/updates/repair-delivery.XXXXXX")
trap 'rm -rf "$work"' EXIT
case "$target" in
  *windows*)
    archiver="$(cygpath -u "$SYSTEMROOT")/System32/tar.exe"
    "$archiver" -xf "$(cygpath -w "$package")" -C "$(cygpath -w "$work")" fastctx.exe
    program="$work/fastctx.exe" ;;
  *) tar -xzf "$package" -C "$work" fastctx; program="$work/fastctx" ;;
esac
@ACTION@
"#;

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(super) async fn help(
    node: &Arc<Node>,
    link: &Arc<Link>,
    machine: &MachineName,
    owner: &AccountName,
    due: &RepairDue,
) -> Result<(), String> {
    if !arranged(link, machine, &due.id) {
        return Ok(());
    }
    let acting = Acting::one(link.hub().clone(), owner.clone());
    let machine = Written { name: machine.clone(), hub: Some(link.hub_name()) };
    let current = super::machine(node, &acting, machine.clone(), UpdateCall::State).await.map_err(|e| e.to_string())?;
    if current.repair_supported {
        return Ok(());
    }
    let target = current.kernel_target;
    if !fastctx_types::update::target_name(&target) || !target.starts_with(fastctx_types::update::KERNEL_PREFIX) {
        return Err("legacy machine did not report a valid command-line package target".into());
    }
    let Some(output) =
        run(node, &acting, &machine, format!("{ROOT}mkdir -p \"$root/updates/packages/v{}\"\n", due.version)).await?
    else {
        return Ok(());
    };
    let root = output
        .lines()
        .find_map(|line| line.strip_prefix("FASTCTX_ROOT="))
        .ok_or("legacy node did not report its World directory")?;
    let manifest = match &due.manifest {
        Some(manifest) => manifest.clone(),
        None => super::manifest_of(node, &due.version).await?,
    };
    let _lease = node.updates.packages.retain(&due.version);
    let mut targets = vec![target.clone()];
    if let Some(app) = &current.app {
        targets.push(app.target.clone());
    }
    for target in targets {
        if !arranged(link, &machine.name, &due.id) {
            return Ok(());
        }
        super::bring(node, &manifest, &target).await.map_err(|e| e.why)?;
        if !arranged(link, &machine.name, &due.id) {
            return Ok(());
        }
        let destination = format!("{}/updates/packages/v{}/{target}", root.trim_end_matches('/'), due.version);
        crate::copy::update::send_package(node, &acting, &manifest, &target, &machine, &destination).await?;
    }
    let script = EXECUTE
        .replace("@VERSION@", &quote(&due.version))
        .replace("@TARGET@", &quote(&target))
        .replace("@MANIFEST@", &quote(&serde_json::to_string(&manifest).map_err(|e| e.to_string())?));
    let read = script.replace("@ACTION@", "printf 'FASTCTX_REPAIR='\n\"$program\" renew repair-state --dir \"$root\"");
    let Some(output) = run(node, &acting, &machine, read).await? else { return Ok(()) };
    let report = output
        .lines()
        .find_map(|line| line.strip_prefix("FASTCTX_REPAIR="))
        .ok_or("legacy repair inspection returned no receipt")?;
    let report: Option<Report> =
        serde_json::from_str(report).map_err(|e| format!("reading legacy repair receipt: {e}"))?;
    if let Some(report) = report {
        if report.id == due.id {
            if report.finished.is_none() || report.success || due.observed != Some(report.started) {
                link.network_updates(
                    owner,
                    ViewCall::RepairResult { machine: machine.name.clone(), result: Box::new(report) },
                )
                .await?;
                return Ok(());
            }
        } else if report.finished.is_none() {
            return Ok(());
        }
    }
    if !arranged(link, &machine.name, &due.id) {
        return Ok(());
    }
    let action = format!(
        "\"$program\" renew repair --background --dir \"$root\" --package \"$package\" --version \"$version\" --id {} --by {}",
        quote(&due.id),
        quote(&due.by.to_string())
    );
    if let Some(output) = run(node, &acting, &machine, script.replace("@ACTION@", &action)).await? {
        node.log.line(format_args!("legacy repair {} accepted on {}: {}", due.id, machine, output.trim()));
    }
    Ok(())
}

fn arranged(link: &Link, machine: &MachineName, id: &str) -> bool {
    matches!(link.status(), fastctx_net::link::Status::Online { .. })
        && link.current_versions().is_some_and(|network| network.repairs.get(machine).is_some_and(|due| due.id == id))
}

async fn run(node: &Arc<Node>, acting: &Acting, machine: &Written, script: String) -> Result<Option<String>, String> {
    let call = RemoteCall::Run {
        answer_receipts: false,
        dispatch: None,
        resident: ResidentId::try_from("world-0000").expect("system job identity"),
        town: None,
        request: RunRequest { turn: None, call_id: None, script, cwd: None, writes_off: false, wait: Some(600) },
    };
    let reply = node
        .call(acting, machine, Purpose::Hands, call)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let RemoteReply::Run(reply) = reply else { return Err("legacy node did not return a command result".into()) };
    let said = reply
        .output
        .pieces
        .iter()
        .filter_map(|p| match p {
            OutputPiece::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    match reply.state {
        RunState::Ended { status: ExitStatus::Code(0) } | RunState::Exited { status: ExitStatus::Code(0), .. } => {
            Ok(Some(said))
        }
        RunState::Running => {
            node.log.line(format_args!("legacy repair delivery job {} on {} is still running", reply.job, machine));
            Ok(None)
        }
        state => Err(format!("repair delivery on {machine} ended as {state:?}: {}", said.trim())),
    }
}
