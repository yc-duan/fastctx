//! The scenarios with an earlier release, as a network has them while its
//! machines update one by one: an earlier hub serving this release's
//! machines, then updated in place, and an earlier node on this release's
//! hub.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fastctx_types::MachineName;
use fastctx_types::access::Purpose;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::update::UpdateCall;

use crate::letters::{self, arrived, send};
use crate::report::{Report, seconds};
use crate::scenarios::{self, give_up};
use crate::world::{Said, World, earlier, until, with_password};

/// An earlier hub serving this release's machines: questions given up are
/// passed on and stopped, who is online comes as changes, and letters,
/// answers of every size, cuts and a crash go as with this release's hub.
/// Then it is updated in place while letters wait for a machine away: they
/// are kept where this release keeps them, those after numbered on past
/// them, and every one comes once.
pub async fn earlier_hub(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    beta.lab.stall(true);
    let gave_up = give_up(world, "alpha", "beta", 3).await?;
    let link = alpha.link();
    let look = RemoteCall::Updates { update: UpdateCall::Look };
    let after = link.call(&beta.name, &world.account, look.purpose(), look).await;
    report.check(
        gave_up == 3 && alpha.online() && matches!(after, Ok(Err(_))),
        "alpha gave three questions up, and its session went on to the next call",
        format!("gave up {gave_up}"),
    );
    let stopped = until(Duration::from_secs(15), || beta.lab.answering() == 0).await;
    beta.lab.stall(false);
    report.check(
        stopped.is_some() && beta.lab.stopped() == 3,
        "the earlier hub passed the three on as given up, and beta stopped them",
        format!("{} stopped, {}", beta.lab.stopped(), stopped.map_or("not within 15 s".to_string(), seconds)),
    );
    scenarios::answers(world, report).await?;
    scenarios::presence(world, report, true).await?;

    let gamma = world.machine("gamma");
    let mut sending = send(world, &["alpha", "beta"], "gamma", "earlier-hub", 800);
    let mut rng = crate::Rng(0x3c6e_f372_fe94_f82b);
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(rng.between(400, 1500))).await;
        for machine in &world.machines {
            machine.line.cut();
        }
    }
    world.hub.restart(Duration::from_millis(500)).await?;
    sending.sent().await?;
    let came = arrived(&gamma.lab, &sending.wanted, Duration::from_secs(180)).await;
    report.check(
        came.missing.is_empty(),
        "every letter reached gamma through cuts and a crash of the earlier hub",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));

    gamma.stop();
    let numbered_before = next_for(world, "gamma").unwrap_or(0);
    let mut waiting = send(world, &["alpha", "beta"], "gamma", "updated-before", 300);
    waiting.sent().await?;
    let wanted = waiting.wanted.len() as u64;
    let held = until(Duration::from_secs(90), || {
        next_for(world, "gamma").is_some_and(|next| next >= numbered_before + wanted)
    })
    .await;
    report.check(
        held.is_some(),
        format!("the earlier hub held all {wanted} letters for gamma, away"),
        format!("numbered {:?} from {numbered_before}", next_for(world, "gamma")),
    );
    world.hub.update()?;
    let serving = until(Duration::from_secs(60), || {
        world.hub.health().is_some_and(|health| health["sessions"].as_u64() == Some(2))
    })
    .await;
    report.check(
        serving.is_some(),
        "this release's hub served the earlier one's directory, alpha and beta connected",
        serving.map_or("not within 60 s".to_string(), seconds),
    );
    let moved = !world.hub.dir.join("outbox").exists() && world.hub.dir.join("outboxes").exists();
    report.check(moved, "and kept the letters waiting where this release keeps them", "");
    let mut after = send(world, &["alpha", "beta"], "gamma", "updated-after", 300);
    after.sent().await?;
    gamma.start()?;
    let wanted: Vec<String> = waiting.wanted.iter().chain(&after.wanted).cloned().collect();
    let came = arrived(&gamma.lab, &wanted, Duration::from_secs(180)).await;
    report.check(
        came.missing.is_empty(),
        "every letter for gamma came once it was back, those held before the update and after",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));
    Ok(())
}

/// An earlier node on this release's hub: it logs in, answers this
/// release's calls, takes its letters, hears who comes and goes, stops and
/// comes back, and its session is not cut again and again by anything it
/// cannot read. Only on Linux, where logging it in installs no service.
pub async fn earlier_node(world: &World, program: &Path, report: &mut Report) -> anyhow::Result<()> {
    if !cfg!(target_os = "linux") {
        report.note("not run here: an earlier node is run on Linux only, where logging in installs no service");
        return Ok(());
    }
    // Its program where its World runs it from, as an installed release
    // is: started from anywhere else, its first start places it there and
    // hands the World to a successor of its own, which the drill would not
    // hold.
    let home = world.root.join("delta-home");
    let installed = home.join("bin").join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
    std::fs::create_dir_all(home.join("bin"))?;
    std::fs::copy(program, &installed)?;
    let node = Earlier { program: installed, home, child: Mutex::new(None) };
    // Its World says a service keeps its node already, so logging in
    // installs none: as root, or on another system, it would install one
    // for real, which then takes the World over from the node run here.
    std::fs::create_dir_all(node.home.join("world"))?;
    std::fs::write(node.home.join("world").join("node.service"), b"")?;
    node.start(&world.root.join("delta.log"))?;
    report.check(node.ready(Duration::from_secs(60)).await, "an earlier node started", "");
    let said = node.login(world.spare.address, "delta")?;
    report.check(said.status.success(), "it logged in to this release's hub", tail(&said));
    let delta = MachineName::try_from("delta".to_string()).map_err(anyhow::Error::msg)?;
    let online = world.sees("alpha", &delta, true, Duration::from_secs(60)).await;
    report.check(online.is_some(), "and came online", online.map_or("not within 60 s".to_string(), seconds));

    let alpha = world.machine("alpha");
    let facts = alpha.link().call(&delta, &world.account, Purpose::Look, RemoteCall::Facts).await;
    let version = match &facts {
        Ok(Ok(RemoteReply::Facts(facts))) => Some(facts.version.clone()),
        _ => None,
    };
    report.check(
        version.as_deref() == Some(crate::EXPECTED_EARLIER),
        "it answered a call from this release, saying its own release",
        format!("{:?}", version.unwrap_or_else(|| format!("{facts:?}"))),
    );

    let numbered_before = next_for(world, "delta").unwrap_or(0);
    let mut sending = letters::send_to(world, &["alpha", "beta"], &delta, "earlier-node", 300);
    sending.sent().await?;
    let wanted = sending.wanted.len() as u64;
    let numbered = until(Duration::from_secs(90), || {
        next_for(world, "delta").is_some_and(|next| next >= numbered_before + wanted)
    })
    .await;
    report.check(
        numbered.is_some(),
        format!("the hub took all {wanted} letters for it"),
        format!("numbered {:?} from {numbered_before}", next_for(world, "delta")),
    );
    // Numbered within a second, the letters may all come after the health
    // last written, which then still says the hub holds none.
    let numbered_at = jiff::Timestamp::now();
    let taken = until(Duration::from_secs(120), || {
        world.hub.health().is_some_and(|health| {
            crate::health::written_after(&health, numbered_at) && health["letters"]["held"].as_u64() == Some(0)
        })
    })
    .await;
    report.check(
        taken.is_some(),
        "it took them all: the hub holds none",
        taken.map_or("not within 120 s".to_string(), seconds),
    );

    alpha.stop();
    let gone = node.sees("alpha", false, Duration::from_secs(30)).await?;
    alpha.start()?;
    let back = node.sees("alpha", true, Duration::from_secs(60)).await?;
    report.check(
        gone.is_some() && back.is_some(),
        "it saw a machine of this release leave and come back",
        format!(
            "left {}, back {}",
            gone.map_or("never".to_string(), seconds),
            back.map_or("never".to_string(), seconds)
        ),
    );

    node.kill()?;
    let ended = !answers_at(&node.home.join("world").join("node.sock")).await;
    let left = world.sees("beta", &delta, false, Duration::from_secs(30)).await;
    node.start(&world.root.join("delta.log"))?;
    let ready = node.ready(Duration::from_secs(60)).await;
    let returned = world.sees("beta", &delta, true, Duration::from_secs(60)).await;
    report.check(
        ended && left.is_some() && ready && returned.is_some(),
        "it stopped and came back, and the others saw both",
        format!(
            "left {}, back {}",
            left.map_or("never".to_string(), seconds),
            returned.map_or("never".to_string(), seconds)
        ),
    );

    // The hub notes every session it opens: the earlier node's are two, its
    // first and the one after it came back, unless something it could not
    // read cut one.
    let its = world.hub.log().lines().filter(|line| line.contains("delta connected")).count();
    report.check(its == 2, "its session was never cut by anything it could not read", format!("{its} sessions of it"));
    node.kill()?;
    Ok(())
}

/// An earlier release's node, kept to its own World.
struct Earlier {
    program: PathBuf,
    home: PathBuf,
    child: Mutex<Option<Child>>,
}

impl Earlier {
    fn start(&self, log: &Path) -> anyhow::Result<()> {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        let mut command = earlier(&self.program, &self.home);
        command.arg("node").arg("--dir").arg(&self.home);
        let mut owned = self.child.lock().expect("never poisoned");
        anyhow::ensure!(owned.is_none(), "the earlier node already owns a child process");
        *owned = Some(command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn()?);
        Ok(())
    }

    /// Waits for it to take calls from this machine's programs.
    async fn ready(&self, within: Duration) -> bool {
        let socket = self.home.join("world").join("node.sock");
        let began = Instant::now();
        while began.elapsed() < within {
            if answers_at(&socket).await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    fn login(&self, hub: SocketAddr, name: &str) -> anyhow::Result<Said> {
        let mut command = earlier(&self.program, &self.home);
        command.arg("login").arg(hub.to_string()).arg("root").arg("--name").arg(name);
        with_password(command)
    }

    /// Whether it says `machine` is online, by `fastctx nodes`.
    async fn sees(&self, machine: &str, online: bool, within: Duration) -> anyhow::Result<Option<Duration>> {
        let began = Instant::now();
        while began.elapsed() < within {
            let mut command = earlier(&self.program, &self.home);
            command.arg("nodes");
            let output = crate::process::run(
                command,
                None,
                within.saturating_sub(began.elapsed()).min(Duration::from_secs(10)),
            )?;
            anyhow::ensure!(output.status.success(), "the earlier nodes command failed: {}", output.text());
            let seen = output.out.lines().find_map(|line| {
                let rest = line.strip_prefix(machine)?;
                if !rest.starts_with(' ') {
                    return None;
                }
                let state = rest.trim_start();
                if state.starts_with("online") {
                    Some(true)
                } else if state.starts_with("offline") {
                    Some(false)
                } else {
                    None
                }
            });
            if seen == Some(online) {
                return Ok(Some(began.elapsed()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(None)
    }

    fn kill(&self) -> anyhow::Result<()> {
        crate::process::stop(&mut self.child.lock().expect("never poisoned"))
    }
}

impl Drop for Earlier {
    fn drop(&mut self) {
        if let Err(error) = self.kill() {
            crate::process::failed(error);
        }
    }
}

/// Whether something takes connections at the local socket `path`.
async fn answers_at(path: &Path) -> bool {
    #[cfg(unix)]
    {
        matches!(tokio::time::timeout(Duration::from_secs(1), tokio::net::UnixStream::connect(path)).await, Ok(Ok(_)))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// The number the hub's next letter to `member` gets, by its records.
fn next_for(world: &World, member: &str) -> Option<u64> {
    let letters = world.hub.letters()?;
    let record = letters.get(member).or_else(|| letters.get("members")?.get(member))?;
    record["next"].as_u64()
}

/// The last lines a command printed.
fn tail(said: &Said) -> String {
    let text = said.text();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(3)..].join(" / ")
}
