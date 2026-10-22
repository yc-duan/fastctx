use anyhow::Context as _;
use fastctx_install::uninstall::Installation;
use fastctx_types::{
    layout::WorldDir,
    run::{EndedHow, JobRecord, StopRequest},
};

pub(crate) async fn stop(installation: &Installation) -> anyhow::Result<Option<fastctx_types::ProcessIdentity>> {
    let world = WorldDir::under(&installation.root);
    let mut identity = None;
    match fastctx_client::Client::connect(&world, fastctx_client::command_peer()?).await {
        Ok(mut node) => {
            identity = Some(
                node.node_lifetime
                    .as_ref()
                    .context("the serving node did not identify its process lifetime")?
                    .process
                    .clone(),
            );
            match node.call(fastctx_types::Call::UninstallPrepare).await {
                Ok(_) => {}
                Err(fastctx_client::CallError::Broken(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(fastctx_client::ConnectError::NoNode { .. }) => {}
        Err(fastctx_client::ConnectError::Refused { mismatch: Some(_), .. }) => {
            identity = fastctx_client::renewal::process(&world).await?;
        }
        Err(error) => return Err(error.into()),
    }
    finish(identity.as_ref()).await?;
    for parent in world.job_parents()? {
        for owner in crate::entries(&parent)? {
            for job in crate::entries(&owner.path().join("jobs"))? {
                let path = job.path();
                if path.extension().is_none_or(|extension| extension != "json")
                    || path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .and_then(|stem| fastctx_types::JobId::try_from(stem).ok())
                        .is_none()
                {
                    continue;
                }
                let record: JobRecord = fastctx_hands::run::record::read(&path)?;
                if record.end.is_some() {
                    continue;
                }
                let stopped = fastctx_hands::run::stop(
                    &world,
                    &path,
                    None,
                    StopRequest {
                        by: "uninstall".into(),
                        reason: "fastctx is being uninstalled from this computer".into(),
                    },
                )
                .await?;
                anyhow::ensure!(
                    stopped.end.as_ref().is_some_and(|end| matches!(end.how, EndedHow::Finished | EndedHow::Stopped)),
                    "job {} did not stop successfully",
                    stopped.id
                );
            }
        }
    }
    stop_claude_without_node(installation, &world)?;
    if world.path().try_exists()? {
        let _node = fastctx_platform::FileLock::acquire(&world.node_lock())?;
        let remaining = fastctx_hands::run::program::uses(&world)?;
        anyhow::ensure!(
            remaining.is_empty(),
            "{}",
            remaining.iter().map(|held| held.reason()).collect::<Vec<_>>().join("; ")
        );
    }
    let owned = images(&installation.root)?;
    let invoking = fastctx_platform::host_process::parent_identity()?;
    for process in fastctx_platform::owned_images::processes(&owned)? {
        if process == invoking || process.pid == std::process::id() {
            continue;
        }
        // A hub on this computer keeps serving until its service is taken
        // away, after the logins are withdrawn from it.
        if fastctx_platform::owned_images::serves_hub(&process)? {
            continue;
        }
        fastctx_platform::terminate::terminate(&process)?;
        fastctx_platform::host_process::exited(&process).await?;
    }
    Ok(None)
}

fn images(root: &std::path::Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut paths = vec![root.join("bin").join(fastctx_install::renewal::EXECUTABLE)];
    for parent in [root.join("world/bin"), root.join("renewal/images")] {
        for entry in crate::entries(&parent)? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(name.len(), 24 | 64) && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                paths.push(entry.path().join(fastctx_install::renewal::EXECUTABLE));
            }
        }
    }
    Ok(paths)
}

fn stop_claude_without_node(installation: &Installation, world: &WorldDir) -> anyhow::Result<()> {
    use fastctx_hosts::claude::background;
    let mut sessions = Vec::new();
    for about_file in crate::inventory::abouts(world)? {
        if let Some(about) = crate::read::<fastctx_types::residents::About>(&about_file)?
            && about.world_started
            && about.host.agent.as_str() == "claude"
        {
            sessions.push((
                about.home.unwrap_or_else(|| installation.claude_home.clone()),
                about.session.context("a World-started Claude session has no identity")?,
            ));
        }
    }
    // An earlier layout kept a resident's record in each town it lived in.
    sessions.sort();
    sessions.dedup();
    if sessions.is_empty() {
        return Ok(());
    }
    let program = background::find(std::env::var_os("PATH").as_ref())
        .context("Claude Code is unavailable; World-started Claude sessions cannot be checked or stopped")?;
    for (home, session) in sessions {
        let mut environment = fastctx_platform::world_process_environment()?;
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"));
        environment.push(("CLAUDE_CONFIG_DIR".into(), home.into_os_string()));
        let list = || -> anyhow::Result<Vec<serde_json::Value>> {
            let text = background::claude(
                &program,
                &["agents".into(), "--json".into(), "--all".into()],
                &installation.home,
                &environment,
            )
            .map_err(anyhow::Error::msg)?;
            Ok(serde_json::from_str(&text)?)
        };
        let agents = list()?;
        let found = agents
            .iter()
            .filter(|agent| agent["kind"] == "background" && agent["sessionId"].as_str() == Some(&session))
            .collect::<Vec<_>>();
        anyhow::ensure!(found.len() <= 1, "Claude Code returned ambiguous session ownership");
        if let Some(agent) = found.first()
            && agent["pid"].as_u64().is_some()
        {
            let id = agent["id"].as_str().context("Claude Code did not identify its background session")?;
            anyhow::ensure!(
                id.len() == 8 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Claude Code returned an invalid management id"
            );
            background::claude(&program, &["stop".into(), id.into()], &installation.home, &environment)
                .map_err(anyhow::Error::msg)?;
            anyhow::ensure!(
                !list()?
                    .iter()
                    .any(|agent| agent["sessionId"].as_str() == Some(&session) && agent["pid"].as_u64().is_some()),
                "Claude still reports the World session running"
            );
        }
    }
    Ok(())
}

pub(crate) async fn finish(identity: Option<&fastctx_types::ProcessIdentity>) -> anyhow::Result<()> {
    if let Some(identity) = identity {
        if !fastctx_platform::has_ended(identity)? {
            fastctx_platform::terminate::terminate(identity)?;
        }
        fastctx_platform::host_process::exited(identity).await?;
    }
    Ok(())
}
