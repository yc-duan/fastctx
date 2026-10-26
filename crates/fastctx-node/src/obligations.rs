//! Durable outcomes and retry reasons for work that can outlive its sender.
//! Only authoritative absence or an explicit permanent refusal ends a debt;
//! a missing connection or unreadable record never proves either.

use std::io;
use std::path::Path;
use std::time::Duration;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::access::{Acting, HubId, Written};
use fastctx_types::ipc::{Failure, FailureCode};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Delivered,
    Impossible,
    Superseded,
}

#[derive(Serialize, Deserialize)]
struct Finished {
    at: Timestamp,
    digest: String,
    outcome: Outcome,
    reason: String,
}

pub(crate) fn finished(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    Ok(fastctx_install::renewal::read_json::<Finished>(&path.with_extension("done"))?
        .is_some_and(|done| done.digest == fastctx_hands::files::content_hash(bytes)))
}

/// The caller serializes changes to this pending record. Changed contents
/// remain pending, even when a previous version just completed remotely.
pub(crate) fn finish(node: &Node, path: &Path, bytes: &[u8], outcome: Outcome, reason: &str) -> io::Result<bool> {
    let done = Finished {
        at: Timestamp::now(),
        digest: fastctx_hands::files::content_hash(bytes),
        outcome,
        reason: reason.into(),
    };
    fastctx_install::renewal::write_json(&path.with_extension("done"), &done)?;
    match std::fs::read(path) {
        Ok(current) if current == bytes => std::fs::remove_file(path)?,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if matches!(outcome, Outcome::Impossible) {
        node.log.line(format_args!("delivery ended for {}: {reason}", path.display()));
    }
    match std::fs::remove_file(path.with_extension("retry")) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    Ok(true)
}

pub(crate) fn retry(node: &Node, path: &Path, reason: &str) -> io::Result<()> {
    static UNWRITABLE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, String>>> =
        std::sync::LazyLock::new(Default::default);
    let marker = path.with_extension("retry");
    let written = (|| -> io::Result<bool> {
        match std::fs::read_to_string(&marker) {
            Ok(previous) if previous == reason => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        ensure_private_dir(marker.parent().expect("retry directory"))?;
        publish(&marker, reason.as_bytes(), Permissions::Private)?;
        Ok(true)
    })();
    match written {
        Ok(false) => {}
        Ok(true) => {
            UNWRITABLE.lock().expect("never poisoned").remove(path);
            node.log.line(format_args!("delivery pending for {}: {reason}", path.display()));
        }
        Err(error) => {
            let message = format!("{reason}; keeping its retry reason failed: {error}");
            let changed =
                UNWRITABLE.lock().expect("never poisoned").insert(path.to_path_buf(), message.clone()).as_ref()
                    != Some(&message);
            if changed {
                node.log.line(format_args!("delivery pending for {}: {message}", path.display()));
            }
        }
    }
    Ok(())
}

/// A local conversation receives the same durable failure fact once, even
/// if settling the failed obligation is retried after a restart.
pub(crate) fn tell_failure(
    node: &std::sync::Arc<Node>,
    owner: &fastctx_types::Address,
    path: &Path,
    text: String,
) -> io::Result<()> {
    if node.is_me(&owner.machine) && node.residents.knows(&owner.resident) {
        let id = crate::speech::fixed_id("delivery-failed", &[&path.to_string_lossy()]);
        crate::received::keep(
            node,
            &owner.resident,
            fastctx_types::residents::Delivery {
                id: id.clone(),
                at: Timestamp::now(),
                notice: fastctx_types::residents::Notice::World {
                    fact: fastctx_types::residents::WorldFact::DeliveryFailed { id },
                    text,
                },
            },
            crate::speech::Owed::Nobody,
        )?;
    }
    Ok(())
}

pub(crate) fn permanent(failure: &Failure) -> bool {
    use fastctx_types::authorization::DenialReason;
    matches!(failure.code, Some(FailureCode::Deleted | FailureCode::OutputNotRetained | FailureCode::SourceMissing))
        || failure.is_refused()
            && !matches!(failure.denial, Some(DenialReason::ViewUnavailable | DenialReason::Unknown))
}

pub(crate) fn hub_left(node: &Node, hub: &HubId) -> io::Result<bool> {
    let net = crate::world::hub_dir(&node.world, hub);
    match fastctx_net::link::state::read::<fastctx_net::link::state::Ended>(&net.join("ended.json")) {
        Ok(_) => return Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if net.join("membership.json").try_exists()? {
        return Ok(false);
    }
    let prefix = format!("net-left-{hub}-");
    Ok(crate::retention::entries(node.world.path())?.iter().any(|path| {
        path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            && path.join("membership.json").is_file()
    }))
}

/// Absence is authoritative only in the current view of every usable route.
/// The reason describes this account's view, not a claim about the computer
/// having ceased to exist in anyone else's network.
pub(crate) fn impossible(node: &Node, acting: &Acting, destination: &Written) -> io::Result<Option<String>> {
    if node.is_here(destination) {
        return Ok(None);
    }
    let links = match node.locate(destination) {
        Ok(links) => links,
        Err(_) => node.links(),
    };
    let mut authoritative = false;
    for link in links {
        if destination.hub.as_ref().is_some_and(|hub| *hub != link.hub_name())
            || node.accounts_on(&link, acting).is_empty()
        {
            continue;
        }
        let view = match crate::authorization::current_view(&link) {
            Ok(view) => view,
            Err(_) => return Ok(None),
        };
        authoritative = true;
        if view.member(&destination.name).is_some() {
            return Ok(None);
        }
    }
    if authoritative {
        return Ok(Some(format!("{} is no longer in this account's current network view", destination.name)));
    }
    if let Acting::Accounts { accounts } = acting
        && !accounts.is_empty()
        && accounts
            .iter()
            .map(|account| hub_left(node, &account.hub))
            .collect::<io::Result<Vec<_>>>()?
            .iter()
            .all(|left| *left)
    {
        return Ok(Some("every network this work belongs to has been left or removed".into()));
    }
    Ok(None)
}

/// Terminal markers expire independently of pending work and never keep an
/// archived conversation alive. Transport receipt retention is separate.
pub(crate) fn collect(node: &Node) -> io::Result<()> {
    let age = Duration::from_secs(30 * 24 * 60 * 60);
    for book in
        ["on-stops", "run-stops", "job-deliveries", "received-controls", "received", "on-deliveries", "run-receipts"]
    {
        for directory in node.world.book_directories(book)? {
            for path in crate::retention::entries(&directory)? {
                let terminal =
                    matches!(path.extension().and_then(|ext| ext.to_str()), Some("done" | "taken" | "stopped"))
                        || book == "run-receipts";
                if terminal && path.metadata()?.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= age) {
                    std::fs::remove_file(path)?;
                }
            }
        }
    }
    for path in crate::retention::entries(&node.world.path().join("forget-outcomes"))? {
        if path.extension().is_some_and(|extension| extension == "done")
            && path.metadata()?.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= age)
        {
            std::fs::remove_file(path)?;
        }
    }
    for kept in node.world.kept()? {
        for path in crate::retention::entries(&kept.directory.outbox())? {
            if matches!(path.extension().and_then(|extension| extension.to_str()), Some("done" | "attempt" | "retry"))
                && !path.with_extension("json").try_exists()?
                && path.metadata()?.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= age)
            {
                std::fs::remove_file(path)?;
            }
        }
    }
    crate::control_posts::collect(node, age)?;
    crate::run_stops::collect(node, age)?;
    Ok(())
}

/// Authoritative removals settle remote-running records without waiting for
/// a result that can no longer arrive through any of their account's hubs.
pub(crate) fn start(node: &std::sync::Arc<Node>) {
    let node = node.clone();
    let mut ways = node.ways.subscribe();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            ways.borrow_and_update();
            let result = retire_elsewhere(&node);
            if let Err(error) = &result {
                let _ = retry(&node, &node.world.path().join("remote-obligations"), &error.to_string());
            }
            let pending = result.unwrap_or(true);
            if !pending {
                again.reset();
            }
            tokio::select! {
                () = tokio::time::sleep(again.next()), if pending => {}
                _ = ways.changed() => {}
            }
        }
    });
}

fn retire_elsewhere(node: &std::sync::Arc<Node>) -> io::Result<bool> {
    let mut pending = false;
    for kept in node.world.kept()? {
        let fastctx_types::layout::Kind::Home(resident) = kept.kind else { continue };
        let Some((about, _)) = node.residents.about(&resident) else { continue };
        for path in crate::retention::entries(&kept.directory.elsewhere())? {
            if path.extension().is_some_and(|extension| extension == "done")
                && path
                    .metadata()?
                    .modified()?
                    .elapsed()
                    .is_ok_and(|elapsed| elapsed >= Duration::from_secs(30 * 24 * 60 * 60))
            {
                std::fs::remove_file(&path)?;
            }
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let result = (|| -> io::Result<bool> {
                let bytes = std::fs::read(&path)?;
                let elsewhere: crate::here::Elsewhere = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                let Some(reason) = impossible(node, &about.acting, &elsewhere.machine)? else { return Ok(false) };
                let owner = fastctx_types::Address { machine: node.machine(), resident: resident.clone() };
                tell_failure(
                    node,
                    &owner,
                    &path,
                    format!("The end of {} on {} cannot be delivered: {reason}.", elsewhere.job, elsewhere.machine),
                )?;
                finish(node, &path, &bytes, Outcome::Impossible, &reason)
            })();
            match result {
                Ok(done) => pending |= !done,
                Err(error) => {
                    pending = true;
                    retry(node, &path, &error.to_string())?;
                }
            }
        }
    }
    Ok(pending)
}

pub(crate) fn control_impossible(node: &Node, acting: &Acting, destination: &Written) -> io::Result<Option<String>> {
    if let Some(reason) = impossible(node, acting, destination)? {
        return Ok(Some(reason));
    }
    if node.is_here(destination) {
        return Ok(None);
    }
    let Ok(links) = node.locate(destination) else { return Ok(None) };
    let mut refusals = Vec::new();
    for link in links {
        let view = match crate::authorization::current_view(&link) {
            Ok(view) => view,
            Err(_) => return Ok(None),
        };
        for account in node.accounts_on(&link, acting) {
            match view.authorize(
                &account,
                fastctx_net::authorization::Action::Call {
                    from: &view.me,
                    to: &destination.name,
                    purpose: fastctx_types::access::Purpose::Hands,
                },
            ) {
                Ok(_) => return Ok(None),
                Err(denial) if denial.reason == fastctx_types::authorization::DenialReason::ViewUnavailable => {
                    return Ok(None);
                }
                Err(denial) => refusals.push(denial.message),
            }
        }
    }
    Ok((!refusals.is_empty()).then(|| refusals.join("; ")))
}
