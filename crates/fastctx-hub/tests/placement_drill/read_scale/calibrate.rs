//! Parser calibration is separate from timed user actions.
use super::*;

pub(crate) fn run(fixture: &Path, all: bool) -> Result<()> {
    let manifest: Value = serde_json::from_slice(&std::fs::read(fixture.join("manifest.json"))?)?;
    anyhow::ensure!(manifest["format"] == 1, "unsupported scale fixture manifest");
    let mut checked = Vec::new();
    for entry in manifest["files"].as_array().context("files")? {
        let path = entry["path"].as_str().context("path")?;
        if entry.get("expected_events").is_none() {
            continue;
        }
        if !all
            && path != "claude/scale-0000.jsonl"
            && !path.contains("agent-child-0000-0.jsonl")
            && path != "claude/long-line.jsonl"
        {
            continue;
        }
        let began = Instant::now();
        let read = match entry["agent"].as_str() {
            Some(agent) => fastctx_hosts::claude::transcript::read_agent(&fixture.join(path), agent)?,
            None => fastctx_hosts::claude::transcript::read(&fixture.join(path))?,
        };
        anyhow::ensure!(read.problems.is_empty(), "{path}: {:?}", read.problems);
        anyhow::ensure!(
            read.events.len() as u64 == entry["expected_events"].as_u64().context("expected events")?,
            "{path}: events {}, expected {}",
            read.events.len(),
            entry["expected_events"]
        );
        anyhow::ensure!(
            read.uuids.len() as u64 == entry["uuids"].as_u64().context("expected UUIDs")?,
            "{path}: UUID count mismatch"
        );
        checked.push(json!({"path":path,"events":read.events.len(),"uuids":read.uuids.len(),"ms":began.elapsed().as_secs_f64()*1000.0}));
    }
    println!("{}", serde_json::to_string_pretty(&json!({"calibration":checked}))?);
    Ok(())
}
