//! Repeatable reads through real nodes and the same client/window entry points as the GUI.
use super::*;
use fastctx_types::attachments::{AttachmentSource, HostImage};
use fastctx_types::{Address, ResidentId};
use std::time::Instant;
pub(super) mod calibrate;
mod page;
pub(super) mod page_worker;
mod resources;
mod sampler;
mod seed;
mod tree;

struct Processes(Vec<Child>);
impl Drop for Processes {
    fn drop(&mut self) {
        for child in self.0.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) async fn run(base: &Path) -> Result<()> {
    anyhow::ensure!(cfg!(target_os = "linux"), "read-scale uses Linux process counters (WSL is supported)");
    let config: Value = serde_json::from_slice(&std::fs::read(base.join("scale-config.json"))?)?;
    let fixture = PathBuf::from(config["fixture"].as_str().context("fixture")?);
    let evidence = PathBuf::from(config["evidence"].as_str().context("evidence")?);
    let manifest: Value = serde_json::from_slice(&std::fs::read(fixture.join("manifest.json"))?)?;
    anyhow::ensure!(manifest["format"] == 1, "unsupported scale fixture manifest");
    anyhow::ensure!(!base.join("host").exists() && !base.join("hub").exists(), "use a fresh run directory");
    std::fs::create_dir_all(base.join("user"))?;
    let (seeding_base, seeding_fixture, seeding_manifest) = (base.to_path_buf(), fixture.clone(), manifest.clone());
    tokio::task::spawn_blocking(move || seed::prepare(&seeding_base, &seeding_fixture, &seeding_manifest)).await??;
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
    let _sampler = sampler::Sampler::start(&evidence)?;
    let mut processes = Processes(vec![serve(base)?]);
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(listen).await.is_ok() {
            break;
        }
        pause().await;
    }
    let (root, child) = node(base, "host", "owner", listen).await?;
    let mut pid = child.id();
    processes.0.push(child);
    let mut local = World::new(WorldDir::under(&root), Peer::Command { resident: None, session: None });
    let (device, remote) = device(base, "reader", "owner", listen).await?;
    let resident: ResidentId = "claude-00000000".parse()?;
    let source = AttachmentSource::HostImage {
        owner: Address { machine: machine("host"), resident: resident.clone() },
        host_image: HostImage::Claude { message: "scale-0000:picture".into(), image: 0 },
    };
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut result = json!({"format":1,"fixture":config["source_fixture"],"runtime_fixture":fixture,"profile":manifest["profile"],"history":manifest["history"],
    "dataset_bytes":manifest["total_bytes"],"source_bytes":manifest["files"][1]["bytes"],
    "started_at":jiff::Timestamp::now(),"node_pid":pid,"optimized":!cfg!(debug_assertions),"samples":[],"limitations":[
        "Linux/WSL, loopback hub; not WAN or native Windows timing",
        "No model or host process; sleeping Claude offers only",
        "First-screen ends at serialized GUI window, excludes WebView paint",
        "Process rchar includes cached file reads and IPC; doctor byte accounting is incomplete",
        "Synthetic connected workbench has no Claude executable; refresh drift is expected",
        "New-device sync and Codex app-server fixture are not implemented"
    ]});
    checkpoint(&evidence, &result)?;
    let repeats = config["repeats"].as_u64().unwrap_or(1);
    let stop_after_first_screen = config["stop_after_first_screen"].as_bool().unwrap_or(false);
    let page_timeout = Duration::from_secs(config["page_timeout_seconds"].as_u64().unwrap_or(600));
    for index in 0..repeats {
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(&evidence, &root, pid, &mut result, "image_original", index, async {
            use fastctx_client::file_read::Read;
            use sha2::{Digest, Sha256};
            let began = Instant::now();
            let mut reading = local.read_attachment(source.clone());
            let mut digest = Sha256::new();
            let mut bytes = 0u64;
            let mut locate_ms = None;
            while let Some(part) = reading.next().await? {
                match part {
                    Read::Opened(_) => {
                        locate_ms = Some(began.elapsed().as_secs_f64() * 1000.0);
                    }
                    Read::Bytes(part) => {
                        bytes += part.len() as u64;
                        digest.update(&part);
                    }
                }
            }
            let sha256 = format!("{:x}", digest.finalize());
            anyhow::ensure!(sha256 == manifest["picture"]["sha256"].as_str().unwrap_or(""), "original hash mismatch");
            Ok(json!({"locate_ms":locate_ms,"bytes":bytes,"sha256":sha256}))
        })
        .await?;
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(&evidence, &root, pid, &mut result, "image_path_thumbnail", index, async {
            Ok(serde_json::to_value(
                local
                    .thumbnail(
                        None,
                        fastctx_types::thumbnail::ThumbnailRequest {
                            path: fixture.join("assets/picture.png").to_string_lossy().into_owned(),
                            max: 256,
                        },
                    )
                    .await?,
            )?)
        })
        .await?;
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(&evidence, &root, pid, &mut result, "image_thumbnail", index, async {
            let image = local.thumbnail_attachment(source.clone(), 256).await?;
            anyhow::ensure!(
                image
                    .content
                    .as_ref()
                    .is_some_and(|f| f.sha256 == manifest["picture"]["sha256"].as_str().unwrap_or("")),
                "image content hash mismatch"
            );
            Ok(serde_json::to_value(image)?)
        })
        .await?;
        let image_reader =
            remote.clone().keeping(fastctx_client::cache::Cache::open(base.join(format!("picture-cache-{index}")))?);
        for operation in ["image_device_cold_thumbnail", "image_device_cached_thumbnail"] {
            restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
            measured(&evidence, &root, pid, &mut result, operation, index, async {
                let image = image_reader.thumbnail_attachment(source.clone(), 256).await?;
                anyhow::ensure!(
                    image
                        .content
                        .as_ref()
                        .is_some_and(|content| content.sha256 == manifest["picture"]["sha256"].as_str().unwrap_or("")),
                    "device thumbnail hash mismatch"
                );
                Ok(serde_json::to_value(image)?)
            })
            .await?;
        }
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(&evidence, &root, pid, &mut result, "subagent", index, async {
            let activity =
                local.agent_activity(resident.clone(), machine("host").into(), "child-0000-0".into()).await?;
            anyhow::ensure!(
                activity.unavailable.is_none() && !activity.events.is_empty(),
                "subagent missing or unavailable: {:?}",
                activity.unavailable
            );
            let entry = manifest["files"]
                .as_array()
                .context("files")?
                .iter()
                .find(|entry| entry["agent"] == "child-0000-0")
                .context("child fixture")?;
            anyhow::ensure!(
                activity.events.len() as u64 == entry["records"].as_u64().context("child rows")?,
                "subagent event count mismatch"
            );
            Ok(serde_json::to_value(activity)?)
        })
        .await?;
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(&evidence, &root, pid, &mut result, "who", index, async {
            let value = serde_json::to_value(local.who(None, true).await?)?;
            anyhow::ensure!(value.to_string().contains("claude-00000000"), "who omitted fixture resident");
            Ok(value)
        })
        .await?;
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(&evidence, &root, pid, &mut result, "offers", index, async {
            let offers = local.offers(resident.clone(), Some(machine("host").into())).await?;
            anyhow::ensure!(!offers.1.models.is_empty(), "offers omitted Claude models");
            Ok(serde_json::to_value(offers)?)
        })
        .await?;
    }
    restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
    // No watchers or requests during the quiet interval, including doctor requests.
    tokio::time::sleep(Duration::from_secs(5)).await;
    result["active_operation"] = json!("quiet");
    checkpoint(&evidence, &result)?;
    let quiet_seconds = config["quiet_seconds"].as_u64().unwrap_or(600);
    let before_readings = readings(&root).await?;
    let before = resources::snapshot(pid)?;
    let began = Instant::now();
    let mut samples = Vec::new();
    while began.elapsed() < Duration::from_secs(quiet_seconds) {
        tokio::time::sleep(
            Duration::from_secs(5).min(Duration::from_secs(quiet_seconds).saturating_sub(began.elapsed())),
        )
        .await;
        let sample = resources::snapshot(pid).unwrap_or_else(|error| json!({"error":error.to_string()}));
        samples.push(sample.clone());
        result["quiet_progress"] = json!({"seconds":began.elapsed().as_secs_f64(),"samples":samples});
        checkpoint(&evidence, &result)?;
        anyhow::ensure!(sample.get("error").is_none(), "node ended during quiet sampling: {sample}");
    }
    let after = resources::snapshot(pid)?;
    let elapsed = began.elapsed().as_secs_f64();
    let after_readings = readings(&root).await?;
    result["quiet"] = json!({"seconds":elapsed,"node":resources::difference(&before,&after),"samples":samples,
        "readings":readings_delta(&before_readings,&after_readings)});
    let expected = manifest["conversations"][0]["expected_events"].as_u64().context("expected events")?;
    let newest = manifest["newest_row"].as_u64().context("newest row")?;
    for index in 0..repeats {
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        page_read(
            base,
            &evidence,
            &root,
            pid,
            &mut result,
            listen,
            &resident,
            expected,
            newest,
            !stop_after_first_screen,
            page_timeout,
            "first_screen_local",
            index,
        )
        .await?;
        if stop_after_first_screen {
            return finish(&evidence, &mut result);
        }
    }
    let mut device_cache_ready = false;
    for (index, phase) in ["first_screen_device_empty_cache", "first_screen_device_cached"].into_iter().enumerate() {
        if phase.ends_with("cached") && !device_cache_ready {
            push(
                &evidence,
                &mut result,
                json!({"operation":phase,"result":{"ok":false,"error":"the earlier device read did not populate a complete cache"}}),
            )?;
            continue;
        }
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        page_read(
            base,
            &evidence,
            &root,
            pid,
            &mut result,
            listen,
            &resident,
            expected,
            newest,
            true,
            page_timeout,
            phase,
            index as u64,
        )
        .await?;
        device_cache_ready = result["samples"]
            .as_array()
            .and_then(|samples| samples.last())
            .is_some_and(|sample| sample["result"]["ok"] == true);
    }
    for index in 0..repeats {
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(
            &evidence,
            &root,
            pid,
            &mut result,
            "collaboration_tree",
            index,
            tree::query(&local, resident.clone()),
        )
        .await?;
        restore_node(base, &evidence, &root, &mut processes, &mut pid, &mut local, &mut result).await?;
        measured(
            &evidence,
            &root,
            pid,
            &mut result,
            "collaboration_watch",
            index,
            tree::watch(&local, resident.clone()),
        )
        .await?;
    }
    drop(remote);
    drop(device);
    finish(&evidence, &mut result)
}

fn finish(evidence: &Path, result: &mut Value) -> Result<()> {
    result["finished_at"] = json!(jiff::Timestamp::now());
    result.as_object_mut().context("result")?.remove("quiet_progress");
    result.as_object_mut().context("result")?.remove("active_operation");
    checkpoint(evidence, result)?;
    println!("READ SCALE complete: {}", evidence.join("scale-result.json").display());
    let failed =
        result["samples"].as_array().context("samples")?.iter().filter(|sample| sample["result"]["ok"] != true).count();
    anyhow::ensure!(failed == 0, "{failed} scale operations failed; their measurements remain in scale-result.json");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn restore_node(
    base: &Path,
    evidence: &Path,
    root: &Path,
    processes: &mut Processes,
    pid: &mut u32,
    local: &mut World,
    result: &mut Value,
) -> Result<()> {
    let child = processes.0.last_mut().context("node process")?;
    let ended = child.try_wait()?;
    let failed = result["samples"]
        .as_array()
        .and_then(|samples| samples.last())
        .is_some_and(|sample| sample["result"]["ok"] != true);
    if ended.is_none() && !failed {
        return Ok(());
    }
    let _ = child.kill();
    let _ = child.wait();
    let before = *pid;
    *child = start_with(root, &base.join("host/bin/fastctx"))?;
    *pid = child.id();
    anyhow::ensure!(online(root).await, "node did not restart after a failed scale probe");
    *local = World::new(WorldDir::under(root), Peer::Command { resident: None, session: None });
    tokio::time::sleep(Duration::from_secs(2)).await;
    if result.get("restarts").is_none() {
        result["restarts"] = json!([]);
    }
    result["restarts"].as_array_mut().context("restarts")?.push(json!({"before":before,"after":*pid,"previous_probe_failed":failed,"previous_exit":ended.map(|status|status.to_string())}));
    checkpoint(evidence, result)
}

#[allow(clippy::too_many_arguments)]
async fn page_read(
    runtime: &Path,
    base: &Path,
    root: &Path,
    pid: u32,
    result: &mut Value,
    listen: SocketAddr,
    resident: &ResidentId,
    expected: u64,
    newest: u64,
    pages: bool,
    deadline: Duration,
    operation: &str,
    index: u64,
) -> Result<()> {
    result["active_operation"] = json!({"name":operation,"sample":index});
    checkpoint(base, result)?;
    let before_readings = readings(root).await?;
    let before = resources::snapshot(pid)?;
    let progress = base.join(format!("{operation}-{index}.json"));
    let request = base.join(format!("{operation}-{index}-request.json"));
    std::fs::write(
        &request,
        serde_json::to_vec(&json!({"progress":progress,"operation":operation,
        "sample":index,"resident":resident,"expected":expected,"newest":newest,"node_pid":pid,"listen":listen.to_string(),"pages":pages}))?,
    )?;
    let output = std::fs::OpenOptions::new().create(true).append(true).open(base.join("page-workers.log"))?;
    let mut children = Processes(vec![
        Command::new(std::env::current_exe()?)
            .arg(runtime)
            .arg(KERNEL.get().context("kernel")?)
            .arg("--scale-page")
            .arg(&request)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output)
            .spawn()?,
    ]);
    let child = children.0.first_mut().context("page child")?;
    let worker_pid = child.id();
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if began.elapsed() >= deadline {
            child.kill()?;
            child.wait()?;
            break None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let mut measured = std::fs::read(&progress)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(json!({}));
    if !status.is_some_and(|status| status.success()) || measured.get("ok").is_none() {
        measured["ok"] = json!(false);
        measured["error"] = json!(status.map_or_else(
            || format!("{} second page deadline", deadline.as_secs()),
            |status| format!("page process ended: {status}")
        ));
        measured["elapsed_ms"] = json!(began.elapsed().as_secs_f64() * 1000.0);
    }
    measured["page_process"] = json!({"pid":worker_pid,"reaped":true,"deadline_seconds":deadline.as_secs(),
        "exit":status.map(|status|status.to_string()),"timed_out":status.is_none()});

    let after = resources::snapshot(pid).unwrap_or_else(|error| json!({"error":error.to_string()}));
    let after_readings = readings(root).await.unwrap_or_else(|error| json!({"error":error.to_string()}));
    push(
        base,
        result,
        json!({"operation":operation,"sample":index,"client":measured["client"],"result":measured,
        "node":resources::difference(&before,&after),"readings":readings_delta(&before_readings,&after_readings)}),
    )
}

async fn measured<F: std::future::Future<Output = Result<Value>>>(
    base: &Path,
    root: &Path,
    pid: u32,
    result: &mut Value,
    operation: &str,
    index: u64,
    work: F,
) -> Result<()> {
    result["active_operation"] = json!({"name":operation,"sample":index});
    checkpoint(base, result)?;
    let client_before = resources::snapshot(std::process::id())?;
    let before_readings = readings(root).await?;
    let before = resources::snapshot(pid)?;
    let began = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(180), work).await;
    let milliseconds = began.elapsed().as_secs_f64() * 1000.0;
    let after = resources::snapshot(pid).unwrap_or_else(|error| json!({"error":error.to_string()}));
    let after_readings = readings(root).await.unwrap_or_else(|error| json!({"error":error.to_string()}));
    let response = match response {
        Ok(Ok(value)) => json!({"ok":true,"json_bytes":json_bytes(&value)?,"shape":shape(&value)}),
        Ok(Err(error)) => json!({"ok":false,"error":format!("{error:#}")}),
        Err(_) => json!({"ok":false,"error":"180 second request deadline"}),
    };
    push(
        base,
        result,
        json!({"operation":operation,"sample":index,"ms":milliseconds,"result":response,
        "node":resources::difference(&before,&after),"readings":readings_delta(&before_readings,&after_readings),
        "client":resources::difference(&client_before,&resources::snapshot(std::process::id())?)}),
    )
}

fn shape(value: &Value) -> Value {
    match value {
        Value::Array(items) => json!({"items":items.len()}),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter_map(|(key, value)| match value {
                    Value::Array(v) => Some((key.clone(), json!({"items":v.len()}))),
                    Value::String(v) if v.len() > 500 => None,
                    Value::Object(_) => Some((key.clone(), shape(value))),
                    _ => Some((key.clone(), value.clone())),
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

async fn readings(root: &Path) -> Result<Value> {
    ask(root, json!({"call":"doctor_readings"})).await
}
fn readings_delta(before: &Value, after: &Value) -> Value {
    match (
        serde_json::from_value::<fastctx_types::readings::Readings>(before["readings"].clone()),
        serde_json::from_value::<fastctx_types::readings::Readings>(after["readings"].clone()),
    ) {
        (Ok(before), Ok(after)) => serde_json::to_value(after.since(&before)).unwrap_or(Value::Null),
        _ => json!({"before":before,"after":after}),
    }
}
fn checkpoint(base: &Path, result: &Value) -> Result<()> {
    std::fs::write(base.join("scale-result.json"), serde_json::to_vec_pretty(result)?)?;
    Ok(())
}
fn push(base: &Path, result: &mut Value, sample: Value) -> Result<()> {
    println!("READ SCALE {} sample {}", sample["operation"], sample["sample"]);
    result["samples"].as_array_mut().context("samples")?.push(sample);
    checkpoint(base, result)
}
fn json_bytes(value: &impl serde::Serialize) -> Result<u64> {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value)?;
    Ok(count.0)
}
