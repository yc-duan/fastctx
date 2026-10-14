//! World's own part of a removal that starts in its package uninstaller:
//! from Windows' installed apps, or from a terminal or a remote session with
//! `uninstall.exe /S`. The uninstaller runs this program, which runs the same
//! kernel removal as the settings and writes each step, and what is left, as
//! a line of the uninstaller's details (`world-gui.md`, 设置).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead as _, Read as _};
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};

use fastctx_types::uninstall::{Event, Left, Plan, Result as Outcome};

use crate::installer_lines::say;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How the removal the uninstaller asked for came out.
pub(crate) enum Removed {
    AsAsked,
    /// Everything was asked for, but this computer is a world's server: that
    /// world ends only from inside World, where its name is typed, so the
    /// data was kept and the rest removed.
    KeptForHub,
    /// Nothing was removed: this computer is a managed machine its owner
    /// keeps, and the uninstaller is not an administrator's.
    Managed,
}

fn command(action: &str, everything: bool) -> Result<Command, String> {
    let mut command = Command::new(crate::removal::program()?);
    command.args(["uninstall", action, "--json"]);
    if everything {
        command.arg("--everything");
    }
    let home = fastctx_platform::user_home().map_err(|error| error.to_string())?;
    for path in fastctx_install::uninstall::application_directories(&home).map_err(|error| error.to_string())? {
        command.arg("--application-dir").arg(path);
    }
    command.stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}

fn plan(everything: bool) -> Result<Plan, String> {
    let output = command("plan", everything)?.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("reading the uninstall plan: {error}"))
}

/// Whether a complete removal begun in World, where the world's name was
/// typed, was cut short: the uninstaller finishes it rather than keep the
/// data.
fn complete_removal_begun() -> Result<bool, String> {
    let marker = fastctx_install::uninstall::Installation::here().map_err(|error| error.to_string())?.marker();
    match std::fs::read(&marker) {
        Ok(bytes) => {
            let request: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| format!("reading {}: {error}", marker.display()))?;
            Ok(request["plan"]["everything"] == serde_json::Value::Bool(true))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("reading {}: {error}", marker.display())),
    }
}

/// Runs the kernel removal and writes its progress to standard output.
/// Fails when a step on this computer did not finish; a server that could
/// not be told this computer left is said among what is left, as in the
/// settings.
pub(crate) fn at_package_uninstall(everything: bool) -> Result<Removed, String> {
    let mut removed = Removed::AsAsked;
    let mut plan = plan(everything)?;
    if let Some(managed) = &plan.managed
        && !fastctx_platform::elevation::is_administrator().map_err(|error| error.to_string())?
    {
        say(&managed.refusal());
        return Ok(Removed::Managed);
    }
    if everything
        && let Some(hub) = &plan.hub
        && !complete_removal_begun()?
    {
        say(&format!(
            "This computer is the server of the world {}. Removing everything here would end that world for every computer in it; that is done only inside Fastctx, where the world's name is typed to confirm. The data stays; the rest of Fastctx is removed.",
            hub.name
        ));
        removed = Removed::KeptForHub;
        plan = self::plan(false)?;
    }
    let mut child = command("run", plan.everything)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stderr = child.stderr.take().ok_or("the uninstall's diagnostics are absent")?;
    let diagnostics = std::thread::spawn(move || {
        let mut text = String::new();
        std::io::BufReader::new(stderr).read_to_string(&mut text).map(|_| text)
    });
    let names: BTreeMap<_, _> = plan.steps.iter().map(|step| (step.id.clone(), step)).collect();
    let mut done = BTreeSet::new();
    let mut finished = false;
    let stdout = child.stdout.take().ok_or("the uninstall's progress is absent")?;
    for line in std::io::BufReader::new(stdout).lines() {
        let line = line.map_err(|error| error.to_string())?;
        match serde_json::from_str::<Event>(&line).map_err(|error| format!("reading uninstall progress: {error}"))? {
            Event::Step { step, result, why } => {
                let (what, location) = names
                    .get(&step)
                    .map(|item| (item.what.as_str(), item.location.as_deref()))
                    .unwrap_or((step.as_str(), None));
                let mark = match result {
                    Outcome::Done => "Done",
                    Outcome::Failed => "Failed",
                    Outcome::Skipped => "Skipped",
                };
                say(&format!(
                    "{mark}: {what}{}{}",
                    location.map(|location| format!(" ({location})")).unwrap_or_default(),
                    why.map(|why| format!(": {why}")).unwrap_or_default()
                ));
                if result == Outcome::Done {
                    done.insert(step);
                } else {
                    done.remove(&step);
                }
            }
            Event::Finished { left } => {
                finished = true;
                say_left(&left);
            }
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let diagnostics = diagnostics
        .join()
        .map_err(|_| "reading the uninstall's diagnostics panicked".to_owned())?
        .map_err(|error| error.to_string())?;
    let local = plan.steps.iter().filter(|step| step.kind != "logout").all(|step| done.contains(&step.id));
    if status.success() || (finished && local) { Ok(removed) } else { Err(diagnostics) }
}

/// Completes a complete removal the settings began, now that World has
/// exited: the application's own data goes.
pub(crate) fn finish_checkpoint() -> Result<(), String> {
    let output = Command::new(crate::removal::program()?)
        .args(["uninstall", "application-finish"])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| error.to_string())?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match serde_json::from_str::<Event>(line) {
            Ok(Event::Finished { left }) => say_left(&left),
            _ => say(line),
        }
    }
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(())
}

fn say_left(left: &[Left]) {
    if left.is_empty() {
        return;
    }
    say("Left on this computer:");
    for item in left {
        say(&format!(
            "  {}{}: {}",
            item.what,
            item.location.as_deref().map(|location| format!(" ({location})")).unwrap_or_default(),
            item.why
        ));
    }
}
