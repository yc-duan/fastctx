//! Process counters include cached reads; physical disk bytes are reported separately.
use super::*;

pub(super) fn snapshot(pid: u32) -> Result<Value> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat.rsplit_once(") ").context("process stat")?.1.split_whitespace().collect();
    let number = |at: usize| fields[at].parse::<u64>().map_err(anyhow::Error::from);
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let memory = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key)?.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let io = std::fs::read_to_string(format!("/proc/{pid}/io"))?;
    let io_number =
        |key: &str| io.lines().find_map(|line| line.strip_prefix(key)?.trim().parse::<u64>().ok()).unwrap_or(0);
    static TICKS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ticks = match TICKS.get() {
        Some(ticks) => *ticks,
        None => {
            let output = Command::new("getconf").arg("CLK_TCK").output()?;
            anyhow::ensure!(output.status.success(), "getconf CLK_TCK failed");
            let ticks = String::from_utf8(output.stdout)?.trim().parse::<u64>()?;
            let _ = TICKS.set(ticks);
            ticks
        }
    };
    Ok(json!({"pid":pid,"start_ticks":number(19)?,"cpu_seconds":(number(11)?+number(12)?) as f64/ticks as f64,
        "children_cpu_seconds":(number(13)?+number(14)?) as f64/ticks as f64,
        "virtual_bytes":number(20)?,"rss_bytes":memory("VmRSS:"),"peak_rss_bytes":memory("VmHWM:"),
        "read_chars":io_number("rchar:"),"disk_read_bytes":io_number("read_bytes:"),
        "write_chars":io_number("wchar:"),"disk_write_bytes":io_number("write_bytes:")}))
}

pub(super) fn difference(before: &Value, after: &Value) -> Value {
    if before["pid"].as_u64().is_none() || after["pid"].as_u64().is_none() {
        return json!({"available":false,"before_error":before["error"],"after_error":after["error"]});
    }
    let mut result = json!({"same_process": before["pid"] == after["pid"] && before["start_ticks"] == after["start_ticks"],
        "rss_bytes":after["rss_bytes"],"peak_rss_bytes":after["peak_rss_bytes"]});
    for key in
        ["cpu_seconds", "children_cpu_seconds", "read_chars", "disk_read_bytes", "write_chars", "disk_write_bytes"]
    {
        result[key] = json!(after[key].as_f64().unwrap_or(0.0) - before[key].as_f64().unwrap_or(0.0));
    }
    result
}
