//! A page probe owns its client process so a deadline also ends detached cache work.
use super::*;

pub(crate) async fn run(base: &Path, request: &Path) -> Result<()> {
    let config: Value = serde_json::from_slice(&std::fs::read(request)?)?;
    let progress = PathBuf::from(config["progress"].as_str().context("progress")?);
    let resident: ResidentId = config["resident"].as_str().context("resident")?.parse()?;
    let expected = config["expected"].as_u64().context("expected")?;
    let newest = config["newest"].as_u64().context("newest")?;
    let node_pid = config["node_pid"].as_u64().context("node pid")? as u32;
    let operation = config["operation"].as_str().context("operation")?;
    let (device, world) = if operation == "first_screen_local" {
        let cache = fastctx_client::cache::Cache::open(
            base.join(format!("local-cache-{}", config["sample"].as_u64().context("sample")?)),
        )?;
        (
            None,
            World::new(WorldDir::under(&base.join("host")), Peer::Command { resident: None, session: None })
                .keeping(cache),
        )
    } else {
        let listen = config["listen"].as_str().context("listen")?.parse()?;
        let (device, world) = device(base, "page-reader", "owner", listen).await?;
        (Some(device), world)
    };
    let before = resources::snapshot(std::process::id())?;
    let began = Instant::now();
    let measured = page::measure(
        &world,
        resident,
        expected,
        newest,
        &progress,
        node_pid,
        config["pages"].as_bool().context("pages")?,
    )
    .await;
    let mut result = match measured {
        Ok(result) => result,
        Err(error) => {
            let mut result = std::fs::read(&progress)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .unwrap_or(json!({}));
            result["ok"] = json!(false);
            result["error"] = json!(format!("{error:#}"));
            result
        }
    };
    result["elapsed_ms"] = json!(began.elapsed().as_secs_f64() * 1000.0);
    result["client"] = resources::difference(&before, &resources::snapshot(std::process::id())?);
    std::fs::write(&progress, serde_json::to_vec_pretty(&result)?)?;
    drop(world);
    drop(device);
    Ok(())
}
