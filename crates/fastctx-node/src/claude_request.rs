//! The short-lived helper held by a Claude hook's process stream. It waits
//! for an answer or a concrete lifecycle event, never for a made-up deadline;
//! the one bound is how long Claude Code's file may lag behind what its
//! session holds (`CATCH_UP`).

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fastctx_hosts::claude::control::{self, Answer, Question, Waiting};
use fastctx_types::ProcessIdentity;
use jiff::Timestamp;
use serde_json::{Value, json};

/// A process stream gives the mod activation an observable lifetime even
/// when its plugin unloads while the Claude process remains alive.
///
/// Given the session's directory and fastctx's root, it also wakes the mod:
/// a line `look` whenever the node leaves the session something (words in
/// its inbox, a stop, a control), so the mod takes it at once instead of on
/// a clock; and a line `removed` when fastctx's removal begins, after which
/// it ends, releasing the program the removal deletes.
pub fn guard(watch: Option<(&Path, &Path)>) -> io::Result<()> {
    let parent = fastctx_platform::host_process::parent_identity()?;
    output(&json!({"process":fastctx_platform::current_identity()?, "parent":parent}))?;
    io::stdout().flush()?;
    match watch {
        None => fastctx_platform::host_process::wait_for_exit(&parent),
        Some((session, root)) => wake(&parent, session, root),
    }
}

enum Woken {
    Look,
    Removed,
    Exited(io::Result<()>),
}

fn wake(parent: &ProcessIdentity, session: &Path, root: &Path) -> io::Result<()> {
    let inbox = session.join("inbox");
    let controls = session.join("control").join("inbox");
    for directory in [&inbox, &controls] {
        std::fs::create_dir_all(directory)?;
    }
    let (sender, receiver) = mpsc::channel();
    let changes = sender.clone();
    let watched = crate::file_events::named(session)?;
    let marker = crate::file_events::named(root)?.join("uninstalled.json");
    let (inbox_seen, controls_seen) = (watched.join("inbox"), watched.join("control").join("inbox"));
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        let event = match event {
            Ok(event) if !event.need_rescan() => event,
            _ => {
                // A lost notification has no reliable path to filter.
                let _ = changes.send(Woken::Look);
                return;
            }
        };
        // The plugin reads these files when it looks: a read is no news.
        if !crate::file_events::changes(&event) {
            return;
        }
        for path in &event.paths {
            let parent = path.parent();
            let woken = if *path == marker {
                Some(Woken::Removed)
            } else if parent == Some(watched.as_path()) && path.file_name().is_some_and(|name| name == "stop.json")
                || (parent == Some(inbox_seen.as_path()) || parent == Some(controls_seen.as_path()))
                    && path.extension().is_some_and(|extension| extension == "json")
            {
                Some(Woken::Look)
            } else {
                None
            };
            if let Some(woken) = woken {
                let _ = changes.send(woken);
            }
        }
    });
    // One watch over the whole session directory, opened on that directory
    // alone: the node may move it into its conversation, which Windows
    // refuses while a watch is open on anything inside it.
    watcher.watch(session, notify::RecursiveMode::Recursive).map_err(io::Error::other)?;
    watcher.watch(root, notify::RecursiveMode::NonRecursive).map_err(io::Error::other)?;
    let parent = parent.clone();
    std::thread::spawn(move || {
        let _ = sender.send(Woken::Exited(fastctx_platform::host_process::wait_for_exit(&parent)));
    });
    if root.join("uninstalled.json").try_exists()? {
        return say("removed");
    }
    let mut out = io::stdout();
    // Inputs can arrive after the mod's first scan and before this watch is
    // installed. A scan after installation closes that notification gap.
    writeln!(out, "look")?;
    out.flush()?;
    loop {
        let mut woken = receiver.recv().map_err(io::Error::other)?;
        // Changes that came together wake the mod once.
        loop {
            match woken {
                Woken::Exited(result) => return result,
                Woken::Removed => return say("removed"),
                Woken::Look => match receiver.try_recv() {
                    Ok(next) => woken = next,
                    Err(_) => break,
                },
            }
        }
        if root.join("uninstalled.json").try_exists()? {
            return say("removed");
        }
        writeln!(out, "look")?;
        out.flush()?;
    }
}

fn say(line: &str) -> io::Result<()> {
    let mut out = io::stdout();
    writeln!(out, "{line}")?;
    out.flush()
}

/// The owner of queued controls, or null once that node lifetime ended.
pub fn owner(world: &Path) -> io::Result<()> {
    let controller = control::read::<ProcessIdentity>(&world.join("claude-controller.json"))?;
    let current = match controller {
        Some(controller) if fastctx_platform::is_alive(&controller)? => Some(controller),
        _ => None,
    };
    output(&current)
}

/// Waits for the node to identify this session before its system prompt is built.
/// How long Claude Code's own file may hold fewer messages than its session
/// does before the policy stops waiting for it. Rows reach the file a moment
/// after the session holds them, but a message the session keeps only in
/// memory, as the error a failed request leaves, never does: a session
/// resumed after one would otherwise wait on its file for good.
const CATCH_UP: Duration = Duration::from_secs(10);

pub fn policy(world: &Path, session: &str, messages: Option<usize>, after_compact: Option<&str>) -> io::Result<()> {
    crate::claude::pairing::validate_session(session)?;
    let directory = world.join("claude").join(session);
    let expected: Option<Value> = if messages.is_some() {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Some(serde_json::from_str(&input).map_err(io::Error::other)?)
    } else {
        None
    };
    let controller = control::read::<ProcessIdentity>(&world.join("claude-controller.json"))?
        .ok_or_else(|| io::Error::other("the World node is not running"))?;
    let (sender, receiver) = mpsc::channel();
    let changes = sender.clone();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if crate::file_events::may_change(&event) {
            let _ = changes.send(event.map(|_| false).map_err(io::Error::other));
        }
    });
    watcher.watch(&directory, notify::RecursiveMode::NonRecursive).map_err(io::Error::other)?;
    std::thread::spawn(move || {
        let _ = sender.send(fastctx_platform::host_process::wait_for_exit(&controller).map(|()| true));
    });
    let mut transcript_watched = false;
    let behind_since = messages.map(|_| Instant::now());
    loop {
        if messages.is_some()
            && let Some(host) = control::host(&directory)?
            && !control::active(&directory, &host)?
        {
            return Err(io::Error::other("the Claude activation ended before persisting its context"));
        }
        if let Some(mut policy) = control::read::<Value>(&directory.join("policy.json"))? {
            if let Some(messages) = messages {
                let path = fastctx_hosts::claude::exchange::transcript(&directory)?
                    .ok_or_else(|| io::Error::other("Claude did not report its transcript path"))?;
                if !transcript_watched {
                    watcher
                        .watch(
                            path.parent().ok_or_else(|| io::Error::other("Claude transcript has no parent"))?,
                            notify::RecursiveMode::NonRecursive,
                        )
                        .map_err(io::Error::other)?;
                    transcript_watched = true;
                }
                match fastctx_hosts::claude::transcript::read(&path) {
                    Ok(read)
                        if (read.context_uuids.len() == messages
                            || (expected.as_ref().is_some_and(|value| value["retained"] == true)
                                && (read.context_uuids.len() <= messages || messages == 4096)))
                            && after_compact.is_none_or(|parent| read.context_parent.as_deref() == Some(parent)) =>
                    {
                        if !read.problems.is_empty() {
                            return Err(io::Error::other(read.problems.join("; ")));
                        }
                        let tail = expected
                            .as_ref()
                            .and_then(|value| value["tail"].as_array())
                            .ok_or_else(|| io::Error::other("the mod omitted its observed message projection"))?;
                        let retained = expected.as_ref().is_some_and(|value| value["retained"] == true);
                        let tail = if retained && tail.len() >= read.context_uuids.len() {
                            &tail[tail.len() - read.context_uuids.len()..]
                        } else {
                            tail.as_slice()
                        };
                        if !read.message_tail_matches(tail) {
                            return Err(io::Error::other(
                                "the native file does not match the mod's current message projection",
                            ));
                        }
                        policy["last_uuid"] =
                            serde_json::to_value(read.context_uuids.last()).map_err(io::Error::other)?;
                        policy["context_tail"] = serde_json::to_value(read.context_tail).map_err(io::Error::other)?;
                        policy["context_contract"] = json!(1);
                        policy["retained_messages"] = json!(read.context_uuids.len());
                        policy["synthetic_tail"] = json!(read.synthetic_tail);
                        policy["context_uuids"] = serde_json::to_value(read.context_uuids).map_err(io::Error::other)?;
                        return output(&policy);
                    }
                    Ok(read)
                        if after_compact.is_none_or(|parent| read.context_parent.as_deref() == Some(parent))
                            && read.context_uuids.len() > messages =>
                    {
                        return Err(io::Error::other(
                            "the native file has more context than the mod's observed prefix",
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::Interrupted) => {}
                    Err(error) => return Err(error),
                }
            } else {
                return output(&policy);
            }
        }
        let heard = match behind_since {
            Some(since) => {
                let Some(left) = CATCH_UP.checked_sub(since.elapsed()) else {
                    return Err(io::Error::other(format!(
                        "the native file still holds less context than the mod's {} observed messages",
                        messages.unwrap_or_default()
                    )));
                };
                match receiver.recv_timeout(left) {
                    Ok(heard) => heard,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(error) => return Err(io::Error::other(error)),
                }
            }
            None => receiver.recv().map_err(io::Error::other)?,
        };
        if heard? {
            return Err(io::Error::other("the World node exited before identifying this Claude session"));
        }
    }
}

/// Runs the mod helper, or the official plugin Elicitation command hook.
/// Neither reads the environment or any Claude login/configuration file.
pub fn run(directory: &Path, elicitation: bool) -> io::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let (directory, question) = if elicitation {
        let params: Value = serde_json::from_str(&input).map_err(io::Error::other)?;
        let session = params["session_id"].as_str().ok_or_else(|| io::Error::other("Elicitation has no session_id"))?;
        crate::claude::pairing::validate_session(session).map_err(io::Error::other)?;
        let directory = control::directory(directory, session);
        let Some(host) = control::host(&directory)? else {
            return output(&json!({}));
        };
        let turn = if params.get("agent_id").is_some_and(Value::is_string) {
            None
        } else {
            control::read_mod::<Value>(&directory.join("activity.json"))?
                .and_then(|value| value["turn"].as_str().map(str::to_string))
        };
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let key = format!("elicitation-{}", random.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
        (directory, Question { key, host, turn, request: "Elicitation".into(), params })
    } else {
        (directory.to_path_buf(), serde_json::from_str::<Question>(&input).map_err(io::Error::other)?)
    };
    let reply = wait(&directory, question)?;
    if elicitation {
        let answer = match reply {
            Answer::Answer { answer } => {
                let mut fields = answer.as_object().expect("validated answer").clone();
                fields.insert("hookEventName".into(), "Elicitation".into());
                json!({"hookSpecificOutput": fields})
            }
            Answer::Pass { reason } => {
                eprintln!("fastctx left MCP elicitation to Claude Code: {reason}");
                json!({})
            }
        };
        output(&answer)
    } else {
        output(&reply)
    }
}

fn output(value: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer(io::stdout().lock(), value).map_err(io::Error::other)?;
    io::stdout().write_all(b"\n")
}

fn wait(directory: &Path, question: Question) -> io::Result<Answer> {
    // A hook can fire while MCP startup still precedes the mod's session
    // registration. Waiting then could block the very registration that
    // would let the node route the request to a resident.
    if !directory.parent().is_some_and(|session| session.join("resident.json").exists()) {
        return Ok(pass("the World has not paired this Claude session with a resident yet"));
    }
    let world = directory.ancestors().nth(3).ok_or_else(|| io::Error::other("invalid Claude control directory"))?;
    let Some(controller) = control::read::<ProcessIdentity>(&world.join("claude-controller.json"))? else {
        return Ok(pass("the World's node is not running"));
    };
    if !fastctx_platform::is_alive(&controller)? || !control::active(directory, &question.host)? {
        return Ok(pass("the node or this Claude activation ended"));
    }
    let request_dir = question.directory(directory)?;
    fastctx_platform::ensure_private_dir(&request_dir)?;
    let waiting = Waiting { question, process: fastctx_platform::current_identity()?, at: Timestamp::now() };
    let (sender, receiver) = mpsc::channel();
    let changes = sender.clone();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if crate::file_events::may_change(&event) {
            let _ = changes.send(event.map(|_| None).map_err(io::Error::other));
        }
    });
    // On the session's directory itself, as the control helper's watch is:
    // the node may move it, which Windows refuses while a watch is open on
    // anything inside it.
    let watched = directory.parent().unwrap_or(directory);
    watcher.watch(watched, notify::RecursiveMode::Recursive).map_err(io::Error::other)?;
    for (process, reason) in [
        (controller, "the World's node exited while the request waited"),
        (waiting.question.host.process.clone(), "the Claude control activation ended"),
    ] {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let result = fastctx_platform::host_process::wait_for_exit(&process).map(|()| Some(reason));
            let _ = sender.send(result);
        });
    }
    control::write(&request_dir.join("request.json"), &waiting)?;
    let answer = loop {
        if let Some(reason) = invalidated(directory, &request_dir, &waiting.question)? {
            break pass(&reason);
        }
        if let Some(reply) = control::read::<Answer>(&request_dir.join("answer.json"))? {
            if let Answer::Answer { answer } = &reply {
                control::validate_answer(&waiting.question, answer).map_err(io::Error::other)?;
            }
            break reply;
        }
        if let Some(reason) = receiver.recv().map_err(io::Error::other)?? {
            break pass(reason);
        }
    };
    control::write(&request_dir.join("resolved.json"), &answer)?;
    Ok(answer)
}

fn invalidated(directory: &Path, request: &Path, question: &Question) -> io::Result<Option<String>> {
    if control::host(directory)?.is_some_and(|host| host != question.host)
        || directory.join("closed").join(format!("{}.json", question.host.instance)).exists()
    {
        return Ok(Some("the Claude activation ended".into()));
    }
    if let Some(cancel) = control::read_mod::<Value>(&request.join("cancel.json"))? {
        return Ok(Some(cancel["reason"].as_str().unwrap_or("the host cancelled the request").into()));
    }
    if let Some(turn) = &question.turn
        && let Some(activity) = control::read_mod::<Value>(&directory.join("activity.json"))?
        && activity["instance"].as_str() == Some(&question.host.instance)
        && activity["turn"].as_str() != Some(turn)
    {
        return Ok(Some("the Claude turn ended".into()));
    }
    Ok(None)
}

fn pass(reason: &str) -> Answer {
    Answer::Pass { reason: reason.into() }
}
