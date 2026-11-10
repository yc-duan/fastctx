//! User-visible first-screen and page-up probes, with the current GUI backend isolated here.
use super::*;
use fastctx_client::{Conversation, Update};
use fastctx_types::session::{Happened, SessionEvent};
use sha2::{Digest, Sha256};

#[allow(clippy::too_many_arguments)]
pub(super) async fn measure(
    world: &World,
    resident: ResidentId,
    expected: u64,
    newest: u64,
    progress: &Path,
    node_pid: u32,
    pages: bool,
) -> Result<Value> {
    let node_before = resources::snapshot(node_pid)?;
    let began = Instant::now();
    let (mut conversation, history) = Conversation::open(world, resident, machine("host").into()).await?;
    let open_ms = began.elapsed().as_secs_f64() * 1000.0;
    let prepared = Instant::now();
    let (mut transcript, window) = crate::transcript::Transcript::new(&history).map_err(anyhow::Error::msg)?;
    let window_bytes = serde_json::to_vec(&window)?;
    let first_screen_ms = began.elapsed().as_secs_f64() * 1000.0;
    let prepare_first_screen_ms = prepared.elapsed().as_secs_f64() * 1000.0;
    let node_at_first_screen = resources::snapshot(node_pid).unwrap_or_else(|error| json!({"error":error.to_string()}));
    let window: Value = serde_json::from_slice(&window_bytes)?;
    anyhow::ensure!(
        String::from_utf8_lossy(&window_bytes).contains(&format!("scale-0000:{newest}")),
        "first screen does not contain the fixture's newest message"
    );
    let mut generation = window["generation"].as_u64().context("window generation")?;
    let anchor_id = format!("scale-0000:{}", newest.saturating_sub(403) / 4 * 4);
    let mut anchor = locate(&history, &anchor_id);
    let mut observed = history.len() as u64;
    let count_start = Instant::now();
    let initial_bytes = json_bytes(&history)?;
    let initial_count_ms = count_start.elapsed().as_secs_f64() * 1000.0;
    let mut count_ms = initial_count_ms;
    let mut result = json!({"ok":true,"first_screen_ok":true,"open_ms":open_ms,"first_screen_ms":first_screen_ms,
        "node_at_first_screen":node_at_first_screen,"node_first_screen":resources::difference(&node_before,&node_at_first_screen),
        "host_confirmation_waited":false,"initial_events":observed,"initial_history_json_bytes":initial_bytes,"screen":{
        "prepare_first_screen_ms":prepare_first_screen_ms,"first_screen_bytes":window_bytes.len(),
        "first_screen_events":window["events"].as_array().context("window events")?.len(),
        "content_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&window["events"])?)),
        "backend":"Conversation::open + Tauri window core; no WebView paint","page_up":[]}});
    std::fs::write(progress, serde_json::to_vec_pretty(&result)?)?;
    if !pages {
        result["first_screen_only"] = json!(true);
        return Ok(result);
    }
    let paging = Instant::now();
    let mut complete_bytes = initial_bytes;
    drop(history);
    while observed < expected {
        let update = conversation.update().await?;
        match &update {
            Update::History(history) => {
                anchor = locate(history, &anchor_id);
                observed = history.len() as u64;
                result["history_received_ms"] = json!(began.elapsed().as_secs_f64() * 1000.0 - count_ms);
                let count_start = Instant::now();
                complete_bytes = json_bytes(history)?;
                count_ms += count_start.elapsed().as_secs_f64() * 1000.0;
            }
            Update::Extended(history) => {
                observed += history.len() as u64;
                let count_start = Instant::now();
                complete_bytes += json_bytes(history)?;
                count_ms += count_start.elapsed().as_secs_f64() * 1000.0;
            }
            Update::Interrupted(why) => anyhow::bail!("history interrupted after first screen: {}", why.why),
            _ => {}
        }
        match transcript.take(update).map_err(anyhow::Error::msg)? {
            Some(crate::Piece::Rebased { generation: next, .. }) => {
                generation = next;
            }
            Some(crate::Piece::Window { window }) => {
                let window = serde_json::to_value(window)?;
                generation = window["generation"].as_u64().context("replacement generation")?;
            }
            _ => {}
        }
    }
    result["complete_history_ms"] = json!(began.elapsed().as_secs_f64() * 1000.0 - count_ms);
    result["byte_count_instrumentation_ms"] = json!(count_ms);
    result["page_up_wait_ms"] = json!(paging.elapsed().as_secs_f64() * 1000.0 - (count_ms - initial_count_ms));
    result["events"] = json!(observed);
    result["history_json_bytes"] = json!(complete_bytes);
    let mut at = anchor.context("known older-page anchor is missing")?;
    result["page_up_anchor"] = json!(anchor_id);
    let mut pages = Vec::new();
    for _ in 0..3 {
        let began = Instant::now();
        let part = transcript
            .part(generation, crate::transcript::Edge::Before { at, floor: 0 })
            .context("older page missing")?;
        let bytes = serde_json::to_vec(&part)?;
        let milliseconds = began.elapsed().as_secs_f64() * 1000.0;
        let value: Value = serde_json::from_slice(&bytes)?;
        at = value["start"].as_u64().context("page start")? as usize;
        pages.push(json!({"ms":milliseconds,"bytes":bytes.len(),"events":value["events"].as_array().context("page events")?.len(),
            "content_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&value["events"])?))}));
    }
    result["screen"]["page_up"] = json!(pages);
    std::fs::write(progress, serde_json::to_vec_pretty(&result)?)?;
    Ok(result)
}

fn locate(history: &[SessionEvent], id: &str) -> Option<usize> {
    history
        .iter()
        .position(|event| matches!(&event.happened,Happened::Heard { message:Some(message),.. } if message.id == id))
}
