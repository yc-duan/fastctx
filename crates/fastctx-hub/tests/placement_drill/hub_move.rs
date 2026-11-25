//! The hub moved to another machine of its super administrator's, as the
//! interface moves it (`HubMoveRequest`), with real nodes: its records copied
//! while it serves, the copy cut once and taken up again, the switch, the
//! machines online following the new address, one away all the while
//! reaching the new hub through the old one's forwarding and following, and
//! the network going on once the old hub is gone; the owner libraries, which
//! the hub is not stopped for, coming to the new hub once the old one serves
//! no more, and after them the large objects they hold. Run alone
//! (`--hub-move`);
//! the launcher may hold every connection to a rate (`--hub-move-rate`), and
//! `FASTCTX_DRILL_HUB_MOVE_MB` is how many MiB of records beyond the hub's
//! own go along.

use super::{PASS, ask, device, machine, name, node, online, pause, record, serve, start};
use anyhow::{Context, Result};
use fastctx_client::World;
use fastctx_types::accounts::AccountChange;
use fastctx_types::hub_move::{HubMove, HubMoveReply, HubMoveRequest, MoveStage, OldPlace, Waiting};
use fastctx_types::layout::WorldDir;
use fastctx_types::skill_files::{File, Reply, Request};
use serde_json::json;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free() -> Result<SocketAddr> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?)
}

/// The hub kept in `dir` run as its own process, as its service would.
fn serve_at(base: &Path, dir: &Path, log: &str) -> Result<Child> {
    let output = std::fs::OpenOptions::new().create(true).append(true).open(base.join(log))?;
    let mut command = Command::new(super::KERNEL.get().expect("kernel path"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", base.join("user"))
        .env("LANG", "C.UTF-8")
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .env("FASTCTX_HUB_HEALTH_SECONDS", "1")
        .args(["hub", "serve", "--dir"])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    super::strictly(&mut command);
    Ok(command.spawn()?)
}

/// The moves of the hub as its super administrator is told, newest first;
/// none while the hub cannot be asked (it hands over, say).
/// How long each file of some size under `at` is that is no database's: the
/// large objects a library holds.
fn large(at: &Path) -> Vec<u64> {
    let mut sizes = Vec::new();
    for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sizes.extend(large(&path));
        } else if !entry.file_name().to_string_lossy().contains(".sqlite3")
            && let Ok(found) = entry.metadata()
            && found.len() > 64 << 10
        {
            sizes.push(found.len());
        }
    }
    sizes
}

async fn moves(world: &World) -> Option<Vec<HubMove>> {
    match tokio::time::timeout(Duration::from_secs(10), world.hub_move(None, name("owner"), HubMoveRequest::Status))
        .await
    {
        Ok(Ok(HubMoveReply::Moves { moves })) => Some(moves),
        _ => None,
    }
}

/// The addresses the computer at `root` reaches its hub by now.
fn addresses(root: &Path) -> Vec<SocketAddr> {
    let dir = WorldDir::under(root);
    std::fs::read_dir(dir.net())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|net| std::fs::read(net.path().join("membership.json")).ok())
        .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .flat_map(|membership| membership["addresses"].as_array().cloned().unwrap_or_default())
        .filter_map(|address| address.as_str().and_then(|text| text.parse().ok()))
        .collect()
}

/// Until the computer at `root` reaches its hub by `to` first.
async fn follows(root: &Path, to: SocketAddr, within: Duration) -> bool {
    let began = Instant::now();
    while began.elapsed() < within {
        if addresses(root).first() == Some(&to) {
            return true;
        }
        pause().await;
    }
    false
}

/// Whether the computer at `root` is online with its hub now, as it says.
async fn online_now(root: &Path) -> bool {
    ask(root, json!({"call": "nodes"})).await.is_ok_and(|said| said.to_string().contains("online"))
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) async fn run(base: &Path) -> Result<()> {
    let began = Instant::now();
    let listen = free()?;
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
    // Records beyond the hub's own, as many as asked, which the move carries
    // as it carries anything of the hub's directory.
    let mib: usize = std::env::var("FASTCTX_DRILL_HUB_MOVE_MB").ok().and_then(|text| text.parse().ok()).unwrap_or(64);
    let records: Vec<u8> = (0..mib << 20).map(|at| ((at % 251) ^ (at >> 20)) as u8).collect();
    std::fs::create_dir_all(base.join("hub/bulk"))?;
    std::fs::write(base.join("hub/bulk/records"), &records)?;
    let mut old_hub = serve(base)?;
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(listen).await.is_ok() {
            break;
        }
        pause().await;
    }
    let (_owner_device, wo) = device(base, "owner-phone", "owner", listen).await?;
    wo.account(
        None,
        name("owner"),
        AccountChange::OpenAccount { name: name("alice"), password: PASS.into(), visitor: true },
    )
    .await?;
    let (owner_pc, mut owner_pc_node) = node(base, "owner-pc", "owner", listen).await?;
    let (target, mut target_node) = node(base, "target", "owner", listen).await?;
    let (alice_pc, mut alice_node) = node(base, "alice-pc", "alice", listen).await?;
    // The owner libraries the machines' records made, one with an object of
    // some size: they stay where the hub was until it serves no more.
    let mut libraries: Vec<String> = Vec::new();
    for _ in 0..120 {
        libraries = ["account", "body"]
            .iter()
            .flat_map(|kind| {
                std::fs::read_dir(base.join("hub/assets").join(kind))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .map(move |entry| format!("assets/{kind}/{}", entry.file_name().to_string_lossy()))
            })
            .collect();
        if !libraries.is_empty() {
            break;
        }
        pause().await;
    }
    libraries.sort();
    record("the machines' records made owner libraries on the hub", !libraries.is_empty(), json!(libraries));
    // A record of the owner's too large to be kept in its item: the hub
    // keeps it as a large object of the owner's library, which follows the
    // library once the hub has moved.
    let much: String = (0..400_000usize).map(|at| char::from(b'a' + ((at * 7 + at / 97) % 26) as u8)).collect();
    let written = wo
        .skill_files(
            None,
            None,
            Request::Write {
                name: "ballast".into(),
                expected: None,
                files: vec![
                    File::text("SKILL.md", "---\nname: ballast\ndescription: A skill of some size.\n---\nBallast.\n"),
                    File::text("references/much.txt", &much),
                ],
            },
        )
        .await;
    record(
        "the owner wrote a record of some size",
        matches!(written, Ok(Reply::Written { .. })),
        json!(written.err().map(|error| error.to_string())),
    );
    // The owner's libraries under a hub's directory, by their paths there.
    let owners = |under: &Path| -> Vec<String> {
        std::fs::read_dir(under.join("assets/account"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("owner."))
            .map(|name| format!("assets/account/{name}"))
            .collect()
    };
    let held = |under: &Path| -> Vec<u64> {
        let mut sizes: Vec<u64> = owners(under).iter().flat_map(|library| large(&under.join(library))).collect();
        sizes.sort_unstable();
        sizes
    };
    let mut before = Vec::new();
    for _ in 0..180 {
        before = held(&base.join("hub"));
        if !before.is_empty() {
            break;
        }
        pause().await;
    }
    let owners = owners(&base.join("hub"));
    record(
        "the hub keeps it as a large object of the owner's library",
        !before.is_empty(),
        json!({"owners": owners, "bytes": before}),
    );
    // The new place's directory on a disk of its own, as a server's data
    // disk is put there before a move (`FASTCTX_DRILL_HUB_MOVE_BIND`): an
    // empty directory of it bound where the hub's directory goes.
    let disk = base.join("data-disk/fastctx-hub");
    let bound = std::env::var_os("FASTCTX_DRILL_HUB_MOVE_BIND").is_some();
    if bound {
        std::fs::create_dir_all(&disk)?;
        std::fs::create_dir_all(target.join("hub"))?;
        let status = Command::new("mount").arg("--bind").arg(&disk).arg(target.join("hub")).status()?;
        anyhow::ensure!(status.success(), "binding the data disk's directory failed");
    }
    // Asked as the interface asks it: through a computer's own node.
    let wpc =
        World::new(WorldDir::under(&owner_pc), fastctx_types::ipc::Peer::Command { resident: None, session: None });
    // Away all through the move.
    stop(&mut alice_node);
    let targets = wpc.hub_move(None, name("owner"), HubMoveRequest::Targets).await?;
    let HubMoveReply::Targets { machines, .. } = &targets else { anyhow::bail!("targets answered {targets:?}") };
    record(
        "the super administrator's machines are offered for the hub",
        machines.iter().any(|one| one.name == machine("target")),
        json!(machines.iter().map(|one| (one.name.to_string(), one.unavailable.clone())).collect::<Vec<_>>()),
    );
    // Asked until the target can take it: its node is to catch up first.
    let to = free()?;
    let asked = Instant::now();
    loop {
        match wpc
            .hub_move(None, name("owner"), HubMoveRequest::Start { to: machine("target"), addresses: vec![to] })
            .await
        {
            Ok(_) => break,
            Err(error) if asked.elapsed() < Duration::from_secs(90) => {
                println!("starting the move: {error}; asked again");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => anyhow::bail!("the move was not started: {error}"),
        }
    }
    let started = Instant::now();
    // Prepared there: its directory made, its service waited for.
    let mut dir = None;
    while started.elapsed() < Duration::from_secs(180) {
        if let Some(Waiting::Service { dir: there, .. }) =
            moves(&wpc).await.and_then(|moves| moves.first().and_then(|one| one.waiting.clone()))
        {
            dir = Some(PathBuf::from(there));
            break;
        }
        pause().await;
    }
    let dir = dir.context("the new place never waited for its service")?;
    let (hub, id) = fastctx_hub::arriving(&dir)?.context("nothing is arriving where the move said")?;
    record(
        "the new place was prepared, waiting for its service",
        true,
        json!({"dir": dir, "move": id, "after_s": started.elapsed().as_secs_f32()}),
    );
    let mut new_hub = serve_at(base, &dir, "new-hub.log")?;
    ask(&target, json!({"call": "hub_arrived", "hub": hub})).await.context("telling the target its hub runs")?;
    // Watched step by step; the copy cut once by the new place stopping.
    let (mut cut, mut last_copying, mut switched_seen) = (false, None::<Instant>, None::<Instant>);
    let mut seen: Vec<String> = Vec::new();
    let mut moved: Option<HubMove> = None;
    while started.elapsed() < Duration::from_secs(40 * 60) {
        let Some(now) = moves(&wpc).await.and_then(|moves| moves.into_iter().next()) else {
            pause().await;
            continue;
        };
        let said = format!("{:?}{}", now.stage, now.error.as_ref().map(|why| format!(" ({why})")).unwrap_or_default());
        if seen.last() != Some(&said) {
            println!("move at {:.1} s: {said}", started.elapsed().as_secs_f32());
            seen.push(said);
        }
        match now.stage {
            MoveStage::Copying => {
                last_copying = Some(Instant::now());
                // Cut once while it copies: the new place stopped, then
                // started again. One done before it could be cut is said so.
                if !cut {
                    cut = true;
                    stop(&mut new_hub);
                    let still = moves(&wpc).await.and_then(|moves| moves.into_iter().next()).map(|now| now.stage);
                    if still.is_some_and(|stage| stage < MoveStage::Switching) {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        record(
                            "the old hub serves on while the copy is cut",
                            online_now(&owner_pc).await,
                            json!({"stage": still}),
                        );
                        record(
                            "the move did not switch while the new place was away",
                            moves(&wpc)
                                .await
                                .and_then(|moves| moves.into_iter().next())
                                .is_some_and(|now| now.stage < MoveStage::Switching),
                            json!({}),
                        );
                    } else {
                        println!("the copy was done before it could be cut ({still:?})");
                    }
                    new_hub = serve_at(base, &dir, "new-hub.log")?;
                }
            }
            MoveStage::Following | MoveStage::Done => {
                switched_seen = Some(Instant::now());
                moved = Some(now);
                break;
            }
            MoveStage::Cancelled => anyhow::bail!("the move was called off: {now:?}"),
            _ => {}
        }
        pause().await;
    }
    let moved = moved.context("the move never switched")?;
    let frozen = match (last_copying, switched_seen) {
        (Some(copying), Some(switched)) => switched.duration_since(copying).as_secs_f32(),
        _ => -1.0,
    };
    record(
        "the hub switched to the new place",
        moved.switched.is_some(),
        json!({"stages": seen, "copy_cut": cut, "total_s": started.elapsed().as_secs_f32(), "from_copying_to_following_s": frozen}),
    );
    record(
        "its records arrived whole",
        std::fs::read(dir.join("bulk/records")).is_ok_and(|there| there == records),
        json!({"mib": mib}),
    );
    if bound {
        record(
            "they are on the data disk bound there",
            std::fs::read(disk.join("bulk/records")).is_ok_and(|there| there == records)
                && disk.join("hub.json").is_file(),
            json!({"disk": disk}),
        );
    }
    record(
        "the old place forwards, the machine away not followed yet",
        moved.following.as_ref().is_some_and(|following| {
            matches!(following.old, OldPlace::Forwarding)
                && following.machines.iter().any(|one| *one == machine("alice-pc"))
        }),
        json!(moved.following),
    );
    for (root, label) in [(&owner_pc, "owner-pc"), (&target, "target")] {
        record(
            &format!("{label} follows the new address"),
            follows(root, to, Duration::from_secs(120)).await,
            json!(addresses(root)),
        );
        record(&format!("{label} is online through the new hub"), online_now(root).await, json!({}));
    }
    // A change of the accounts, from a phone, taken by the new hub.
    let mut changed = Err(String::from("not asked"));
    for _ in 0..30 {
        changed = wo
            .account(
                None,
                name("owner"),
                AccountChange::OpenAccount { name: name("carol"), password: PASS.into(), visitor: true },
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        if changed.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    record("the new hub takes an account's change from a phone", changed.is_ok(), json!({"error": changed.err()}));
    // The owner libraries follow, the old hub serving no more: each opened
    // as it came, none taken for one put back.
    let coming = dir.join("libraries-coming.json");
    let waited = Instant::now();
    while coming.exists() && waited.elapsed() < Duration::from_secs(600) {
        pause().await;
    }
    record(
        "the owner libraries came once the hub had switched",
        !coming.exists(),
        json!({"libraries": libraries, "after_s": waited.elapsed().as_secs_f32(), "left": std::fs::read_to_string(&coming).ok()}),
    );
    let databases = |library: &String| {
        std::fs::read_dir(dir.join(library))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".sqlite3"))
            .count()
    };
    record(
        "each owner library's databases are at the new place",
        libraries.iter().all(|library| databases(library) > 0),
        json!(libraries.iter().map(|library| (library, databases(library))).collect::<Vec<_>>()),
    );
    // Each was checked against what the old place sent before it was opened:
    // so the old place's log says.
    let sent = std::fs::read_to_string(base.join("hub.log")).unwrap_or_default();
    record(
        "the old place sent every owner library and the new hub took each",
        sent.contains("every owner library is with the new hub")
            && libraries
                .iter()
                .all(|library| sent.contains(&format!("the owner library {library} is with the new hub"))),
        json!(
            sent.lines()
                .filter(|line| line.contains("owner librar"))
                .map(|line| line.chars().take(160).collect::<String>())
                .collect::<Vec<_>>()
        ),
    );
    // Nothing of them went before the switch; each that something refers to
    // is put into its library once that is open at the new place.
    let after = held(&dir);
    record(
        "the large objects followed their libraries",
        !before.is_empty()
            && after == before
            && owners.iter().all(|library| {
                sent.lines().any(|line| {
                    line.contains(&format!("large objects of {library} are with the new hub"))
                        && !line.contains(": 0 sent now")
                })
            }),
        json!({
            "before": before,
            "after": after,
            "said": sent.lines().filter(|line| line.contains("large objects")).map(|line| line.chars().take(200).collect::<String>()).collect::<Vec<_>>(),
        }),
    );
    // With the hub stopped, the databases go as what of them changed since
    // the copy sent while it served.
    let stopped: Option<(u64, u64)> = sent.lines().find_map(|line| {
        let said = line.split("with the hub stopped, ").nth(1)?;
        let mut numbers = said.split(' ').filter_map(|word| word.parse::<u64>().ok());
        let (bytes, _files, whole) = (numbers.next()?, numbers.next()?, numbers.next()?);
        Some((bytes, whole))
    });
    record(
        "with the hub stopped, less went than the changed files are long",
        stopped.is_some_and(|(bytes, whole)| bytes < whole),
        json!({"sent": stopped.map(|(bytes, _)| bytes), "of": stopped.map(|(_, whole)| whole)}),
    );
    let said = std::fs::read_to_string(base.join("new-hub.log")).unwrap_or_default();
    record(
        "the new hub sealed no asset",
        !said.contains("sealed") && !said.contains("older than the hub's own record"),
        json!(
            said.lines()
                .filter(|line| line.contains("sealed") || line.contains("older than"))
                .take(3)
                .collect::<Vec<_>>()
        ),
    );
    // How long the old hub took nothing, as its log says: from stopping to
    // forwarding.
    let log = std::fs::read_to_string(base.join("hub.log")).unwrap_or_default();
    let at = |said: &str| {
        log.lines()
            .find(|line| line.contains(said))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|stamp| stamp.parse::<jiff::Timestamp>().ok())
    };
    let froze = match (at("stops taking anything and hands over"), at("took over; this place forwards")) {
        (Some(from), Some(to)) => (to.as_millisecond() - from.as_millisecond()) as f64 / 1000.0,
        _ => -1.0,
    };
    record("the hub took nothing only while it handed over", froze >= 0.0, json!({"frozen_s": froze}));
    // The machine away comes back: through the old place, to the new one.
    alice_node = start(&alice_pc)?;
    record("the machine away comes online again", online(&alice_pc).await, json!({}));
    record(
        "the machine away follows the new address",
        follows(&alice_pc, to, Duration::from_secs(180)).await,
        json!(addresses(&alice_pc)),
    );
    let mut done = None;
    let waiting = Instant::now();
    while waiting.elapsed() < Duration::from_secs(180) {
        if let Some(now) = moves(&wpc).await.and_then(|moves| moves.into_iter().next())
            && now.following.as_ref().is_some_and(|following| {
                following.machines.is_empty() && !matches!(following.old, OldPlace::Forwarding)
            })
        {
            done = Some(now);
            break;
        }
        pause().await;
    }
    record(
        "every machine followed and the old place stopped forwarding",
        done.is_some(),
        json!(done.map(|now| (now.stage, now.following))),
    );
    // Gone, the old hub is missed by none.
    stop(&mut old_hub);
    tokio::time::sleep(Duration::from_secs(5)).await;
    for (root, label) in [(&owner_pc, "owner-pc"), (&alice_pc, "alice-pc")] {
        record(&format!("{label} is online with the old hub gone"), online_now(root).await, json!({}));
    }
    println!("HUB MOVE: {:.1} s in all", began.elapsed().as_secs_f32());
    for child in [&mut new_hub, &mut owner_pc_node, &mut target_node, &mut alice_node] {
        stop(child);
    }
    let failed = super::FAILED.load(std::sync::atomic::Ordering::Relaxed);
    anyhow::ensure!(failed == 0, "{failed} checks failed");
    Ok(())
}
