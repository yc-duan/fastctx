//! Prompt-carrying wakes use the same durable admission as control wakes.

use super::{
    Command, command, continuing, copies, isolation, oldest, plugin_gone, resume_dir, reword, startup, trust, wake_once,
};
use crate::Node;
use fastctx_hosts::claude::background;
use fastctx_types::{ResidentId, ipc::Failure, residents::Landed};
use std::{path::Path, sync::Arc};

pub(crate) async fn wake(
    node: &Arc<Node>,
    resident: &ResidentId,
    home: Option<&Path>,
    session: &str,
    words: String,
    id: fastctx_types::NoticeId,
) -> Result<Landed, crate::lifecycle::wake::WakeFailure> {
    let launch = match wake_once::begin(node, resident, home, session).await? {
        wake_once::Admission::Launch(launch) => launch,
        wake_once::Admission::Present | wake_once::Admission::Waiting => return Err(wake_once::waiting().into()),
    };
    let wake_once::Launch { intent, active: _active } = *launch;
    let result = wake_inner(node, resident, home, session, words, id).await;
    let (started, accepted) = match &result {
        Ok(_) => (true, true),
        Err(error) => (error.attempted, false),
    };
    wake_once::finish(node, resident, intent, started, accepted)
        .map_err(|failure| crate::lifecycle::wake::WakeFailure { failure, attempted: started, permanent: false })?;
    result
}

async fn wake_inner(
    node: &Arc<Node>,
    resident: &ResidentId,
    home: Option<&Path>,
    session: &str,
    words: String,
    id: fastctx_types::NoticeId,
) -> Result<Landed, crate::lifecycle::wake::WakeFailure> {
    if let Err(error) = reword(node, resident) {
        node.log.line(format_args!("rewording {resident}'s instructions before it wakes: {error}"));
    }
    if let Some(source) = isolation::source(node, resident)? {
        crate::inbox::unbind(&node.world.conversation(resident), &id)
            .map_err(|error| Failure::new(error.to_string()))?;
        crate::lifecycle::retry::attempting(node, resident, &id)?;
        let short = isolation::continue_in(node, resident, &source, &words).await.map_err(|error| {
            crate::lifecycle::wake::WakeFailure {
                failure: Failure::new(error.message),
                attempted: error.started,
                permanent: false,
            }
        })?;
        crate::inbox::rebind(&node.world.conversation(resident), &id, &short).map_err(|error| {
            crate::lifecycle::wake::WakeFailure {
                failure: Failure::new(error.to_string()),
                attempted: true,
                permanent: false,
            }
        })?;
        crate::lifecycle::retry::accepted(node, resident, &id)
            .map_err(|failure| crate::lifecycle::wake::WakeFailure { failure, attempted: true, permanent: false })?;
        startup::observe(node, resident, id.clone(), true);
        return Ok(Landed::Handed { id });
    }
    let mut launching = command(node, home)?;
    launching.cwd = resume_dir(node, session);
    oldest::check(node, &launching).await?;

    let launch_command = launching.clone();
    let Command { cwd, env, .. } = launching;
    if let Some(resident) = node.residents.resident_of_thread(session) {
        crate::profile::session::load_claude(node, &resident, false).await?;
    }
    let mut args = background::wake_args(session, &words);
    // A session the plugin was gone from keeps Claude Code's account of
    // itself this time (`plugin_gone`): the option goes before the words.
    let session_directory = node.world.path().join("claude").join(session);
    if plugin_gone::debugged(&session_directory) {
        let at = args.iter().position(|arg| arg == "--").unwrap_or(args.len());
        args.splice(
            at..at,
            [std::ffi::OsString::from("--debug-file"), session_directory.join(plugin_gone::DEBUG_LOG).into()],
        );
    }
    let active = wake_once::hold(node, resident).ok_or_else(wake_once::waiting)?;
    let (launch_node, launch_resident, launch_id) = (node.clone(), resident.clone(), id.clone());
    let printed = fastctx_platform::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        use crate::lifecycle::wake::WakeFailure;
        let _active = active;
        trust(&[cwd.as_path()], &env).map_err(Failure::new)?;
        crate::lifecycle::retry::attempting(&launch_node, &launch_resident, &launch_id)?;
        super::wake_command::run(&launch_node, &launch_command, &args).map_err(|error| WakeFailure {
            failure: Failure::new(error.message),
            attempted: error.started,
            permanent: false,
        })
    }))
    .await
    .map_err(|error| crate::lifecycle::wake::WakeFailure {
        failure: Failure::new(format!("waking the session: {error}")),
        attempted: true,
        permanent: false,
    })??;
    // Claude Code copied the session instead of waking it: the copy, which
    // has none of the World's settings, is stopped. A session still running
    // takes the words, which never reached it, when they are handed again;
    // one that does not run Claude Code would copy at every wake, so the
    // conversation goes on in a session of the World's (`continuing`).
    if let Some(copy) = copies::of(session, &printed.out) {
        copies::discard(node, home, &copy).await.map_err(|failure| crate::lifecycle::wake::WakeFailure {
            failure,
            attempted: true,
            permanent: false,
        })?;
        if wake_once::observe(node, home, session).await.is_ok_and(|seen| seen.resting) {
            let why = background::copy_note(&printed.told).unwrap_or("it said nothing of why");
            // The words go to the session it goes on in, which takes them as
            // it registers.
            let directory = node.world.conversation(resident);
            let continued = match crate::inbox::unbind(&directory, &id) {
                Ok(()) => continuing::start(node, resident, home, session, &words, why).await.and_then(|short| {
                    crate::inbox::rebind(&directory, &id, &short).map_err(|error| background::CommandFailure {
                        message: format!("giving the words to the session it went on in: {error}"),
                        started: true,
                    })
                }),
                Err(error) => Err(background::CommandFailure {
                    message: format!("freeing the words for the session it goes on in: {error}"),
                    started: false,
                }),
            };
            continued.map_err(|error| crate::lifecycle::wake::WakeFailure {
                failure: Failure::new(error.message),
                attempted: error.started,
                permanent: false,
            })?;
        } else {
            return Err(crate::lifecycle::wake::WakeFailure {
                failure: Failure::new(format!(
                    "{session} was still running; Claude Code started a copy of it instead, which was stopped"
                )),
                attempted: false,
                permanent: false,
            });
        }
    }
    crate::lifecycle::retry::accepted(node, resident, &id).map_err(|failure| crate::lifecycle::wake::WakeFailure {
        failure,
        attempted: true,
        permanent: false,
    })?;
    // Claude Code took the words; the session that takes them may still end
    // before the World's plugin pairs, which only watching it tells.
    startup::observe(node, resident, id.clone(), true);
    Ok(Landed::Handed { id })
}
