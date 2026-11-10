//! Optional real-node scale measurements, isolated by the placement launcher.
use super::*;
use fastctx_types::{
    Address, ResidentId,
    history::{HistoryCall, HistoryReply, Scope},
};

const CONVERSATIONS: usize = 300;
const COMPUTERS: usize = 20;
fn document(index: usize, changed: bool) -> String {
    format!(
        "---\nname: evidence\ndescription: Scale fixture {index}\n---\n2026-10-31: {} {}\n",
        if changed { "CHANGED" } else { "ORIGINAL" },
        "measured evidence ".repeat(60)
    )
}
fn scope(index: usize) -> Scope {
    Scope::Conversation {
        resident: format!("codex-{:08x}", index + 0x10000000).parse().unwrap(),
        machine: machine("scale-origin"),
    }
}
fn resident(index: usize) -> ResidentId {
    let Scope::Conversation { resident, .. } = scope(index) else { unreachable!() };
    resident
}
fn bytes() -> Result<u64> {
    let text = std::fs::read_to_string("/proc/net/dev")?;
    let line = text.lines().find(|line| line.trim_start().starts_with("lo:")).context("loopback counter")?;
    Ok(line.split_once(':').unwrap().1.split_whitespace().next().unwrap().parse()?)
}
fn process(pid: u32) -> Result<Value> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let rss: u64 = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .context("process RSS")?
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat.rsplit_once(')').context("process stat")?.1.split_whitespace().collect();
    let ticks = fields[11].parse::<u64>()? + fields[12].parse::<u64>()?;
    Ok(json!({"pid":pid,"rss_kib":rss,"cpu_ticks":ticks}))
}
fn resources(hub: &Child, computers: &[(PathBuf, Child)]) -> Result<Value> {
    let nodes = computers.iter().map(|(_, child)| process(child.id())).collect::<Result<Vec<_>>>()?;
    Ok(json!({"hub":process(hub.id())?,"nodes":nodes}))
}
async fn copied(root: &Path, label: &str, changed: bool) -> Result<()> {
    let began = std::time::Instant::now();
    loop {
        let names = fastctx_types::layout::Names { me: machine(label), was: Vec::new(), renamed: Vec::new() };
        let complete = (0..CONVERSATIONS).all(|index| {
            let address = Address { machine: machine("scale-origin"), resident: resident(index) };
            let folder = WorldDir::under(root).owner(&names, &address).path().to_path_buf();
            std::fs::read_to_string(folder.join("plan.md")).ok().as_deref() == Some("SCALE_PLAN")
                && std::fs::read_to_string(folder.join("history/evidence.md")).ok().as_deref()
                    == Some(document(index, changed && index == 0).as_str())
        });
        if complete {
            return Ok(());
        }
        anyhow::ensure!(began.elapsed() < Duration::from_secs(240), "{label} did not receive every document");
        pause().await;
    }
}

pub(super) async fn run(base: &Path) -> Result<()> {
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
    let mut hub = serve(base)?;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(listen).await.is_ok() {
            break;
        }
        pause().await;
    }
    let (device, world) = device(base, "scale-phone", "owner", listen).await?;
    // Notes are off until the account turns them on.
    world.history_switch_write(None, Some(name("owner")), true).await?;
    let (origin, mut origin_node) = node(base, "scale-origin", "owner", listen).await?;
    origin_node.kill()?;
    origin_node.wait()?;
    for index in 0..CONVERSATIONS {
        lifecycle::seed(&origin, "scale-origin", &resident(index), None)?;
    }
    origin_node = start(&origin)?;
    anyhow::ensure!(online(&origin).await, "scale origin did not restart");
    let seeded = std::time::Instant::now();
    for index in 0..CONVERSATIONS {
        let mut registered = false;
        for _ in 0..240 {
            if world.history(HistoryCall::List { scope: scope(index) }).await.is_ok() {
                registered = true;
                break;
            }
            pause().await;
        }
        anyhow::ensure!(registered, "scale conversation {index} not registered");
        world.plan_write(None, None, resident(index), None, "SCALE_PLAN".into(), Some(machine("scale-origin"))).await?;
        world
            .history(HistoryCall::Save {
                scope: scope(index),
                name: "evidence".into(),
                text: document(index, false),
                expected: None,
            })
            .await?;
    }
    copied(&origin, "scale-origin", false).await?;
    println!("SCALE seeded {CONVERSATIONS} conversations in {:.3}s", seeded.elapsed().as_secs_f64());
    let mut computers = vec![(origin, origin_node)];
    for index in 1..COMPUTERS - 1 {
        let label = format!("scale-{index}");
        let (root, child) = node(base, &label, "owner", listen).await?;
        copied(&root, &label, false).await?;
        computers.push((root, child));
        println!("SCALE ready {} computers", computers.len());
    }
    let before_join = resources(&hub, &computers)?;
    let join_bytes = bytes()?;
    let started = std::time::Instant::now();
    let label = format!("scale-{}", COMPUTERS - 1);
    let (last, child) = node(base, &label, "owner", listen).await?;
    copied(&last, &label, false).await?;
    let join_seconds = started.elapsed().as_secs_f64();
    let join_bytes = bytes()? - join_bytes;
    computers.push((last, child));
    let after_join = resources(&hub, &computers)?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle_start = bytes()?;
    let idle_resources = resources(&hub, &computers)?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle_bytes = bytes()? - idle_start;
    let idle_end = resources(&hub, &computers)?;
    let change_start = bytes()?;
    let before_change = resources(&hub, &computers)?;
    let started = std::time::Instant::now();
    let saved = world
        .history(HistoryCall::Save {
            scope: scope(0),
            name: "evidence".into(),
            text: document(0, true),
            expected: Some(1),
        })
        .await?;
    anyhow::ensure!(matches!(saved, HistoryReply::Saved { .. }), "scale change was not accepted");
    for (index, (root, _)) in computers.iter().enumerate() {
        let label = if index == 0 { "scale-origin".into() } else { format!("scale-{index}") };
        copied(root, &label, true).await?;
    }
    let change_seconds = started.elapsed().as_secs_f64();
    let change_bytes = bytes()? - change_start;
    let after_change = resources(&hub, &computers)?;
    let report = json!({"conversations":CONVERSATIONS,"computers":COMPUTERS,"current_documents_per_computer":CONVERSATIONS*2,
        "history_bytes":document(0,false).len(),"loopback_counter":"receive bytes, one copy of each loopback packet including transport overhead",
        "new_computer":{"seconds":join_seconds,"bytes":join_bytes,"before":before_join,"after":after_join},
        "idle":{"seconds":2,"bytes":idle_bytes,"before":idle_resources,"after":idle_end},
        "single_change":{"seconds":change_seconds,"bytes":change_bytes,"before":before_change,"after":after_change}});
    std::fs::write(base.join("scale-results.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("SCALE RESULT {}", report);
    device.disconnect();
    for (_, child) in &mut computers {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = hub.kill();
    let _ = hub.wait();
    Ok(())
}
