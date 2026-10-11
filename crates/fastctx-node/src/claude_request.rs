//! The short-lived helper held by a Claude hook's process stream. It waits
//! for an answer or a concrete lifecycle event, never for a made-up deadline.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc;

use fastctx_hosts::claude::control::{self, Answer, Question, Waiting};
use fastctx_types::ProcessIdentity;
use jiff::Timestamp;
use serde_json::{Value, json};

/// A process stream gives the mod activation an observable lifetime even
/// when its plugin unloads while the Claude process remains alive.
pub fn guard() -> io::Result<()> {
    let parent = fastctx_platform::host_process::parent_identity()?;
    output(&json!({"process":fastctx_platform::current_identity()?, "parent":parent}))?;
    io::stdout().flush()?;
    fastctx_platform::host_process::wait_for_exit(&parent)
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
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = changes.send(event.map(|_| false).map_err(io::Error::other));
    })
    .map_err(io::Error::other)?;
    notify::Watcher::watch(&mut watcher, &directory, notify::RecursiveMode::NonRecursive).map_err(io::Error::other)?;
    std::thread::spawn(move || {
        let _ = sender.send(fastctx_platform::host_process::wait_for_exit(&controller).map(|()| true));
    });
    let mut transcript_watched = false;
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
                    notify::Watcher::watch(
                        &mut watcher,
                        path.parent().ok_or_else(|| io::Error::other("Claude transcript has no parent"))?,
                        notify::RecursiveMode::NonRecursive,
                    )
                    .map_err(io::Error::other)?;
                    transcript_watched = true;
                }
                match fastctx_hosts::claude::transcript::read(&path) {
                    Ok(read)
                        if read.context_uuids.len() == messages
                            && after_compact.is_none_or(|parent| read.context_parent.as_deref() == Some(parent)) =>
                    {
                        if !read.problems.is_empty() {
                            return Err(io::Error::other(read.problems.join("; ")));
                        }
                        let tail = expected
                            .as_ref()
                            .and_then(|value| value["tail"].as_array())
                            .ok_or_else(|| io::Error::other("the mod omitted its observed message projection"))?;
                        if !read.message_tail_matches(tail) {
                            return Err(io::Error::other(
                                "the native file does not match the mod's current message projection",
                            ));
                        }
                        policy["last_uuid"] =
                            serde_json::to_value(read.context_uuids.last()).map_err(io::Error::other)?;
                        policy["context_tail"] = serde_json::to_value(read.context_tail).map_err(io::Error::other)?;
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
        if receiver.recv().map_err(io::Error::other)?? {
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
        (directory, Question { key, host, turn, request: "Elicitation".into(), params, native_wait: false })
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
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = changes.send(event.map(|_| None).map_err(io::Error::other));
    })
    .map_err(io::Error::other)?;
    notify::Watcher::watch(&mut watcher, directory, notify::RecursiveMode::Recursive).map_err(io::Error::other)?;
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
