//! Local operational diagnostics of the serving node.

#[derive(clap::Subcommand)]
pub enum Doctor {
    /// Export one stopped node's conversation before moving its directory.
    ExportConversation {
        /// The conversation directory's name.
        conversation: String,
        /// The source Fastctx root, containing world/.
        #[arg(long)]
        dir: std::path::PathBuf,
    },
    /// Let a conversation go from a stopped node once another machine
    /// imported its export, setting its directory aside.
    ReleaseConversation {
        /// The conversation directory's name.
        conversation: String,
        /// The fingerprint the other machine lists the import under.
        #[arg(long)]
        fingerprint: String,
        /// The source Fastctx root, containing world/.
        #[arg(long)]
        dir: std::path::PathBuf,
    },
    /// Conversation imports, pending, rejected or done, and conversations
    /// released, with their fingerprints and explanations.
    ConversationImports {
        /// The Fastctx root, containing world/.
        #[arg(long)]
        dir: std::path::PathBuf,
        /// Print structured diagnostics.
        #[arg(long)]
        json: bool,
    },
    /// Calls being answered, waiting for admission, or still finishing.
    Calls {
        /// Print structured diagnostics.
        #[arg(long)]
        json: bool,
    },
    /// The node's health, its measures of itself, the rules it broke, and
    /// the scenes it left when it was held up or stuck.
    Readings {
        /// Print structured diagnostics.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: Doctor) -> anyhow::Result<()> {
    let (call, json) = match command {
        Doctor::ExportConversation { conversation, dir } => {
            let exported = fastctx_node::export_conversation(&dir, &conversation)?;
            println!("{}", exported.bundle.display());
            println!("fingerprint {}", exported.fingerprint);
            return Ok(());
        }
        Doctor::ReleaseConversation { conversation, fingerprint, dir } => {
            let kept = fastctx_node::release_conversation(&dir, &conversation, &fingerprint)?;
            println!("{}", kept.display());
            return Ok(());
        }
        Doctor::ConversationImports { dir, json } => {
            let imports = fastctx_node::conversation_imports(&dir)?;
            if json {
                println!("{}", serde_json::to_string(&imports)?);
            } else if imports.is_empty() {
                println!("No conversation imports.");
            } else {
                for import in imports {
                    println!(
                        "{}: {}{}{}",
                        import.conversation,
                        import.state,
                        import.from.map(|from| format!(" from {from}")).unwrap_or_default(),
                        import.reason.map(|reason| format!(" — {reason}")).unwrap_or_default()
                    );
                    if let Some(fingerprint) = import.fingerprint {
                        println!("  fingerprint {fingerprint}");
                    }
                    if let Some(archive) = import.archive {
                        println!("  {}", archive.display());
                    }
                }
            }
            return Ok(());
        }
        Doctor::Calls { json } => (fastctx_types::Call::DoctorCalls, json),
        Doctor::Readings { json } => (fastctx_types::Call::DoctorReadings, json),
    };
    let world = fastctx_types::layout::WorldDir::under(&fastctx_platform::fastctx_root()?);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let reply = runtime.block_on(async {
        let mut client = fastctx_client::Client::connect(&world, fastctx_client::command_peer()?).await?;
        Ok::<_, anyhow::Error>(client.call(call).await?)
    })?;
    match reply {
        fastctx_types::Reply::DoctorCalls { calls } => calls_out(calls, json),
        fastctx_types::Reply::DoctorReadings { readings, health, scenes } => {
            readings_out(&readings, &health, &scenes, json)
        }
        _ => anyhow::bail!("the node returned no diagnostics"),
    }
}

fn calls_out(calls: Vec<fastctx_types::stats::CallWork>, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(&calls)?);
    } else if calls.is_empty() {
        println!("No calls in progress.");
    } else {
        for call in calls {
            println!(
                "{} {} for {} from {}: {}, {} s, {}{}",
                call.id,
                call.kind,
                call.account,
                call.machine,
                call.phase,
                call.seconds,
                call.waiting,
                call.conversation.map(|id| format!(" ({id})")).unwrap_or_default()
            );
        }
    }
    Ok(())
}

/// What work cost since the node started, the costliest first: the time its
/// tasks held a thread, the time it blocked, and the bytes it read.
fn costs_out(readings: &fastctx_types::readings::Readings) {
    const SHOWN: usize = 20;
    let mut costs: std::collections::BTreeMap<&str, [u64; 3]> = std::collections::BTreeMap::new();
    for count in &readings.counts {
        let Some(named) = count.name.strip_prefix("cost.") else { continue };
        let Some((what, measure)) = named.rsplit_once('.') else { continue };
        let slot = match measure {
            "task_us" => 0,
            "blocking_us" => 1,
            "read_bytes" => 2,
            _ => continue,
        };
        costs.entry(what).or_default()[slot] = count.value;
    }
    if costs.is_empty() {
        return;
    }
    let mut costs: Vec<_> = costs.into_iter().collect();
    costs.sort_by_key(|(_, [task, blocking, _])| std::cmp::Reverse(task + blocking));
    println!("What work cost, the costliest first (on task threads, blocking, read):");
    for (what, [task, blocking, read]) in costs.iter().take(SHOWN) {
        println!(
            "  {what}: {:.1} s, {:.1} s, {:.1} MiB",
            *task as f64 / 1e6,
            *blocking as f64 / 1e6,
            *read as f64 / (1024.0 * 1024.0)
        );
    }
    if costs.len() > SHOWN {
        println!("  and {} more (--json lists them all)", costs.len() - SHOWN);
    }
}

fn readings_out(
    readings: &fastctx_types::readings::Readings,
    health: &fastctx_types::health::Health,
    scenes: &[String],
    json: bool,
) -> anyhow::Result<()> {
    if json {
        let all = serde_json::json!({ "health": health, "readings": readings, "scenes": scenes });
        println!("{}", serde_json::to_string(&all)?);
        return Ok(());
    }
    let mut said = vec![if health.busy { "busy".to_string() } else { "not busy".to_string() }];
    if health.held_up_ms > 0 {
        said.push(format!("work waited up to {} ms for a thread in the last minute", health.held_up_ms));
    }
    if health.stuck > 0 {
        said.push(format!("{} stuck", health.stuck));
    }
    if let Some(near) = &health.near {
        said.push(format!("{} is {}% used", near.bound, near.used));
    }
    println!("Health: {}.", said.join("; "));
    println!("Rules broken since the node started: {}.", readings.violations_total);
    for violation in &readings.violations {
        println!(
            "  {} at {}: {} times, worst {} ms, last {}{}",
            violation.kind,
            violation.place,
            violation.count,
            violation.worst_ms,
            violation.last,
            if violation.doing.is_empty() { String::new() } else { format!(", doing {}", violation.doing) }
        );
    }
    if !readings.timings.is_empty() {
        println!("Timings (median, 99th percentile, longest, in milliseconds):");
        for timing in &readings.timings {
            println!(
                "  {}: {} times, {:.1}, {:.1}, {:.1}",
                timing.name,
                timing.count,
                timing.p50_us as f64 / 1000.0,
                timing.p99_us as f64 / 1000.0,
                timing.max_us as f64 / 1000.0
            );
        }
    }
    costs_out(readings);
    if !readings.counts.is_empty() || !readings.levels.is_empty() {
        println!("Counts and levels:");
        for count in readings.counts.iter().filter(|count| !count.name.starts_with("cost.")) {
            println!("  {}: {}", count.name, count.value);
        }
        for level in &readings.levels {
            println!("  {}: {}", level.name, level.value);
        }
    }
    if scenes.is_empty() {
        println!("No scenes left.");
    } else {
        println!("Scenes left:");
        for scene in scenes {
            println!("  {scene}");
        }
    }
    Ok(())
}
