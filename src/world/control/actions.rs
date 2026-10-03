//! Execution of explicit control operations, with partial success preserved in receipts.

use super::{Action, Failure, Receipt, Reply, ServiceAction, snapshot};
use crate::control::paths::ControlPaths;
use crate::world::node::admin::AdminRequest;
use crate::world::{WorldPaths, enroll, load_config, node, notes};
use std::path::Path;
use std::time::Duration;

const ADMIN_WAIT: Duration = Duration::from_secs(40);

pub(super) async fn execute(paths: &ControlPaths, action: Action) -> Result<Reply, Failure> {
    if let Action::Snapshot { hub_data } = action {
        return Ok(Reply::Snapshot(Box::new(
            snapshot::read(paths, hub_data).await,
        )));
    }
    let world = WorldPaths::from_control(paths);
    let mut receipt = Receipt::default();
    match &action {
        Action::Snapshot { .. } => unreachable!(),
        Action::Init {
            hub,
            bootstrap,
            name,
            tags,
            network,
            interface,
            install_service,
        } => {
            ensure_not_enrolled(&world)?;
            let options = enroll::EnrollOptions {
                name: name.clone(),
                tags: tags.clone(),
                network: *network,
                interface: interface.clone(),
            };
            let result = enroll::bootstrap(paths, hub, bootstrap, options).await;
            return enrollment_result(paths, result, *install_service).await;
        }
        Action::Enroll {
            invite,
            name,
            tags,
            network,
            interface,
            install_service,
        } => {
            ensure_not_enrolled(&world)?;
            let options = enroll::EnrollOptions {
                name: name.clone(),
                tags: tags.clone(),
                network: *network,
                interface: interface.clone(),
            };
            let result = enroll::enroll(paths, invite, options).await;
            return enrollment_result(paths, result, *install_service).await;
        }
        Action::ExportInvite { path, text } => {
            crate::world::invite::Invite::parse(text).map_err(Failure::from)?;
            export_secret(path, text)?;
            receipt
                .facts
                .push(("file".into(), crate::paths::display_path(path)));
        }
        Action::HubRevoke { data, name } => {
            let message = crate::world::hub::revoke_from_cli(data, name)
                .map_err(|error| snapshot::mutation_failure(error.into(), &action))?;
            receipt.facts.push(("result".into(), message));
        }
        _ => {
            if load_config(&world).map_err(Failure::from)?.is_none() {
                return Err(Failure::new("This machine is not enrolled in a World."));
            }
            match &action {
                Action::Description { text } => {
                    notes::validate(text).map_err(Failure::from)?;
                    let status = snapshot::ask(&AdminRequest::Status, Duration::from_secs(3)).await;
                    if let Err(error) = &status
                        && error.kind != super::FailureKind::Unavailable
                    {
                        return Err(error.clone());
                    }
                    if status.is_ok() {
                        let data = mutate(
                            &AdminRequest::SetDescription { text: text.clone() },
                            &action,
                        )
                        .await?;
                        if let Some(error) = data.get("error").and_then(serde_json::Value::as_str) {
                            receipt.warnings.push(error.to_string());
                        }
                        receipt.facts.push(("description".into(), "saved".into()));
                        receipt.facts.push((
                            "sync".into(),
                            if data.get("connected").and_then(serde_json::Value::as_bool)
                                == Some(true)
                            {
                                "queued"
                            } else {
                                "waiting_for_connection"
                            }
                            .into(),
                        ));
                    } else {
                        notes::save_description(&world, text).map_err(Failure::from)?;
                        receipt.facts.push(("description".into(), "saved".into()));
                        receipt
                            .facts
                            .push(("sync".into(), "waiting_for_node".into()));
                    }
                }
                Action::PrivateNote { name, text } => {
                    notes::save_private_note(&world, name, text).map_err(Failure::from)?;
                    receipt.facts.push(("private_note".into(), name.clone()));
                }
                Action::Refresh => {
                    mutate(&AdminRequest::Refresh, &action).await?;
                    receipt.facts.push(("snapshot".into(), "refreshed".into()));
                }
                Action::Reconnect => {
                    mutate(&AdminRequest::Reconnect, &action).await?;
                    receipt
                        .facts
                        .push(("connection".into(), "reconnect_requested".into()));
                }
                Action::Events { since, limit } => {
                    let data = snapshot::ask(
                        &AdminRequest::Events {
                            since: *since,
                            limit: (*limit).clamp(1, 200),
                        },
                        ADMIN_WAIT,
                    )
                    .await?;
                    let events = serde_json::from_value(data).map_err(|error| {
                        Failure::new(format!("Cannot decode the event page: {error}"))
                    })?;
                    return Ok(Reply::Events(events));
                }
                Action::Invite { name, hours, hubs } => {
                    if let Some(name) = name {
                        crate::world::validate_node_name(name).map_err(Failure::from)?;
                    }
                    if !(1..=8760).contains(hours) {
                        return Err(Failure::invalid(
                            "Invite validity must be between 1 and 8760 hours.",
                        ));
                    }
                    for address in hubs {
                        crate::world::link::Endpoint::parse(address).map_err(Failure::from)?;
                    }
                    let data = mutate(
                        &AdminRequest::Invite {
                            name: name.clone(),
                            ttl_hours: *hours,
                            hubs: (!hubs.is_empty()).then(|| hubs.clone()),
                        },
                        &action,
                    )
                    .await?;
                    receipt.invite = Some(
                        data.as_str()
                            .ok_or_else(|| Failure::new("The node returned no invite string."))?
                            .to_string(),
                    );
                    receipt.facts.push(("hours".into(), hours.to_string()));
                    receipt.facts.push(("delivery".into(), "queued".into()));
                }
                Action::Revoke { name } => {
                    let _data =
                        mutate(&AdminRequest::Revoke { name: name.clone() }, &action).await?;
                    receipt.facts.push(("member".into(), name.clone()));
                }
                Action::Grant {
                    grant,
                    expected_revision,
                } => {
                    grant.validate().map_err(Failure::from)?;
                    let data = mutate(
                        &AdminRequest::Grant {
                            id: None,
                            principal: grant.principal.clone(),
                            nodes: grant.nodes.clone(),
                            verbs: grant.verbs.clone(),
                            expires: grant.expires.clone(),
                            delete: false,
                            expected_revision: Some(*expected_revision),
                        },
                        &action,
                    )
                    .await?;
                    grant_facts(&mut receipt, data);
                }
                Action::DeleteGrant {
                    id,
                    expected_revision,
                } => {
                    let data = mutate(
                        &AdminRequest::Grant {
                            id: Some(id.clone()),
                            principal: String::new(),
                            nodes: Vec::new(),
                            verbs: Vec::new(),
                            expires: None,
                            delete: true,
                            expected_revision: Some(*expected_revision),
                        },
                        &action,
                    )
                    .await?;
                    grant_facts(&mut receipt, data);
                }
                Action::Service { action } => {
                    receipt = manage_service(paths, *action).await?;
                }
                Action::Leave => {
                    return leave(paths).await;
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(Reply::Receipt(receipt))
}

fn ensure_not_enrolled(world: &WorldPaths) -> Result<(), Failure> {
    if load_config(world).map_err(Failure::from)?.is_some() {
        Err(Failure::new(
            "This machine is already enrolled. Refresh World to see its current enrollment.",
        ))
    } else {
        Ok(())
    }
}

async fn mutate(request: &AdminRequest, action: &Action) -> Result<serde_json::Value, Failure> {
    snapshot::ask(request, ADMIN_WAIT)
        .await
        .map_err(|failure| snapshot::mutation_failure(failure, action))
}

fn grant_facts(receipt: &mut Receipt, data: serde_json::Value) {
    if let Some(id) = data.get("id").and_then(serde_json::Value::as_str) {
        receipt.facts.push(("grant".into(), id.to_string()));
    }
    if let Some(revision) = data.get("revision").and_then(serde_json::Value::as_u64) {
        receipt
            .facts
            .push(("revision".into(), revision.to_string()));
    }
}

async fn enrollment_result(
    paths: &ControlPaths,
    result: Result<enroll::EnrollSummary, String>,
    install: bool,
) -> Result<Reply, Failure> {
    let mut receipt = Receipt::default();
    match result {
        Ok(summary) => {
            receipt.enrolled = true;
            receipt.facts = vec![
                ("member".into(), summary.name),
                ("world".into(), summary.world_id),
                ("hub".into(), summary.hub.join(", ")),
                ("tls".into(), summary.tls.as_str().into()),
            ];
        }
        Err(error) => match load_config(&WorldPaths::from_control(paths)) {
            Ok(Some(config)) => {
                receipt.enrolled = true;
                receipt.facts.push(("member".into(), config.name));
                receipt.warnings.push(error);
            }
            _ => return Err(Failure::new(error)),
        },
    }
    if install {
        match node::service::install(paths, None) {
            Ok(message) => {
                receipt.facts.push(("service".into(), message));
                if let Err(failure) = wait_for_node(true).await {
                    receipt.warnings.push(failure.detail);
                }
            }
            Err(error) => receipt.warnings.push(error),
        }
    } else {
        receipt
            .facts
            .push(("service".into(), "not_installed".into()));
    }
    Ok(Reply::Receipt(receipt))
}

async fn manage_service(paths: &ControlPaths, action: ServiceAction) -> Result<Receipt, Failure> {
    let installed = node::service::inspect(paths).map_err(Failure::from)?;
    if action == ServiceAction::Start {
        match snapshot::ask(&AdminRequest::Status, Duration::from_secs(2)).await {
            Ok(_) => {
                return Ok(Receipt {
                    facts: vec![(
                        "service".into(),
                        "The node is already running for this HOME and build.".into(),
                    )],
                    ..Receipt::default()
                });
            }
            Err(error) if error.kind == super::FailureKind::Unavailable => {}
            Err(error) => return Err(error),
        }
    }
    if action == ServiceAction::Install && !installed {
        stop_direct().await?;
    }
    let message = match action {
        ServiceAction::Install => node::service::install(paths, None),
        ServiceAction::Start if installed => node::service::start(paths),
        ServiceAction::Start => node::process::start(paths),
        ServiceAction::Restart if installed => node::service::restart(paths),
        ServiceAction::Restart => {
            stop_direct().await?;
            node::process::start(paths)
        }
        ServiceAction::Stop | ServiceAction::Uninstall => {
            if installed {
                if action == ServiceAction::Uninstall {
                    node::service::uninstall(paths)
                } else {
                    node::service::stop(paths)
                }
            } else {
                stop_direct().await?;
                Ok("Stopped the node running without a service registration.".to_string())
            }
        }
    }
    .map_err(Failure::from)?;
    let mut receipt = Receipt {
        facts: vec![("service".into(), message)],
        ..Receipt::default()
    };
    let expected = matches!(
        action,
        ServiceAction::Install | ServiceAction::Start | ServiceAction::Restart
    );
    if let Err(failure) = wait_for_node(expected).await {
        receipt.warnings.push(failure.detail);
    }
    Ok(receipt)
}

async fn wait_for_node(expected: bool) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let result = snapshot::ask(&AdminRequest::Status, Duration::from_secs(1)).await;
        if (expected && result.is_ok())
            || (!expected
                && result
                    .as_ref()
                    .is_err_and(|error| error.kind == super::FailureKind::Unavailable))
        {
            return Ok(());
        }
        if let Err(error) = result.as_ref()
            && error.kind == super::FailureKind::Permission
        {
            return Err(error.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::new(if expected {
                "The service command completed, but this build's node did not answer within 15 seconds. Check its log, user account, HOME and executable version, then retry from Services."
            } else {
                "The stop command completed, but a node still answers. Check the service manager before leaving the World."
            }));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn stop_direct() -> Result<(), Failure> {
    match snapshot::ask(&AdminRequest::Shutdown, Duration::from_secs(3)).await {
        Ok(_) => wait_for_node(false).await,
        Err(error) if error.kind == super::FailureKind::Unavailable => Ok(()),
        Err(error) => Err(error),
    }
}

async fn leave(paths: &ControlPaths) -> Result<Reply, Failure> {
    let mut receipt = Receipt::default();
    match snapshot::ask(&AdminRequest::Leave, ADMIN_WAIT).await {
        Ok(_) => {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let confirmed = loop {
                if let Ok(data) = snapshot::ask(&AdminRequest::Status, Duration::from_secs(1)).await
                    && let Ok(status) =
                        serde_json::from_value::<crate::world::node::admin::NodeStatus>(data)
                    && matches!(status.link.state, crate::world::client::LinkState::Stopped { reason, .. } if reason.starts_with("revoked:"))
                {
                    break true;
                }
                if tokio::time::Instant::now() >= deadline {
                    break false;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            };
            if confirmed {
                receipt.facts.push(("hub_notice".into(), "revoked".into()));
            } else {
                receipt.warnings.push("The leave message was queued, but the hub did not confirm revocation before local cleanup. Another member should check and revoke this name if it remains.".into());
            }
        }
        Err(failure) => receipt.warnings.push(format!(
            "The hub was not notified: {} Another member must revoke this machine's name.",
            failure.detail
        )),
    }
    match node::service::inspect(paths) {
        Ok(true) => match node::service::uninstall(paths) {
            Ok(message) => receipt.facts.push(("service".into(), message)),
            Err(error) => receipt.warnings.push(error),
        },
        Ok(false) => {}
        Err(error) => receipt.warnings.push(error),
    }
    if let Err(error) = stop_direct().await {
        receipt.warnings.push(error.detail);
        receipt.warnings.push("Local enrollment was retained because the node may still be running. Stop it, then retry leaving.".into());
        return Ok(Reply::Receipt(receipt));
    }
    let world = WorldPaths::from_control(paths);
    crate::world::remove_config(&world).map_err(Failure::from)?;
    receipt.left = true;
    if let Err(error) = std::fs::remove_dir_all(&world.dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        receipt.warnings.push(format!(
            "Enrollment was removed, but {} could not be deleted: {error}",
            crate::paths::display_path(&world.dir)
        ));
    }
    receipt.facts.push(("enrollment".into(), "removed".into()));
    Ok(Reply::Receipt(receipt))
}

fn export_secret(path: &Path, text: &str) -> Result<(), Failure> {
    use std::io::Write;
    if !path.is_absolute() {
        return Err(Failure::invalid(
            "Choose an absolute path for the invite file.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Failure::invalid("The invite file needs a parent directory."))?;
    if !parent.is_dir() {
        return Err(Failure::invalid(
            "The destination directory does not exist. Choose an existing directory.",
        ));
    }
    let staging = tempfile::Builder::new()
        .prefix(".fastctx-invite-")
        .tempdir_in(parent)
        .map_err(|error| {
            Failure::new(format!(
                "Cannot create the invite staging directory: {error}"
            ))
        })?;
    crate::edit::private_storage::ensure_private_directory(staging.path(), "World invite export")
        .map_err(Failure::from)?;
    let mut temporary = tempfile::NamedTempFile::new_in(staging.path())
        .map_err(|error| Failure::new(format!("Cannot create the invite file: {error}")))?;
    temporary
        .write_all(text.as_bytes())
        .and_then(|()| temporary.write_all(b"\n"))
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| Failure::new(format!("Cannot write the invite file: {error}")))?;
    temporary.persist_noclobber(path).map_err(|error| {
        Failure::new(format!(
            "Cannot save the invite to {} (existing files are never overwritten): {}",
            crate::paths::display_path(path),
            error.error
        ))
    })?;
    Ok(())
}
