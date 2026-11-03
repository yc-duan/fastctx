//! The network drill (`world-concurrency.md`, 常驻的演练): a hub and three
//! machines on this computer, put through the ways a network fails. Every
//! connection cut, the hub killed, a receiver away or restarting while
//! thousands of letters come for it, a line gone dead without closing,
//! either way or one way only, a line slower than one message crosses
//! within a silence, a stream whose far end goes dead or takes nothing,
//! questions given up or never answered, machines coming and going; and,
//! given an earlier release's kernel, that release's hub serving this
//! one's machines and its node on this one's hub. Each scenario checks that
//! its trouble did happen, what must hold however it went, and, on this
//! release's hub, that the hub was left as it was found.
//!
//!     network_drill [--root DIR] [--earlier FASTCTX] [--earlier-version VERSION] [--only NAME,...]
//!     network_drill hub DIR      (the hub, served in a child process)
//!
//! `DIR` and `FASTCTX` are absolute paths; the drill's directory is under
//! `--root`, else `FASTCTX_TEST_ROOT`, else the system's temporary one, and
//! is deleted when every check passed.

// A drill drives the hub from outside it, and is not held to the roles of
// the hub's threads (`clippy.toml`).
#![allow(clippy::disallowed_methods)]

mod health;
mod lab;
mod letters;
mod mixed;
mod plan_copies;
mod process;
mod proxy;
mod record_following;
mod report;
mod scenarios;
mod skill_folders;
mod stalls;
mod world;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use futures_util::FutureExt as _;
use report::Report;
use world::{Program, World};

/// Every scenario, in the order they run.
const SCENARIOS: [&str; 17] = [
    "skill-folders",
    "plan-copies",
    "answers",
    "backlog",
    "cuts",
    "refused",
    "crash",
    "restart",
    "freeze",
    "half-open",
    "quiet-stream",
    "unread-stream",
    "slow",
    "cancel",
    "presence",
    "earlier-hub",
    "earlier-node",
];

/// The longest a scenario may take.
const LONGEST: Duration = Duration::from_secs(8 * 60);
pub const EXPECTED_EARLIER: &str = "1.0.1-beta.13";
static EARLIER_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
pub fn expected_earlier() -> &'static str {
    EARLIER_VERSION.get().map_or(EXPECTED_EARLIER, String::as_str)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("hub") {
        return serve(Path::new(&args[1]));
    }
    match drill(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            println!("FAIL network drill: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Serves the hub in `dir` until it is killed, its threads in their roles as
/// the command runs it.
fn serve(dir: &Path) -> ExitCode {
    let leave = fastctx_hub::Leave { remove_service: std::sync::Arc::new(|_| Ok(String::new())) };
    match fastctx_hub::run(dir, leave, tokio::runtime::Builder::new_multi_thread()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("the drill's hub: {error:#}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    root: PathBuf,
    earlier: Option<PathBuf>,
    earlier_version: String,
    only: Option<Vec<String>>,
}

fn options(args: &[String]) -> anyhow::Result<Options> {
    let mut root = std::env::var_os("FASTCTX_TEST_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let (mut earlier, mut only) = (None, None);
    let mut earlier_version = EXPECTED_EARLIER.to_string();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow::anyhow!("{arg} takes a value"));
        match arg.as_str() {
            "--root" => root = PathBuf::from(value()?),
            "--earlier" => earlier = Some(PathBuf::from(value()?)),
            "--earlier-version" => earlier_version = value()?.clone(),
            "--only" => only = Some(value()?.split(',').map(str::to_string).collect::<Vec<_>>()),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(root.is_absolute(), "the root {} is not an absolute path", root.display());
    if let Some(earlier) = &earlier {
        anyhow::ensure!(earlier.is_absolute(), "the earlier kernel {} is not an absolute path", earlier.display());
        anyhow::ensure!(earlier.is_file(), "there is no earlier kernel at {}", earlier.display());
    }
    if let Some(only) = &only {
        for name in only {
            anyhow::ensure!(SCENARIOS.contains(&name.as_str()), "there is no scenario {name}");
        }
    }
    Ok(Options { root, earlier, earlier_version, only })
}

fn drill(args: &[String]) -> anyhow::Result<bool> {
    let options = options(args)?;
    let _ = EARLIER_VERSION.set(options.earlier_version.clone());
    let expected = expected_earlier();
    let root = options.root.join(format!("fastctx-network-drill-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    println!("network drill in {}", root.display());
    if let Some(earlier) = &options.earlier {
        let mut command = world::earlier(earlier, &root.join("version-check"));
        command.arg("--version");
        let said = process::run(command, None, Duration::from_secs(10))?;
        anyhow::ensure!(
            said.status.success()
                && said.out.split_whitespace().any(|word| word == expected)
                && !said.text().contains("development"),
            "expected the released {expected} kernel, got {}",
            said.text()
        );
    }
    stalls::watch();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for name in SCENARIOS {
        if options.only.as_ref().is_some_and(|only| !only.iter().any(|wanted| wanted == name)) {
            continue;
        }
        let earlier = name.starts_with("earlier-");
        if earlier && options.earlier.is_none() {
            println!("  {name}: not run: no earlier kernel was given (--earlier)");
            skipped += 1;
            if options.only.is_some() {
                failed += 1;
            }
            continue;
        }
        let mut report = runtime
            .block_on(
                std::panic::AssertUnwindSafe(scenario(name, &root.join(name), options.earlier.as_deref()))
                    .catch_unwind(),
            )
            .unwrap_or_else(|_| {
                let mut report = Report::new(name);
                report.check(false, "the scenario did not panic", "see the panic above");
                report
            });
        rules_kept(&mut report, world::violated());
        passed += report.passed;
        failed += report.failed;
        if report.failed == 0
            && let Err(error) = remove_finished(&root.join(name))
        {
            failed += 1;
            println!("FAIL removing {name}'s completed workspace: {error}");
        }
    }
    // What the links leave running ends with the runtime, and the hubs they
    // kept with them.
    runtime.shutdown_timeout(Duration::from_secs(5));
    let late = world::violated();
    if !late.is_empty() {
        let mut report = Report::new("hubs stopped last");
        rules_kept(&mut report, late);
        failed += report.failed;
    }
    failed += process::CLEANUP_FAILURES.load(std::sync::atomic::Ordering::SeqCst);
    println!("network drill: {passed} passed, {failed} failed, {skipped} scenarios not run");
    if failed == 0 {
        std::fs::remove_dir_all(&root)?;
    } else {
        println!("what the failed scenarios left is in {}", root.display());
    }
    Ok(failed == 0)
}

/// Checks that the hubs of this release a scenario ran broke none of the
/// threads' rules (`specs/world-engine.md`, 线程的三种角色), as each told in
/// its health before it was stopped. A development build runs many times
/// slower than the numbers the timed ones are set for, and a machine that
/// held threads back past the slow poll's mark tells its load in them
/// (`stalls`): there, those are told, and only blocking where a thread must
/// not, or a nested read, fails.
fn rules_kept(report: &mut Report, violated: Vec<(PathBuf, serde_json::Value)>) {
    let mut broken = Vec::new();
    let held_back = stalls::held_back();
    for (dir, places) in &violated {
        for place in places.as_array().into_iter().flatten() {
            let told = format!("{}: {} {} x{}", dir.display(), place["kind"], place["place"], place["count"]);
            let timed = !matches!(place["kind"].as_str(), Some("blocking" | "nested-read"));
            if timed && cfg!(debug_assertions) {
                report.note(format!("in a development build: {told}"));
            } else if let Some(held_back) = held_back.filter(|_| timed) {
                report.note(format!("on a machine that held a thread back {} ms: {told}", held_back.as_millis()));
            } else {
                broken.push(told);
            }
        }
    }
    report.check(broken.is_empty(), "this release's hubs broke none of the threads' rules", broken.join("; "));
}

/// Removes a finished scenario's directory. A link stopped in it can still
/// be finishing a write there, as its blocking work outlives it by a moment
/// where a process would have ended; what is left after that is named.
fn remove_finished(dir: &Path) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Err(error) => return Err(std::io::Error::other(format!("{error}; still there: {}", left_in(dir)))),
        }
    }
}

/// Up to twenty of what is in `dir`, named from it.
fn left_in(dir: &Path) -> String {
    let mut left = Vec::new();
    let mut directories = vec![dir.to_path_buf()];
    while let Some(next) = directories.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path.clone());
            }
            left.push(path.strip_prefix(dir).unwrap_or(&path).display().to_string());
            if left.len() == 20 {
                return left.join(", ");
            }
        }
    }
    left.join(", ")
}

async fn scenario(name: &'static str, dir: &Path, earlier: Option<&Path>) -> Report {
    let mut report = Report::new(name);
    let program = match (name, earlier) {
        ("earlier-hub", Some(earlier)) => Program::Earlier(earlier.to_path_buf()),
        _ => Program::This,
    };
    let names: &[&str] = if name == "earlier-node" { &["alpha", "beta"] } else { &["alpha", "beta", "gamma"] };
    let world = match tokio::time::timeout(Duration::from_secs(240), World::new(dir, program.clone(), names)).await {
        Ok(Ok(world)) => world,
        Ok(Err(error)) => {
            report.check(false, "the hub and its machines were set up", format!("{error:#}"));
            return report;
        }
        Err(error) => {
            report.check(false, "setup completed within its deadline", error.to_string());
            return report;
        }
    };
    if !report.check(
        world.all_online(Duration::from_secs(60)).await,
        "every machine came online and saw the others",
        "",
    ) {
        let ended = world.end().await;
        report.check(ended.is_ok(), "every process, proxy and handler ended", format!("{ended:?}"));
        return report;
    }
    let ran = tokio::time::timeout(LONGEST, async {
        match name {
            "plan-copies" => plan_copies::exercise(&world, &mut report).await,
            "skill-folders" => skill_folders::exercise(&world, &mut report).await,
            "answers" => scenarios::answers(&world, &mut report).await,
            "backlog" => scenarios::backlog(&world, &mut report).await,
            "cuts" => scenarios::cuts(&world, &mut report).await,
            "refused" => scenarios::refused(&world, &mut report).await,
            "crash" => scenarios::crash(&world, &mut report).await,
            "restart" => scenarios::restart(&world, &mut report).await,
            "freeze" => scenarios::freeze(&world, &mut report).await,
            "half-open" => scenarios::half_open(&world, &mut report).await,
            "quiet-stream" => scenarios::quiet_stream(&world, &mut report).await,
            "unread-stream" => scenarios::unread_stream(&world, &mut report).await,
            "slow" => scenarios::slow(&world, &mut report).await,
            "cancel" => scenarios::cancel(&world, &mut report).await,
            "presence" => scenarios::presence(&world, &mut report, true).await,
            "earlier-hub" => mixed::earlier_hub(&world, &mut report).await,
            "earlier-node" => {
                mixed::earlier_node(&world, earlier.expect("an earlier kernel for this scenario"), &mut report).await
            }
            _ => unreachable!("every scenario is matched"),
        }
    })
    .await;
    match ran {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            report.check(false, "the scenario ran to its end", format!("{error:#}"));
        }
        Err(_) => {
            report.check(false, "the scenario ended within its time", report::seconds(LONGEST));
        }
    }
    if world.hub.serves_this() {
        let online = world.all_online(Duration::from_secs(90)).await;
        let since = jiff::Timestamp::now();
        let settled = health::settled(&world.hub, since, names.len() as u64, Duration::from_secs(30)).await;
        report.check(
            online && settled.is_ok(),
            "the hub was left as it was found: nothing waiting, held or paired, every machine connected once",
            match settled {
                Ok(took) => report::seconds(took),
                Err(left) => left,
            },
        );
        // The hub's own count of sessions opened lately, against the ones it
        // noted, while it ran without a restart.
        if world.hub.restarts.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            let noted = world
                .hub
                .log()
                .lines()
                .filter(|line| line.contains(" connected from ") || line.contains(" connected through "))
                .count() as u64;
            let counted = health::fresh(&world.hub, jiff::Timestamp::now(), Duration::from_secs(5))
                .await
                .and_then(|health| health["opened_in_5_minutes"].as_u64());
            report.check(
                counted == Some(noted),
                "the hub counted every session it opened",
                format!("counted {counted:?}, noted {noted}"),
            );
        }
    }
    let ended = world.end().await;
    report.check(ended.is_ok(), "every process, proxy and handler ended", format!("{ended:?}"));
    report
}

/// A small generator of the times the scenarios wait, the same every run.
pub struct Rng(pub u64);

impl Rng {
    pub fn draw(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn between(&mut self, low: u64, high: u64) -> u64 {
        low + self.draw() % (high - low + 1)
    }
}
