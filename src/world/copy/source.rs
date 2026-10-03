//! A copy source streams a signed manifest and file bytes, then signs the final digest.
use super::{CopyArgs, ObjectKind, ReceiveArgs, walk};
use crate::session::SessionContext;
use crate::world::{
    client::WorldClient,
    messages::{Call, CallBudget, CallOrigin},
    node::executor::VerbOutcome,
    stream::{self, Control, Record},
    wire::StreamRole,
};
use std::io::Read;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub(crate) async fn run(
    client: &Arc<WorldClient>,
    call: &Call,
    _session: &Arc<SessionContext>,
    token: &CancellationToken,
    timeout: Duration,
) -> VerbOutcome {
    match tokio::select! {
        result = tokio::time::timeout(timeout, transfer(client, call, timeout)) => result.unwrap_or_else(|_| Err("copy exceeded the caller's deadline".into())),
        () = token.cancelled() => Err("copy cancelled".into()),
    } {
        Ok(outcome) => outcome,
        Err(error) => VerbOutcome::error_text(error),
    }
}
async fn transfer(
    client: &Arc<WorldClient>,
    call: &Call,
    timeout: Duration,
) -> Result<VerbOutcome, String> {
    let args: CopyArgs = serde_json::from_value(call.args.clone()).map_err(|e| e.to_string())?;
    let from = crate::paths::parse_input_path(&args.from);
    if !from.is_absolute() {
        return Err("copy source must be absolute".into());
    }
    let metadata = std::fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("copy source is a symbolic link".into());
    }
    let kind = if metadata.is_dir() {
        ObjectKind::Dir
    } else {
        ObjectKind::File
    };
    let ticket = stream::ticket(client, &args.to_node, &client.name(), &args.to_node, "copy")?;
    let receive = ReceiveArgs {
        to: args.to.clone(),
        kind,
        from_node: client.name(),
        deadline_ms: timeout.as_millis() as u64,
        ticket: ticket.clone(),
    };
    let accepted = client
        .call(
            "copy_receive",
            &args.to_node,
            serde_json::to_value(receive).map_err(|e| e.to_string())?,
            CallBudget::default(),
            timeout,
            CallOrigin::Script,
        )
        .await?;
    if accepted.status != "ok" {
        return Err(format!(
            "copy_receive: {} {:?}",
            accepted.status, accepted.message
        ));
    }
    let dialed = stream::open(client, &ticket, StreamRole::Source).await?;
    let mut channel = stream::Channel::new(client, dialed, ticket, StreamRole::Source)?;
    let (items, totals) = tokio::task::spawn_blocking(move || {
        let mut items = Vec::new();
        let totals = walk::walk(&from, kind, &|| false, &mut |item| {
            items.push(item);
            true
        })?;
        Ok::<_, String>((items, totals))
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut file_sha = None;
    for item in items {
        match item {
            walk::Item::Dir(path) => channel.send(Record::Control(Control::Dir { path })).await?,
            walk::Item::LinkSkipped(path) => {
                channel
                    .send(Record::Control(Control::LinkSkipped { path }))
                    .await?
            }
            walk::Item::Unreadable { path, reason } => return Err(format!("{path}: {reason}")),
            walk::Item::File(entry) => {
                if kind == ObjectKind::File {
                    file_sha = Some(entry.sha256.clone());
                }
                channel
                    .send(Record::Control(Control::File {
                        index: entry.index,
                        path: entry.relative,
                        size: entry.size,
                        mode: entry.mode,
                        mtime: entry.mtime,
                        sha256: entry.sha256,
                    }))
                    .await?;
                match channel.recv().await? {
                    Record::Control(Control::Skip { index }) if index == entry.index => continue,
                    Record::Control(Control::Want { index }) if index == entry.index => {}
                    _ => return Err("unexpected copy manifest response".into()),
                }
                channel
                    .send(Record::Control(Control::FileBegin { index: entry.index }))
                    .await?;
                let mut file = std::fs::File::open(&entry.path).map_err(|e| e.to_string())?;
                let mut buffer = vec![0; stream::CHUNK_BYTES];
                loop {
                    let size = file.read(&mut buffer).map_err(|e| e.to_string())?;
                    if size == 0 {
                        break;
                    }
                    channel.send(Record::Data(buffer[..size].to_vec())).await?;
                }
                channel
                    .send(Record::Control(Control::FileEnd { index: entry.index }))
                    .await?;
            }
        }
    }
    channel
        .send(Record::Control(Control::End {
            files: totals.files,
            bytes: totals.bytes,
        }))
        .await?;
    let Record::Control(Control::Done {
        copied,
        bytes,
        skipped,
        links_skipped,
        failed,
    }) = channel.recv().await?
    else {
        return Err("copy ended without a signed receipt".into());
    };
    let mut facts = std::collections::BTreeMap::from([
        ("bytes".into(), serde_json::json!(bytes)),
        ("files".into(), serde_json::json!(copied)),
    ]);
    if let Some(hash) = file_sha {
        facts.insert("sha256".into(), serde_json::json!(hash));
    }
    let text = format!(
        "{}: copied {copied} files, {bytes} bytes to {}; {skipped} unchanged, {links_skipped} links skipped",
        client.name(),
        super::endpoint(&args.to_node, &args.to, &args.caller)
    );
    if !failed.is_empty() {
        return Err(format!("{text}; failures: {failed:?}"));
    }
    Ok(VerbOutcome {
        response: crate::model::ToolResponse::text(text),
        facts,
    })
}
