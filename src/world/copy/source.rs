//! The sending end of a `copy`: asks the destination to receive, opens the data connection,
//! walks and hashes the object, streams the manifest, sends the files the destination wants,
//! and renders the outcome for the caller.

use super::walk::{self, Entry, Item, Totals};
use super::{CopyArgs, ObjectKind, ReceiveArgs, endpoint};
use crate::head_note::{HeadMetric, HeadNote};
use crate::model::ToolResponse;
use crate::paths::{display_path, parse_input_path};
use crate::session::SessionContext;
use crate::world::client::WorldClient;
use crate::world::messages::{Call, CallBudget, CallOrigin};
use crate::world::node::executor::VerbOutcome;
use crate::world::stream::{self, Control, Failure, Opener, Record, Sealer};
use crate::world::wire::StreamRole;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// How long the destination gets to answer `copy_receive`.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(20);
/// Time kept back from the call's deadline so the progress sentence goes out before the
/// executor's own timeout fires.
const REPORT_MARGIN: Duration = Duration::from_secs(3);
/// Lines of skipped and failed entries the body lists before summarizing the rest.
const BODY_LINES: usize = 100;

#[derive(Default)]
struct Progress {
    totals: Option<Totals>,
    /// Files the destination has verified and moved into place; what "finished" means when
    /// the copy stops early. Bytes in flight are not counted.
    received_files: u64,
    received_bytes: u64,
    skipped: u64,
    unreadable: Vec<Failure>,
    /// The hash of a single-file copy, for the head note.
    file_sha256: Option<String>,
}

pub(crate) async fn run(
    client: &Arc<WorldClient>,
    call: &Call,
    _session: &Arc<SessionContext>,
    token: &CancellationToken,
    timeout: Duration,
) -> VerbOutcome {
    let args: CopyArgs = match serde_json::from_value(call.args.clone()) {
        Ok(args) => args,
        Err(error) => return VerbOutcome::error_text(format!("Invalid copy arguments: {error}")),
    };
    let me = client.name();
    let from = parse_input_path(&args.from);
    let from_display = display_path(&from);
    if !from.is_absolute() {
        return VerbOutcome::error_text(format!(
            "from must be an absolute path on the machine holding it: {from_display}"
        ));
    }
    let kind = match std::fs::symlink_metadata(&from) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return VerbOutcome::error_text(format!(
                "{from_display} is a symbolic link; copy what it points to."
            ));
        }
        Ok(metadata) if metadata.is_dir() => ObjectKind::Dir,
        Ok(metadata) if metadata.is_file() => ObjectKind::File,
        Ok(_) => {
            return VerbOutcome::error_text(format!(
                "{from_display} is not a regular file or a directory."
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return VerbOutcome::error_text(format!("{from_display} does not exist."));
        }
        Err(error) => {
            return VerbOutcome::error_text(format!("Cannot read {from_display}: {error}"));
        }
    };
    if args.to_node == me {
        return VerbOutcome::error_text(
            "from_node and to_node name the same machine; copy works between two machines.",
        );
    }
    let started = Instant::now();
    let deadline = started
        + timeout
            .saturating_sub(REPORT_MARGIN)
            .max(Duration::from_secs(5));

    // Ask the destination to receive; its answer carries the stream id, the key epoch, and
    // the destination path as that machine spells it.
    let receive = ReceiveArgs {
        to: args.to.clone(),
        kind,
        from_node: me.clone(),
        deadline_ms: deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64,
    };
    let receive_args = match serde_json::to_value(&receive) {
        Ok(value) => value,
        Err(error) => return VerbOutcome::error_text(format!("Internal copy failure: {error}")),
    };
    let accepted = match client
        .call(
            "copy_receive",
            &args.to_node,
            receive_args,
            CallBudget::default(),
            ACCEPT_TIMEOUT,
            CallOrigin::Tool,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return VerbOutcome::error_text(error),
    };
    let (stream_id, epoch) = match (&accepted.response, accepted.status.as_str()) {
        (Some(response), "ok") if !response.is_error => {
            let stream_id = accepted
                .facts
                .get("stream")
                .and_then(serde_json::Value::as_u64);
            let epoch = accepted
                .facts
                .get("epoch")
                .and_then(serde_json::Value::as_u64);
            match (stream_id, epoch) {
                (Some(stream_id), Some(epoch)) => (stream_id, epoch as u32),
                _ => {
                    return VerbOutcome::error_text(format!(
                        "{}: accepted the copy without naming a stream.",
                        args.to_node
                    ));
                }
            }
        }
        (Some(response), _) => {
            return VerbOutcome::error_text(format!("{}: {}", args.to_node, first_text(response)));
        }
        (None, status) => {
            return VerbOutcome::error_text(format!(
                "{}: {status} ({})",
                args.to_node,
                accepted.message.unwrap_or_default()
            ));
        }
    };
    let to_shown = accepted
        .facts
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&args.to)
        .to_string();
    let to_display = endpoint(&args.to_node, &to_shown, &args.caller);
    let key = match stream::blob_key(client, epoch) {
        Ok(key) => key,
        Err(error) => return VerbOutcome::error_text(error),
    };
    let dialed = match stream::open(client, stream_id, StreamRole::Source).await {
        Ok(dialed) => dialed,
        Err(error) => {
            return VerbOutcome::error_text(format!(
                "cannot open the data connection to {}: {error}",
                args.to_node
            ));
        }
    };

    let progress = Arc::new(Mutex::new(Progress::default()));
    let files: Arc<Mutex<HashMap<u64, Entry>>> = Arc::new(Mutex::new(HashMap::new()));
    let (out_tx, mut out_rx) = mpsc::channel::<Record>(16);
    let (mut writer, mut reader) = dialed.socket.split();
    let mut sealer = Sealer::new(key, stream_id, StreamRole::Source);
    let mut opener = Opener::new(key, stream_id, StreamRole::Sink);

    // Writer: the one owner of the outbound half. It does not close the connection when the
    // senders are done: the destination's report still has to arrive on the other half, and
    // a WebSocket close is not half-duplex through the hub's pump.
    let writer_task = tokio::spawn(async move {
        while let Some(record) = out_rx.recv().await {
            let bytes = sealer.seal(&record)?;
            writer
                .send(Message::Binary(bytes.into()))
                .await
                .map_err(|error| format!("the data connection closed while sending: {error}"))?;
        }
        Ok::<(), String>(())
    });

    // Walker: the manifest, on a blocking thread.
    let walker_task = {
        let out_tx = out_tx.clone();
        let files = Arc::clone(&files);
        let progress = Arc::clone(&progress);
        let token = token.clone();
        let root = from.clone();
        tokio::task::spawn_blocking(move || {
            let totals = walk::walk(&root, kind, &|| token.is_cancelled(), &mut |item| {
                let record = match item {
                    Item::Dir(path) => Control::Dir { path },
                    Item::File(entry) => {
                        let record = Control::File {
                            index: entry.index,
                            path: entry.relative.clone(),
                            size: entry.size,
                            mode: entry.mode,
                            mtime: entry.mtime,
                            sha256: entry.sha256.clone(),
                        };
                        if kind == ObjectKind::File {
                            progress.lock().file_sha256 = Some(entry.sha256.clone());
                        }
                        files.lock().insert(entry.index, entry);
                        record
                    }
                    Item::LinkSkipped(path) => Control::LinkSkipped { path },
                    Item::Unreadable { path, reason } => {
                        progress.lock().unreadable.push(Failure { path, reason });
                        return true;
                    }
                };
                out_tx.blocking_send(Record::Control(record)).is_ok()
            })?;
            progress.lock().totals = Some(totals);
            out_tx
                .blocking_send(Record::Control(Control::End {
                    files: totals.files,
                    bytes: totals.bytes,
                }))
                .map_err(|_| "the data connection closed before the manifest ended".to_string())
        })
    };

    // Reader: the destination's wants, skips, and final report.
    let (want_tx, mut want_rx) = mpsc::unbounded_channel::<u64>();
    let reader_task = {
        let progress = Arc::clone(&progress);
        tokio::spawn(async move {
            loop {
                let bytes = match reader.next().await {
                    None | Some(Ok(Message::Close(_))) => {
                        return Err("the data connection closed before the destination reported"
                            .to_string());
                    }
                    Some(Err(error)) => {
                        return Err(format!("the data connection failed: {error}"));
                    }
                    Some(Ok(Message::Binary(bytes))) => bytes,
                    Some(Ok(_)) => continue,
                };
                match opener.open(&bytes)? {
                    Record::Control(Control::Want { index }) => {
                        if want_tx.send(index).is_err() {
                            return Err("the sender stopped".to_string());
                        }
                    }
                    Record::Control(Control::Skip { .. }) => progress.lock().skipped += 1,
                    Record::Control(Control::Received { bytes, .. }) => {
                        let mut progress = progress.lock();
                        progress.received_files += 1;
                        progress.received_bytes += bytes;
                    }
                    Record::Control(done @ Control::Done { .. }) => return Ok(done),
                    _ => {}
                }
            }
        })
    };

    // Sender: the wanted files, in the order the destination asked for them.
    let sender_task = {
        let out_tx = out_tx.clone();
        let files = Arc::clone(&files);
        tokio::spawn(async move {
            while let Some(index) = want_rx.recv().await {
                let Some(entry) = files.lock().get(&index).cloned() else {
                    continue;
                };
                out_tx
                    .send(Record::Control(Control::FileBegin { index }))
                    .await
                    .map_err(|_| "the data connection closed".to_string())?;
                let chunks = out_tx.clone();
                let path = entry.path.clone();
                // A file that could not be read to its end still gets its end record: the
                // destination's hash check fails it and reports it, once, so the read error
                // itself is not recorded here.
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let mut file = std::fs::File::open(&path)
                        .map_err(|error| format!("Cannot read {}: {error}", display_path(&path)))?;
                    let mut buffer = vec![0_u8; stream::CHUNK_BYTES];
                    loop {
                        let read = file.read(&mut buffer).map_err(|error| {
                            format!("Cannot read {}: {error}", display_path(&path))
                        })?;
                        if read == 0 {
                            break Ok(());
                        }
                        chunks
                            .blocking_send(Record::Data(buffer[..read].to_vec()))
                            .map_err(|_| "the data connection closed".to_string())?;
                    }
                })
                .await
                .map_err(|error| format!("Internal copy failure: {error}"))?
                .ok();
                out_tx
                    .send(Record::Control(Control::FileEnd { index }))
                    .await
                    .map_err(|_| "the data connection closed".to_string())?;
            }
            Ok::<(), String>(())
        })
    };
    drop(out_tx);

    let mut walker_task = walker_task;
    let mut reader_task = reader_task;
    let mut sender_task = sender_task;
    let mut writer_task = writer_task;
    let mut walker_done = false;
    let mut sender_done = false;
    let mut writer_done = false;
    let outcome = loop {
        tokio::select! {
            result = &mut reader_task => break match result {
                Ok(Ok(done)) => Ok(done),
                Ok(Err(error)) => Err(Stop::Interrupted(error)),
                Err(error) => Err(Stop::Interrupted(format!("the reader stopped: {error}"))),
            },
            result = &mut walker_task, if !walker_done => {
                walker_done = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => break Err(Stop::Interrupted(error)),
                    Err(error) => break Err(Stop::Interrupted(format!("the walker stopped: {error}"))),
                }
            }
            result = &mut sender_task, if !sender_done => {
                sender_done = true;
                if let Ok(Err(error)) = result {
                    break Err(Stop::Interrupted(error));
                }
            }
            result = &mut writer_task, if !writer_done => {
                writer_done = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => break Err(Stop::Interrupted(error)),
                    Err(error) => break Err(Stop::Interrupted(format!("the writer stopped: {error}"))),
                }
            }
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => break Err(Stop::Timeout),
            () = token.cancelled() => break Err(Stop::Cancelled),
        }
    };
    reader_task.abort();
    walker_task.abort();
    sender_task.abort();
    writer_task.abort();

    let elapsed = started.elapsed();
    let progress = progress.lock();
    match outcome {
        Ok(Control::Done {
            copied,
            bytes,
            skipped,
            links_skipped,
            failed,
        }) => render_done(
            &args,
            &me,
            &to_display,
            kind,
            &progress,
            Report {
                copied,
                bytes,
                skipped,
                links_skipped,
                failed,
            },
        ),
        Ok(_) => VerbOutcome::error_text("the destination ended the copy without a report"),
        Err(stop) => {
            let files = match progress.totals {
                Some(totals) => format!(
                    "{} of {} files finished",
                    progress.received_files, totals.files
                ),
                None => format!("{} files finished so far", progress.received_files),
            };
            let tail = "finished files stay, run it again to continue.";
            VerbOutcome::error_text(match stop {
                Stop::Timeout => format!(
                    "copy stopped after {} s with {files} ({} bytes) at {to_display}; {tail}",
                    elapsed.as_secs(),
                    progress.received_bytes
                ),
                Stop::Cancelled => format!(
                    "copy was cancelled after {} s with {files} ({} bytes) at {to_display}; {tail}",
                    elapsed.as_secs(),
                    progress.received_bytes
                ),
                Stop::Interrupted(error) => format!(
                    "copy to {to_display} was interrupted after {} s ({files}, {} bytes): {error}; {tail}",
                    elapsed.as_secs(),
                    progress.received_bytes
                ),
            })
        }
    }
}

enum Stop {
    Timeout,
    Cancelled,
    Interrupted(String),
}

struct Report {
    copied: u64,
    bytes: u64,
    skipped: u64,
    links_skipped: u64,
    failed: Vec<Failure>,
}

fn render_done(
    args: &CopyArgs,
    me: &str,
    to_display: &str,
    kind: ObjectKind,
    progress: &Progress,
    report: Report,
) -> VerbOutcome {
    let subject = format!(
        "copy {} to {to_display}",
        endpoint(
            me,
            &display_path(&parse_input_path(&args.from)),
            &args.caller
        )
    );
    let mut note = match kind {
        ObjectKind::File if report.copied == 1 => HeadNote::new(
            subject,
            HeadMetric::event(format!("{} bytes", report.bytes)),
        ),
        ObjectKind::File => {
            let mut note = HeadNote::new(subject, HeadMetric::event("0 bytes copied"));
            if report.skipped == 1 {
                note = note.fact("identical, skipped");
            }
            note
        }
        ObjectKind::Dir => HeadNote::new(
            subject,
            HeadMetric::event(format!(
                "{} {}, {} bytes copied",
                report.copied,
                if report.copied == 1 { "file" } else { "files" },
                report.bytes
            )),
        ),
    };
    if let (ObjectKind::File, Some(sha256)) = (kind, &progress.file_sha256) {
        note = note.fact(format!("sha256 {sha256}"));
    }
    if kind == ObjectKind::Dir && report.skipped > 0 {
        note = note.fact(format!("{} identical skipped", report.skipped));
    }
    if report.links_skipped > 0 {
        note = note.fact(format!(
            "{} {} skipped",
            report.links_skipped,
            if report.links_skipped == 1 {
                "symlink"
            } else {
                "symlinks"
            }
        ));
    }
    let unreadable = progress.unreadable.len() as u64;
    let failed_count = report.failed.len() as u64 + unreadable;
    if failed_count > 0 {
        note = note.fact(format!("{failed_count} failed"));
    }
    let mut lines = Vec::new();
    for failure in report.failed.iter().chain(progress.unreadable.iter()) {
        lines.push(format!("{}: {}", failure.path, failure.reason));
    }
    if lines.len() > BODY_LINES {
        let more = lines.len() - BODY_LINES;
        lines.truncate(BODY_LINES);
        lines.push(format!("... and {more} more"));
    }
    let mut facts = BTreeMap::new();
    facts.insert("copied".to_string(), serde_json::json!(report.copied));
    facts.insert("bytes".to_string(), serde_json::json!(report.bytes));
    facts.insert("skipped".to_string(), serde_json::json!(report.skipped));
    facts.insert("failed".to_string(), serde_json::json!(failed_count));
    if let Some(sha256) = &progress.file_sha256 {
        facts.insert("sha256".to_string(), serde_json::json!(sha256));
    }
    VerbOutcome {
        response: note.into_text_response(&lines.join("\n")),
        facts,
    }
}

fn first_text(response: &ToolResponse) -> String {
    response
        .content
        .iter()
        .find_map(|block| match block {
            crate::model::ToolContent::Text(text) => Some(text.clone()),
            crate::model::ToolContent::Image { .. } => None,
        })
        .unwrap_or_default()
}
