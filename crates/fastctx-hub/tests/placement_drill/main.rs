//! Real nodes and an isolated hub, driven by scripts/placement.py and its loopback model.

// A drill drives the hub from outside it, and is not held to the roles of
// the hub's threads (`clippy.toml`).
#![allow(clippy::disallowed_methods)]

mod attachments;
mod colleagues;
mod controls;
mod earlier;
mod goals;
mod history;
mod lifecycle;
mod release_colleague;
mod resume;
mod scale;
mod shared_controls;
mod sharing;
mod skills;
use anyhow::{Context, Result};
use fastctx_client::World;
use fastctx_net::{
    link::{Membership, NetDir},
    login,
    sign::SecretKey,
};
use fastctx_types::accounts::{AccountChange, MemberKind};
use fastctx_types::focus::{Focus, FocusedMachine};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::management::Management;
use fastctx_types::{AccountName, AgentName, MachineName};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

static KERNEL: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
const PASS: &str = "Testing-Waters-74!";
static PASSED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static FAILED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn name(s: &str) -> AccountName {
    s.parse().unwrap()
}
fn machine(s: &str) -> MachineName {
    s.parse().unwrap()
}
fn record(what: &str, ok: bool, detail: Value) {
    if ok { &PASSED } else { &FAILED }.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let detail: String = detail.to_string().chars().take(900).collect();
    println!("{} {what} {detail}", if ok { "PASS" } else { "FAIL" });
}
async fn pause() {
    tokio::time::sleep(Duration::from_millis(250)).await;
}
async fn ask(root: &Path, call: Value) -> Result<Value> {
    let mut client =
        fastctx_client::Client::connect(&WorldDir::under(root), Peer::Command { resident: None, session: None })
            .await?;
    Ok(serde_json::to_value(client.call(serde_json::from_value(call)?).await?)?)
}
fn command(root: &Path, bin: &Path) -> Command {
    let base = root.parent().unwrap();
    let mut c = Command::new(bin);
    c.env_clear()
        .env("PATH", format!("{}/bin:/usr/bin:/bin", base.display()))
        .env("HOME", base.join("user"))
        .env("FASTCTX_HOME", root)
        .env("FASTCTX_PREVIEW", "1")
        .env("CLAUDE_CONFIG_DIR", base.join("claude"))
        .env("CODEX_HOME", base.join("codex"))
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .env("LANG", "C.UTF-8")
        .current_dir(base.join("user"))
        .stdin(Stdio::null());
    strictly(&mut c);
    c
}
/// A kernel run as the drill was: a development one run strictly stops at
/// the first rule of its threads broken and says how it got there; one
/// tracing tells the first at each place and runs on.
fn strictly(command: &mut Command) {
    if let Some(strict) = std::env::var_os("FASTCTX_STRICT_ROLES") {
        command.env("FASTCTX_STRICT_ROLES", strict).env("RUST_BACKTRACE", "1");
    }
    if let Some(traced) = std::env::var_os("FASTCTX_TRACE_ROLES") {
        command.env("FASTCTX_TRACE_ROLES", traced);
    }
}
async fn device(
    base: &Path,
    label: &str,
    account: &str,
    listen: SocketAddr,
) -> Result<(std::sync::Arc<fastctx_client::device::Device>, World)> {
    let cache = fastctx_client::cache::Cache::open(base.join(label).join("cache"))?;
    let d = fastctx_client::device::Device::open_managed(base.join(label).join("device"), cache.clone())?;
    d.login(vec![listen], name(account), PASS.into(), Some(machine(label))).await?;
    for _ in 0..150 {
        if d.state().hubs.iter().any(|h| h.logins.contains(&name(account))) {
            return Ok((d.clone(), World::from_device(d).keeping(cache)));
        }
        pause().await;
    }
    anyhow::bail!("device {label} not logged in")
}
fn start(root: &Path) -> Result<Child> {
    start_with(root, KERNEL.get().expect("kernel path"))
}
/// A node as a release runs it for its users: no preview of what is not
/// offered yet.
fn start_without_preview(root: &Path) -> Result<Child> {
    let output = std::fs::OpenOptions::new().create(true).append(true).open(root.join("stdout.log"))?;
    let mut command = command(root, KERNEL.get().expect("kernel path"));
    command.env_remove("FASTCTX_PREVIEW");
    Ok(command.arg("node").stdout(output.try_clone()?).stderr(output).spawn()?)
}
fn start_with(root: &Path, program: &Path) -> Result<Child> {
    let output = std::fs::OpenOptions::new().create(true).append(true).open(root.join("stdout.log"))?;
    Ok(command(root, program).arg("node").stdout(output.try_clone()?).stderr(output).spawn()?)
}
async fn online(root: &Path) -> bool {
    for _ in 0..240 {
        if let Ok(v) = ask(root, json!({"call": "nodes"})).await
            && v.to_string().contains("online")
        {
            return true;
        }
        pause().await;
    }
    false
}
async fn node(base: &Path, label: &str, account: &str, listen: SocketAddr) -> Result<(PathBuf, Child)> {
    node_with(base, label, account, listen, None, true).await
}
/// A machine logged in to `account`, its node this build's or, given
/// `earlier`, an earlier release's, run from where its World runs it from:
/// started from anywhere else, a release places itself there and hands the
/// World to a successor of its own, which the drill would not hold. Without
/// `preview`, a release offers what it offers its users and no more.
async fn node_with(
    base: &Path,
    label: &str,
    account: &str,
    listen: SocketAddr,
    earlier: Option<&Path>,
    preview: bool,
) -> Result<(PathBuf, Child)> {
    let root = base.join(label);
    let dir = WorldDir::under(&root);
    std::fs::create_dir_all(dir.path())?;
    let key = SecretKey::generate()?;
    let joined = login::with_password(
        &[listen],
        &name(account),
        PASS,
        login::Device { name: machine(label), key: &key, kind: MemberKind::Computer, register: true },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    key.create(&dir.device_key())?;
    std::fs::write(dir.machine_file(), serde_json::to_vec(&json!({"name": label, "was": []}))?)?;
    let net = NetDir::new(dir.net().join(joined.hub.as_str()));
    net.prepare()?;
    let m = Membership {
        id: joined.hub,
        hub_name: joined.hub_name,
        name: joined.name,
        addresses: vec![listen],
        cert: joined.cert,
        kind: MemberKind::Computer,
        relays: joined.relays,
    };
    fastctx_net::link::state::write(&net.membership(), &m)?;
    std::fs::create_dir_all(root.join("bin"))?;
    let installed = root.join("bin/fastctx");
    let program = earlier.unwrap_or(KERNEL.get().expect("kernel"));
    if std::fs::hard_link(program, &installed).is_err() {
        std::fs::copy(program, &installed)?;
    }
    let child = match earlier {
        Some(_) => start_with(&root, &installed)?,
        None if preview => start(&root)?,
        None => start_without_preview(&root)?,
    };
    anyhow::ensure!(online(&root).await, "node {label} not ready");
    Ok((root, child))
}
fn block(host: &Path, resident: &str, name: &str) -> String {
    std::fs::read_to_string(
        host.join("world/conversations").join(resident).join("context").join(format!("{name}.json")),
    )
    .unwrap_or_default()
}
/// The conversation's state, as its `now` file on `host` tells it.
fn now(host: &Path, resident: &str) -> Value {
    std::fs::read(host.join("world/conversations").join(resident).join("now"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// The last turn the conversation took, before it is spoken to or opened.
fn last_turn(host: &Path, resident: &str) -> Option<String> {
    now(host, resident)["last_turn"].as_str().map(str::to_string)
}

/// Waits for the conversation to be idle after a turn other than `before`:
/// until the turn it was given begins, it is idle from the one before.
async fn idle_after(host: &Path, resident: &str, before: Option<String>) -> bool {
    for _ in 0..480 {
        let now = now(host, resident);
        if now["activity"] == "idle" && now["last_turn"].as_str().is_some_and(|last| Some(last) != before.as_deref()) {
            return true;
        }
        pause().await;
    }
    false
}

/// Waits for the conversation to be in a turn.
async fn working(host: &Path, resident: &str) -> bool {
    for _ in 0..480 {
        if !now(host, resident)["turn"].is_null() {
            return true;
        }
        pause().await;
    }
    false
}
/// The hub as its own process, as it runs: stopping it ends every connection to it.
fn serve(base: &Path) -> Result<Child> {
    let output = std::fs::OpenOptions::new().create(true).append(true).open(base.join("hub.log"))?;
    let mut command = Command::new(KERNEL.get().expect("kernel path"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", base.join("user"))
        .env("LANG", "C.UTF-8")
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .args(["hub", "serve", "--dir"])
        .arg(base.join("hub"))
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    strictly(&mut command);
    Ok(command.spawn()?)
}

fn focused(name: &str) -> Focus {
    Focus { machines: vec![FocusedMachine { machine: name.parse().unwrap(), folders: Vec::new() }] }
}
fn plan_file(host: &Path, resident: &str) -> Result<PathBuf> {
    let receipt: Value = serde_json::from_slice(&std::fs::read(
        host.join("world/conversations").join(resident).join("working-place.json"),
    )?)?;
    Ok(PathBuf::from(receipt["path"].as_str().context("working place has no path")?).join("plan.md"))
}
async fn file_is(path: &Path, text: &str) -> bool {
    for _ in 0..240 {
        if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
            return true;
        }
        pause().await;
    }
    false
}
async fn plan_is(world: &World, resident: &fastctx_types::ResidentId, text: &str) -> bool {
    for _ in 0..240 {
        if let Ok(page) = world.plan_read(None, None, resident.clone(), Some(machine("host"))).await
            && page.body.as_ref().is_some_and(|body| body.markdown == text)
        {
            return true;
        }
        pause().await;
    }
    false
}

#[tokio::main]
async fn main() -> Result<()> {
    let base = PathBuf::from(std::env::args().nth(1).context("base")?);
    KERNEL.set(PathBuf::from(std::env::args().nth(2).context("kernel")?)).expect("one kernel");
    std::fs::create_dir_all(&base)?;
    if std::env::args().any(|argument| argument == "--scale") {
        return scale::run(&base).await;
    }
    let mock_configuration = std::fs::read(base.join("codex/config.toml"))?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let listen = listener.local_addr()?;
    drop(listener);
    fastctx_hub::init(
        &base.join("hub"),
        fastctx_hub::New {
            name: "lab".parse()?,
            public: vec![listen],
            listen,
            admin: name("owner"),
            password: PASS.into(),
        },
    )?;
    let mut hub = serve(&base)?;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(listen).await.is_ok() {
            break;
        }
        pause().await;
    }
    let (owner_device, wo) = device(&base, "owner-phone", "owner", listen).await?;
    for a in ["alice", "bob", "mobile"] {
        wo.account(
            None,
            name("owner"),
            AccountChange::OpenAccount { name: name(a), password: PASS.into(), visitor: true },
        )
        .await?;
    }
    let (host, mut host_node) = node(&base, "host", "owner", listen).await?;
    let (owner_pc, mut owner_pc_node) = node(&base, "owner-pc", "owner", listen).await?;
    let (alice_pc, mut alice_node) = node(&base, "alice-pc", "alice", listen).await?;
    let (alice_two, mut alice_two_node) = node(&base, "alice-two", "alice", listen).await?;
    let (_bob_pc, mut bob_node) = node(&base, "bob-pc", "bob", listen).await?;
    let (alice_device, wa) = device(&base, "alice-phone", "alice", listen).await?;
    let (bob_device, wb) = device(&base, "bob-phone", "bob", listen).await?;
    let (mobile_device, wm) = device(&base, "mobile-phone", "mobile", listen).await?;
    for (world, tag) in [(&wo, "OWNER"), (&wa, "ALICE"), (&wb, "BOB"), (&wm, "MOBILE")] {
        use fastctx_types::skill_files::{File, Reply, Request};
        let mut script = File::text("scripts/run.sh", &format!("#!/bin/sh\nprintf '%s\\n' {tag}_SKILL_EXECUTED\n"));
        script.executable = true;
        let files = vec![
            File::text(
                "SKILL.md",
                &format!("---\nname: kit\ndescription: {tag}_SKILL_DESCRIPTION\n---\n{tag}_SKILL_BODY\n"),
            ),
            script,
            File::text("references/note.txt", &format!("{tag}_REFERENCE")),
        ];
        let reply = world.skill_files(None, None, Request::Write { name: "kit".into(), expected: None, files }).await?;
        anyhow::ensure!(matches!(reply, Reply::Written { .. }), "skill creation failed");
    }
    let home = base.join("codex");
    let agent: AgentName = "codex".parse()?;
    wo.add_agent_login("host".parse()?, agent.clone(), home.clone()).await.context("adding the mock Codex login")?;
    wo.change_agent_login("host".parse()?, agent.clone(), home.clone(), true, true, true, None, Some(true))
        .await
        .context("enabling the mock Codex login")?;
    for account in ["alice", "bob", "mobile"] {
        wo.account(
            None,
            name("owner"),
            AccountChange::Assist {
                machine: machine("host"),
                to: name(account),
                agent: agent.clone(),
                home: Some(home.clone()),
                assisting: true,
                until: None,
                weekly: None,
                daily: None,
                five_hour: None,
                turns: None,
            },
        )
        .await
        .context("granting the mock login assistance")?;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let choices = fastctx_types::agents::Choices { home: Some(home.clone()), ..Default::default() };
    let sa = wa
        .open_with(
            agent.clone(),
            None,
            Some("host".parse()?),
            "ALICE_OPEN".into(),
            Default::default(),
            fastctx_types::agents::Choices { focus: focused("alice-pc"), ..choices.clone() },
        )
        .await
        .context("opening alice conversation")?;
    let sb = wb
        .open_with(agent.clone(), None, Some("host".parse()?), "BOB_OPEN".into(), Default::default(), choices.clone())
        .await
        .context("opening another conversation")?;
    let so = wo
        .open_with(agent.clone(), None, Some("host".parse()?), "OWNER_OPEN".into(), Default::default(), choices.clone())
        .await
        .context("opening another conversation")?;
    let sm = wm
        .open_with(
            agent.clone(),
            None,
            Some("host".parse()?),
            "MOBILE_OPEN".into(),
            Default::default(),
            fastctx_types::agents::Choices { focus: focused("host"), ..choices.clone() },
        )
        .await
        .context("opening another conversation")?;
    let (a, b, o, m) = (
        sa.to.resident.to_string(),
        sb.to.resident.to_string(),
        so.to.resident.to_string(),
        sm.to.resident.to_string(),
    );
    record(
        "four conversations settle",
        idle_after(&host, &a, None).await
            && idle_after(&host, &b, None).await
            && idle_after(&host, &o, None).await
            && idle_after(&host, &m, None).await,
        json!([a, b, o, m]),
    );
    let pa = plan_file(&host, &a)?;
    let pb = plan_file(&host, &b)?;
    let po = plan_file(&host, &o)?;
    record("assisted plan belongs on the focused working computer", pa.starts_with(&alice_pc), json!(pa));
    record(
        "owner plan stays on its own host",
        po == host.join("world/conversations").join(&o).join("plan.md"),
        json!(po),
    );
    let owner_terminal = World::new(WorldDir::under(&owner_pc), Peer::Command { resident: None, session: None });
    let before = last_turn(&host, &o);
    owner_terminal.talk(so.to.resident.clone(), Some("host".parse()?), "OWNER_FROM_SECOND_COMPUTER".into()).await?;
    anyhow::ensure!(idle_after(&host, &o, before).await, "owner's second input did not settle");
    let spoken: MachineName =
        serde_json::from_slice(&std::fs::read(host.join("world/conversations").join(&o).join("working-from.json"))?)?;
    record(
        "the owner's host precedes another speaking computer",
        spoken == machine("owner-pc") && plan_file(&host, &o)? == po,
        json!(spoken),
    );
    record("the other assisted account has its own computer", pb.starts_with(base.join("bob-pc")), json!(pb));
    record(
        "no assisted plan.md is present on host",
        !host.join("world/conversations").join(&a).join("plan.md").exists()
            && !host.join("world/conversations").join(&b).join("plan.md").exists(),
        json!(null),
    );
    record(
        "a phone-only account cannot place files on a shared computer even when focused there",
        block(&host, &m, "plan-guide").contains("no online working computer"),
        json!(block(&host, &m, "plan-guide")),
    );
    wo.account(
        None,
        name("owner"),
        AccountChange::Share { machine: machine("host"), to: name("mobile"), shared: true, jobs: None },
    )
    .await?;
    let before = last_turn(&host, &m);
    wm.talk(
        sm.to.resident.clone(),
        Some("host".parse()?),
        "MOBILE_SHARED_SKILL [[run: on host <<'SHARED'\nprintf reached > \"$FASTCTX_SCRATCH/shared-ran\"\nfastctx skill kit\nSHARED]]".into(),
    )
    .await?;
    anyhow::ensure!(idle_after(&host, &m, before).await, "the shared-computer skill probe did not settle");
    record(
        "shared-machine work permission is unchanged",
        host.join("world/conversations").join(&m).join("scratch/shared-ran").exists(),
        json!(null),
    );
    record(
        "sharing does not materialize the recipient's skill on the provider",
        !host.join("world/skill-copies").exists(),
        json!(null),
    );
    let script = "ALICE_SKILL_RUN [[run: on alice-pc <<'SKILL'\nset -e\ntext=$(fastctx skill kit)\nprintf '%s\\n' \"$text\"\nfolder=$(printf '%s\\n' \"$text\" | sed -n 's/^Skill folder on [^:]*: //p')\n\"$folder/scripts/run.sh\"\ncat \"$folder/references/note.txt\"\nSKILL]]";
    let before = last_turn(&host, &a);
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), script.into()).await?;
    anyhow::ensure!(idle_after(&host, &a, before).await, "skill turn did not finish");
    record(
        "skill bodies are absent from the assisted host's context cache",
        !block(&host, &a, "skills").contains("ALICE_SKILL_DESCRIPTION"),
        json!(null),
    );
    record(
        "skill folders are not materialized on the assisting host",
        !host.join("world/skill-copies").exists(),
        json!(null),
    );
    skills::exercise(&wa, &host, &alice_pc, &sa.to.resident, script).await?;
    let before = last_turn(&host, &a);
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), format!("ALICE_FILE_WRITE [[run: on alice-pc <<'WRITE'\nfastctx view '{}'\nwrite '{}' <<'TEXT'\n# Plan\nALICE_FILE_CANARY\nTEXT\nWRITE]]", pa.display(), pa.display())).await?;
    anyhow::ensure!(idle_after(&host, &a, before).await, "the controlled plan write did not settle");
    record(
        "a world write reaches the account",
        plan_is(&wa, &sa.to.resident, "# Plan\nALICE_FILE_CANARY\n").await,
        json!(null),
    );
    let second_plan = WorldDir::under(&alice_two)
        .owner(&fastctx_types::layout::Names { me: machine("alice-two"), was: Vec::new(), renamed: Vec::new() }, &sa.to)
        .path()
        .join("plan.md");
    record(
        "every eligible computer keeps the plan before focus changes",
        file_is(&second_plan, "# Plan\nALICE_FILE_CANARY\n").await,
        json!(second_plan),
    );
    let permissions = std::fs::metadata(&pa)?.permissions();
    let mut writable = permissions.clone();
    #[allow(clippy::permissions_set_readonly_false)]
    writable.set_readonly(false);
    std::fs::set_permissions(&pa, writable)?;
    std::fs::write(&pa, "UNCONTROLLED_TAMPERING")?;
    ask(&alice_pc, json!({"call":"prepare_copies","paths":[pa]})).await?;
    record(
        "uncontrolled edits are restored without a new hub version",
        file_is(&pa, "# Plan\nALICE_FILE_CANARY\n").await
            && plan_is(&wa, &sa.to.resident, "# Plan\nALICE_FILE_CANARY\n").await,
        json!(null),
    );
    let page = wa.plan_read(None, None, sa.to.resident.clone(), Some(machine("host"))).await?;
    wa.plan_write(
        None,
        None,
        sa.to.resident.clone(),
        page.version(),
        "# Plan\nALICE_UI_CANARY\n".into(),
        Some(machine("host")),
    )
    .await?;
    record("panel save reaches the copy", file_is(&pa, "# Plan\nALICE_UI_CANARY\n").await, json!(null));
    let before = last_turn(&host, &a);
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), "ALICE_READ_AFTER_UI".into()).await?;
    anyhow::ensure!(idle_after(&host, &a, before).await, "the read after the panel's save did not settle");
    record(
        "plan projection stores no body on the host",
        !block(&host, &a, "plan").contains("ALICE_UI_CANARY"),
        json!(block(&host, &a, "plan")),
    );
    let mobile = wm
        .plan_write(None, None, sm.to.resident.clone(), None, "MOBILE_PLAN_CANARY".into(), Some(machine("host")))
        .await?;
    record(
        "phone-only can still edit its hub record",
        matches!(mobile, fastctx_types::plan::Result::Page { .. }),
        json!(null),
    );
    let before = last_turn(&host, &a);
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), "ALICE_HOLD [[hang]]".into()).await?;
    anyhow::ensure!(working(&host, &a).await, "the held turn did not begin");
    wa.manage(sa.to.resident.clone(), Some("host".parse()?), Management::Focus { focus: focused("alice-two") }).await?;
    record(
        "changing focus during a native turn does not change the path told to it",
        plan_file(&host, &a)? == pa,
        json!(null),
    );
    wa.stop(sa.to.resident.clone(), Some("host".parse()?)).await?;
    anyhow::ensure!(idle_after(&host, &a, before).await, "the held turn did not stop");
    let local_home = base.join("alice-codex");
    std::fs::create_dir_all(&local_home)?;
    std::fs::write(local_home.join("config.toml"), &mock_configuration)?;
    wa.add_agent_login("alice-pc".parse()?, agent.clone(), local_home.clone()).await?;
    let installed = wa
        .change_agent_login("alice-pc".parse()?, agent.clone(), local_home.clone(), true, true, true, None, Some(true))
        .await?;
    anyhow::ensure!(
        matches!(installed.end, Some(fastctx_types::logins::ChangeEnd::Done { .. })),
        "installing the local mock tools failed: {installed:?}"
    );
    let mut login_ready = false;
    for _ in 0..120 {
        let listed = serde_json::to_value(wa.agents(Some("alice-pc".parse()?)).await?)?;
        if listed.to_string().contains(&local_home.to_string_lossy().to_string()) {
            login_ready = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(login_ready, "the local mock login was never advertised");
    let writer = wa
        .open_with(
            agent.clone(),
            None,
            Some("alice-pc".parse()?),
            "ALICE_LOCAL_WRITER".into(),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(local_home), ..Default::default() },
        )
        .await?;
    anyhow::ensure!(
        idle_after(&alice_pc, writer.to.resident.as_str(), None).await,
        "the offline writer did not settle"
    );
    let offline_scope = fastctx_types::history::Scope::Conversation {
        resident: sa.to.resident.clone(),
        machine: sa.to.machine.clone(),
    };
    let offline_original = "---\nname: offline-history\ndescription: Offline replay fixture\n---\nBASE\n";
    wa.history(fastctx_types::history::HistoryCall::Save {
        scope: offline_scope.clone(),
        name: "offline-history".into(),
        text: offline_original.into(),
        expected: None,
    })
    .await?;
    let offline_path = pa.parent().unwrap().join("history/offline-history.md");
    anyhow::ensure!(file_is(&offline_path, offline_original).await, "offline history did not reach its computer");
    hub.kill()?;
    hub.wait()?;
    for _ in 0..120 {
        if !online_now(&alice_pc, "host").await {
            break;
        }
        pause().await;
    }
    write_as(&alice_pc, &writer.to, &pa, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await?;
    record(
        "a disconnected world write is durable locally",
        file_is(&pa, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        json!(null),
    );
    let offline_text = offline_original.replace("BASE", "OFFLINE_HISTORY");
    write_as(&alice_pc, &writer.to, &offline_path, &offline_text).await?;
    alice_node.kill()?;
    alice_node.wait()?;
    hub = serve(&base)?;
    anyhow::ensure!(
        plan_is(&wa, &sa.to.resident, "# Plan\nALICE_UI_CANARY\n").await,
        "the hub did not return before replay"
    );
    let online_text = offline_original.replace("BASE", "ONLINE_HISTORY");
    wa.history(fastctx_types::history::HistoryCall::Save {
        scope: offline_scope.clone(),
        name: "offline-history".into(),
        text: online_text.clone(),
        expected: Some(1),
    })
    .await?;
    alice_node = start(&alice_pc)?;
    anyhow::ensure!(online(&alice_pc).await, "the computer did not reconnect");
    record(
        "offline history survives node restart and publishes a conflicting draft as a new version",
        file_is(&offline_path, &offline_text).await,
        json!(null),
    );
    let mut replayed = false;
    for _ in 0..240 {
        if let Ok(fastctx_types::history::HistoryReply::Document { document }) = wa
            .history(fastctx_types::history::HistoryCall::Read {
                scope: offline_scope.clone(),
                name: "offline-history".into(),
                version: None,
            })
            .await
            && document.revision.version == 3
            && document.text == offline_text
        {
            replayed = true;
            break;
        }
        pause().await;
    }
    let preserved = wa
        .history(fastctx_types::history::HistoryCall::Read {
            scope: offline_scope,
            name: "offline-history".into(),
            version: Some(2),
        })
        .await?;
    record(
        "offline conflict preserves the competing hub revision without duplicate replay",
        replayed
            && matches!(preserved, fastctx_types::history::HistoryReply::Document { document } if document.text == online_text && document.versions.len() == 3),
        json!(null),
    );
    record(
        "offline edits are submitted on return",
        plan_is(&wa, &sa.to.resident, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        json!(null),
    );
    let mut reachable = false;
    for _ in 0..240 {
        if wa.agents(Some("host".parse()?)).await.is_ok() {
            reachable = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(reachable, "the assisting host did not reconnect");
    wa.manage(sa.to.resident.clone(), Some("host".parse()?), Management::Focus { focus: focused("alice-two") }).await?;
    let before = last_turn(&host, &a);
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), "ALICE_NEW_PLACE".into()).await?;
    anyhow::ensure!(idle_after(&host, &a, before).await, "the new-place turn did not settle");
    let moved = plan_file(&host, &a)?;
    record("the next turn gets the selected replica path", moved.starts_with(&alice_two) && moved != pa, json!(moved));
    record(
        "both copies retain the recovered edit",
        file_is(&pa, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await
            && file_is(&moved, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        json!(null),
    );
    write_as(&alice_pc, &writer.to, &pa, "").await?;
    record(
        "the former selected copy remains writable through the world",
        plan_is(&wa, &sa.to.resident, "").await && file_is(&moved, "").await,
        json!(null),
    );
    write_as(&alice_pc, &writer.to, &pa, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await?;
    anyhow::ensure!(
        file_is(&moved, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        "the restored plan did not reach every copy"
    );
    attachments::exercise(&wa, &host, &moved, &sa.to.resident).await?;
    controls::exercise(&wa, &host, &alice_two, &sa.to.resident, &moved).await?;
    resume::exercise(&wa, &host, &sa.to.resident).await?;
    let colleague = colleagues::exercise(&wa, &host, &alice_two, &sa.to.resident, &home).await?;
    std::fs::write(base.join("colleague.json"), serde_json::to_vec(&colleague)?)?;
    let release_colleague = release_colleague::exercise(&base, &wo, listen, &mock_configuration, &agent).await?;
    if let Some(earlier_kernel) = std::env::var_os("FASTCTX_DRILL_EARLIER") {
        earlier::exercise(
            &base,
            &wa,
            listen,
            &host,
            &sa.to.resident,
            &agent,
            &mock_configuration,
            Path::new(&earlier_kernel),
        )
        .await?;
    }
    let endpoint = std::env::args().nth(3).context("goal mock endpoint")?;
    goals::exercise(&wa, &host, &moved, &sa.to.resident, &endpoint).await?;
    history::exercise(&wa, &host, &alice_two, &sa.to.resident, &writer.to).await?;
    sharing::exercise(&base, &host, &owner_pc, &wo, &wb, &so.to.resident, &sb.to, &home).await?;
    host_node.kill()?;
    host_node.wait()?;
    let host_offline =
        fastctx_types::history::Scope::Conversation { resident: sa.to.resident.clone(), machine: machine("host") };
    let offline_saved = wa
        .history(fastctx_types::history::HistoryCall::Save {
            scope: host_offline.clone(),
            name: "host-offline".into(),
            text: "---\nname: host-offline\ndescription: Offline host access\n---\nHOST_OFFLINE_HISTORY\n".into(),
            expected: None,
        })
        .await;
    record(
        "a phone reads and edits history while its model's computer is off",
        offline_saved.is_ok()
            && wa
                .history(fastctx_types::history::HistoryCall::Read {
                    scope: host_offline,
                    name: "host-offline".into(),
                    version: None,
                })
                .await
                .is_ok(),
        json!(null),
    );
    lifecycle::exercise(&base, &wa, &alice_pc, &alice_two, &mut alice_node, &mut alice_two_node, listen).await?;
    std::fs::write(
        base.join("expectations.json"),
        serde_json::to_vec(
            &json!({"alice":a,"bob":b,"owner":o,"mobile":m,"old":pa,"new":moved,"host":host,"owner_plan":po,"bob_plan":pb,
                    "release_colleague":release_colleague}),
        )?,
    )?;
    alice_device.disconnect();
    bob_device.disconnect();
    owner_device.disconnect();
    mobile_device.disconnect();
    for child in [&mut host_node, &mut owner_pc_node, &mut alice_node, &mut alice_two_node, &mut bob_node, &mut hub] {
        let _ = child.kill();
        let _ = child.wait();
    }
    let passed = PASSED.load(std::sync::atomic::Ordering::Relaxed);
    let failed = FAILED.load(std::sync::atomic::Ordering::Relaxed);
    println!("RESULT {passed} passed, {failed} failed");
    anyhow::ensure!(failed == 0, "placement assertions failed");
    Ok(())
}
async fn online_now(root: &Path, label: &str) -> bool {
    ask(root, json!({"call":"nodes"}))
        .await
        .ok()
        .is_some_and(|value| value.to_string().contains(&format!("\"name\":\"{label}\",\"online\":true")))
}

async fn write_as(root: &Path, owner: &fastctx_types::Address, path: &Path, text: &str) -> Result<()> {
    operation_as(root, owner, json!({"tool":"write","file_path":path,"content":text})).await
}

async fn operation_as(root: &Path, owner: &fastctx_types::Address, operation: Value) -> Result<()> {
    let mut client = fastctx_client::Client::connect(
        &WorldDir::under(root),
        Peer::Command { resident: Some(owner.clone()), session: None },
    )
    .await?;
    let call = serde_json::from_value(
        json!({"call":"write","resident":null,"node":null,"town":null,"job":null,"op":operation}),
    )?;
    let fastctx_types::Reply::Written { outcome } = client.call(call).await? else {
        anyhow::bail!("the world did not answer the write")
    };
    outcome.map_err(|error| anyhow::anyhow!(error.message))?;
    Ok(())
}
