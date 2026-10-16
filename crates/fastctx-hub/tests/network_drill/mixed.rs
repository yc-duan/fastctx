//! The scenarios with an earlier release, as a network has them while its
//! machines update one by one: an earlier hub serving this release's
//! machines, and an earlier node on this release's hub.

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

/// How long a question waits for its answer (`fastctx_types::contract`),
/// past which the machine answering it stops it by itself.
const QUESTION: Duration = Duration::from_secs(60);

/// An earlier hub serving this release's machines: what it does not pass
/// on, giving a question up, the machine answering stops by itself; who is
/// online comes in whole directories; letters, answers of every size, cuts
/// and a crash go as with this release's hub.
pub async fn earlier_hub(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    beta.lab.stall(true);
    let asked = Instant::now();
    let gave_up = give_up(world, "alpha", "beta", 3).await;
    let link = alpha.link();
    let look = RemoteCall::Updates { update: UpdateCall::Look };
    let after = link.call(&beta.name, &world.account, look.purpose(), look).await;
    report.check(
        gave_up == 3 && alpha.online() && matches!(after, Ok(Err(_))),
        "alpha gave three questions up, and its session went on to the next call",
        format!("gave up {gave_up}"),
    );
    scenarios::answers(world, report).await?;
    scenarios::presence(world, report, false).await?;
    let stopped =
        until(QUESTION + Duration::from_secs(20) - asked.elapsed().min(QUESTION), || beta.lab.answering() == 0).await;
    let took = asked.elapsed();
    beta.lab.stall(false);
    report.check(
        stopped.is_some() && beta.lab.stopped() == 3 && took >= QUESTION - Duration::from_secs(5),
        "beta stopped the three by itself, a little past the time their caller waits",
        format!("{} stopped after {}", beta.lab.stopped(), seconds(took)),
    );

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
    sending.sent().await;
    let came = arrived(&gamma.lab, &sending.wanted, Duration::from_secs(180)).await;
    report.check(
        came.missing.is_empty(),
        "every letter reached gamma through cuts and a crash of the earlier hub",
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
        version.as_deref().is_some_and(|version| version.starts_with("1.0.1-beta.")),
        "it answered a call from this release, saying its own release",
        format!("{:?}", version.unwrap_or_else(|| format!("{facts:?}"))),
    );

    let numbered_before = next_for(world, "delta").unwrap_or(0);
    let mut sending = letters::send_to(world, &["alpha", "beta"], &delta, "earlier-node", 300);
    sending.sent().await;
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
    let taken = until(Duration::from_secs(120), || {
        world.hub.health().is_some_and(|health| health["letters"]["held"].as_u64() == Some(0))
    })
    .await;
    report.check(
        taken.is_some(),
        "it took them all: the hub holds none",
        taken.map_or("not within 120 s".to_string(), seconds),
    );

    alpha.stop();
    let gone = until(Duration::from_secs(30), || node.sees("alpha") == Some(false)).await;
    alpha.start()?;
    let back = until(Duration::from_secs(60), || node.sees("alpha") == Some(true)).await;
    report.check(
        gone.is_some() && back.is_some(),
        "it saw a machine of this release leave and come back",
        format!(
            "left {}, back {}",
            gone.map_or("never".to_string(), seconds),
            back.map_or("never".to_string(), seconds)
        ),
    );

    node.kill();
    let ended = !answers_at(&node.home.join("world").join("node.sock"));
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
    node.kill();
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
        let child = command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn()?;
        *self.child.lock().expect("never poisoned") = Some(child);
        Ok(())
    }

    /// Waits for it to take calls from this machine's programs.
    async fn ready(&self, within: Duration) -> bool {
        let socket = self.home.join("world").join("node.sock");
        until(within, || answers_at(&socket)).await.is_some()
    }

    fn login(&self, hub: SocketAddr, name: &str) -> anyhow::Result<Said> {
        let mut command = earlier(&self.program, &self.home);
        command.arg("login").arg(hub.to_string()).arg("root").arg("--name").arg(name);
        with_password(command)
    }

    /// Whether it says `machine` is online, by `fastctx nodes`.
    fn sees(&self, machine: &str) -> Option<bool> {
        let output = earlier(&self.program, &self.home).arg("nodes").stdin(Stdio::null()).output().ok()?;
        String::from_utf8_lossy(&output.stdout).lines().find_map(|line| {
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
        })
    }

    fn kill(&self) {
        if let Some(mut child) = self.child.lock().expect("never poisoned").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Earlier {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Whether something takes connections at the local socket `path`.
fn answers_at(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(path).is_ok()
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
