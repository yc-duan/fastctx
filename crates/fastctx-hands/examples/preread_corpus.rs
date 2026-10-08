//! Runs `run`'s pre-read check over a corpus of scripts, one JSON object per
//! line on standard input (`{"id": ..., "script": ...}`), and prints one line
//! per refusal (`{"id": ..., "refusal": ...}`) and a count at the end. It
//! measures how often the check would refuse scripts models really wrote.

use std::io::{BufRead as _, Write as _};

fn main() -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let (mut checked, mut refused) = (0u64, 0u64);
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let Ok(item) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let Some(script) = item.get("script").and_then(|script| script.as_str()) else { continue };
        checked += 1;
        if let Err(failure) = fastctx_hands::run::preread::check(script) {
            refused += 1;
            let id = item.get("id").cloned().unwrap_or(serde_json::Value::Null);
            writeln!(out, "{}", serde_json::json!({ "id": id, "refusal": failure.message }))?;
        }
    }
    writeln!(out, "{}", serde_json::json!({ "checked": checked, "refused": refused }))
}
