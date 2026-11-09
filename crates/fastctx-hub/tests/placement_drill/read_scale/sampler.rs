//! Samples every process in the drill's private PID namespace, including short-lived decoders.
use super::*;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Sampler {
    pub fn start(base: &Path) -> Result<Self> {
        let init = std::fs::read("/proc/1/cmdline")?;
        anyhow::ensure!(
            String::from_utf8_lossy(&init).contains("scale.py") && init.windows(8).any(|part| part == b"--inside"),
            "process sampling requires scripts/scale.py's private PID namespace"
        );
        let mut output = std::fs::File::create(base.join("processes.jsonl"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let began = Instant::now();
            while !stopping.load(Ordering::Relaxed) {
                if let Ok(entries) = std::fs::read_dir("/proc") {
                    let processes: Vec<_> = entries
                        .flatten()
                        .filter_map(|entry| {
                            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
                            let mut value = resources::snapshot(pid).ok()?;
                            let command = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
                            value["command"] = json!(String::from_utf8_lossy(&command).replace('\0', " "));
                            Some(value)
                        })
                        .collect();
                    if serde_json::to_writer(
                        &mut output,
                        &json!({"seconds":began.elapsed().as_secs_f64(),"processes":processes,"memory_group":memory_group()}),
                    )
                    .is_err()
                        || output.write_all(b"\n").is_err()
                    {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let _ = serde_json::to_writer(&mut output, &json!({"final_memory_group":memory_group()}));
            let _ = output.write_all(b"\n");
        });
        Ok(Self { stop, thread: Some(thread) })
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn memory_group() -> Value {
    let group = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|text| text.lines().find_map(|line| line.strip_prefix("0::").map(str::to_owned)));
    let Some(group) = group else { return Value::Null };
    let directory = Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    let mut result = json!({"path":group});
    for file in ["memory.current", "memory.peak", "memory.max", "memory.events"] {
        result[file] = json!(std::fs::read_to_string(directory.join(file)).ok());
    }
    result
}
