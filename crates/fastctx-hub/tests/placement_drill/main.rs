//! Real nodes and an isolated hub, driven by scripts/placement.py and its loopback model.
mod attachments;
mod colleagues;
mod controls;
mod goals;
mod memory;
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
    c
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
    let output = std::fs::OpenOptions::new().create(true).append(true).open(root.join("stdout.log"))?;
    Ok(command(root, KERNEL.get().expect("kernel path"))
        .arg("node")
        .stdout(output.try_clone()?)
        .stderr(output)
        .spawn()?)
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
    if std::fs::hard_link(KERNEL.get().expect("kernel"), &installed).is_err() {
        std::fs::copy(KERNEL.get().expect("kernel"), &installed)?;
    }
    let child = start(&root)?;
    anyhow::ensure!(online(&root).await, "node {label} not ready");
    Ok((root, child))
}
fn block(host: &Path, resident: &str, name: &str) -> String {
    std::fs::read_to_string(
        host.join("world/conversations").join(resident).join("context").join(format!("{name}.json")),
    )
    .unwrap_or_default()
}
async fn idle(host: &Path, resident: &str) -> bool {
    for _ in 0..480 {
        if std::fs::read_to_string(host.join("world/conversations").join(resident).join("now"))
            .unwrap_or_default()
            .contains("\"idle\"")
        {
            return true;
        }
        pause().await;
    }
    false
}
/// The hub as its own process, as it runs: stopping it ends every connection to it.
fn serve(base: &Path) -> Result<Child> {
    let output = std::fs::OpenOptions::new().create(true).append(true).open(base.join("hub.log"))?;
    Ok(Command::new(KERNEL.get().expect("kernel path"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", base.join("user"))
        .env("LANG", "C.UTF-8")
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .args(["hub", "serve", "--dir"])
        .arg(base.join("hub"))
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .spawn()?)
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
    wo.account(
        None,
        name("owner"),
        AccountChange::Share { machine: machine("host"), to: name("mobile"), shared: true, jobs: None },
    )
    .await?;
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
    wo.change_agent_login("host".parse()?, agent.clone(), home.clone(), true, true, true)
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
        idle(&host, &a).await && idle(&host, &b).await && idle(&host, &o).await && idle(&host, &m).await,
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
    owner_terminal.talk(so.to.resident.clone(), Some("host".parse()?), "OWNER_FROM_SECOND_COMPUTER".into()).await?;
    anyhow::ensure!(idle(&host, &o).await, "owner's second input did not settle");
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
    wm.talk(
        sm.to.resident.clone(),
        Some("host".parse()?),
        "MOBILE_SHARED_SKILL [[run: on host <<'SHARED'\nprintf reached > \"$FASTCTX_SCRATCH/shared-ran\"\nfastctx skill kit\nSHARED]]".into(),
    )
    .await?;
    anyhow::ensure!(idle(&host, &m).await, "the shared-computer skill probe did not settle");
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
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), script.into()).await?;
    anyhow::ensure!(idle(&host, &a).await, "skill turn did not finish");
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
    std::fs::write(&pa, "# Plan\nALICE_FILE_CANARY\n")?;
    record(
        "native file save reaches the account",
        plan_is(&wa, &sa.to.resident, "# Plan\nALICE_FILE_CANARY\n").await,
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
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), "ALICE_READ_AFTER_UI".into()).await?;
    idle(&host, &a).await;
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
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), "ALICE_HOLD [[hang]]".into()).await?;
    wa.manage(sa.to.resident.clone(), Some("host".parse()?), Management::Focus { focus: focused("alice-two") }).await?;
    record(
        "changing focus during a native turn does not move its working copy",
        plan_file(&host, &a)? == pa,
        json!(null),
    );
    wa.stop(sa.to.resident.clone(), Some("host".parse()?)).await?;
    anyhow::ensure!(idle(&host, &a).await, "the held turn did not stop");
    alice_node.kill()?;
    alice_node.wait()?;
    for _ in 0..120 {
        if !online_now(&host, "alice-pc").await {
            break;
        }
        pause().await;
    }
    std::fs::write(&pa, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n")?;
    wa.manage(sa.to.resident.clone(), Some("host".parse()?), Management::Focus { focus: focused("alice-two") }).await?;
    wa.talk(sa.to.resident.clone(), Some("host".parse()?), "ALICE_NEW_PLACE".into()).await?;
    idle(&host, &a).await;
    let moved = plan_file(&host, &a)?;
    record("the next turn gets the new place", moved.starts_with(&alice_two) && moved != pa, json!(moved));
    record("the new copy begins with the hub text", file_is(&moved, "# Plan\nALICE_UI_CANARY\n").await, json!(null));
    alice_node = start(&alice_pc)?;
    anyhow::ensure!(online(&alice_pc).await, "old computer did not restart");
    record(
        "offline edits are submitted on return",
        plan_is(&wa, &sa.to.resident, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        json!(null),
    );
    for _ in 0..240 {
        if !pa.exists() {
            break;
        }
        pause().await;
    }
    record("the old writable copy is withdrawn", !pa.exists(), json!(pa));
    record(
        "the new copy receives the recovered edit",
        file_is(&moved, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        json!(null),
    );
    std::fs::write(&pa, "")?;
    record("a late empty save at the withdrawn path is not lost", plan_is(&wa, &sa.to.resident, "").await, json!(null));
    let restored = wa.plan_read(None, None, sa.to.resident.clone(), Some(machine("host"))).await?;
    wa.plan_write(
        None,
        None,
        sa.to.resident.clone(),
        restored.version(),
        "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n".into(),
        Some(machine("host")),
    )
    .await?;
    anyhow::ensure!(
        file_is(&moved, "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n").await,
        "the restored plan did not reach the new copy"
    );
    attachments::exercise(&wa, &host, &moved, &sa.to.resident).await?;
    controls::exercise(&wa, &host, &alice_two, &sa.to.resident, &moved).await?;
    let colleague = colleagues::exercise(&wa, &host, &alice_two, &sa.to.resident, &home).await?;
    std::fs::write(base.join("colleague.json"), serde_json::to_vec(&colleague)?)?;
    let endpoint = std::env::args().nth(3).context("goal mock endpoint")?;
    goals::exercise(&wa, &host, &moved, &sa.to.resident, &endpoint).await?;
    memory::exercise(&wa, &host, &alice_two, &sa.to.resident).await?;
    std::fs::write(
        base.join("expectations.json"),
        serde_json::to_vec(
            &json!({"alice":a,"bob":b,"owner":o,"mobile":m,"old":pa,"new":moved,"host":host,"owner_plan":po,"bob_plan":pb}),
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
