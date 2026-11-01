//! Retry and terminal diagnostics for outgoing answers share their custody
//! transaction. Diagnostics expire 30 days after the answer has settled.

use super::*;
use crate::records;
use fastctx_types::residents::Owing;

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    storage::initialize(directory)?;
    for (family, extension) in
        [("answer-retries", "retry"), ("answer-outcomes", "done"), ("answer-attempts", "attempt")]
    {
        records::ensure_each(directory, family, |push| {
            for path in entries(&directory.outbox())? {
                if path.extension().is_none_or(|found| found != extension)
                    || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                let key = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| io::Error::other("invalid answer diagnostic"))?
                    .to_string();
                let bytes = std::fs::read(&path)?;
                let body = if extension == "retry" {
                    serde_json::to_vec(&String::from_utf8(bytes).map_err(io::Error::other)?)?
                } else {
                    serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&bytes)?)?
                };
                push(records::Source::new(key, path, body)?)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
pub(crate) async fn retry(node: &Arc<Node>, directory: &ConversationDir, path: &Path, reason: &str) -> io::Result<()> {
    let (node, directory, reason) = (node.clone(), directory.clone(), reason.to_string());
    let key = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("invalid outgoing answer identity"))?
        .to_string();
    tokio::task::spawn_blocking(move || {
        prepare(&directory)?;
        let changed = records::write_at(&directory, fastctx_storage::Priority::Background, reason.len(), {
            let (key, reason) = (key.clone(), reason.clone());
            move |rows| {
                if rows.get::<String>("answer-retries", &key)?.as_deref() == Some(&reason) {
                    return Ok(false);
                }
                rows.put("answer-retries", &key, &reason)?;
                Ok(true)
            }
        })?;
        if changed {
            node.log.line(format_args!("outgoing answer {key} remains pending: {reason}"));
        }
        Ok(())
    })
    .await
    .map_err(io::Error::other)?
}
pub(crate) async fn abandon(
    node: &Arc<Node>,
    directory: &ConversationDir,
    owing: &Owing,
    reason: &str,
) -> io::Result<()> {
    let (node, directory, owing, reason) = (node.clone(), directory.clone(), owing.clone(), reason.to_string());
    tokio::task::spawn_blocking(move || {
        prepare(&directory)?;
        let key = owing.delivery.id.to_string();
        let expected = records::read(&directory, |rows| {
            if let Some(current) = super::answers::get(rows, &key)?
                && current != owing
            {
                return Err(fastctx_storage::Error::new("outgoing answer identity changed"));
            }
            rows.get_raw("outgoing-answers", &key)
        })?;
        let bytes = expected.as_ref().map_or(0, Vec::len) + reason.len();
        records::write_at(&directory, fastctx_storage::Priority::Normal, bytes, {
            let reason = reason.clone();
            move |rows| {
                if let Some(current) = rows.get_raw("outgoing-answers", &key)?
                    && Some(&current) != expected.as_ref()
                {
                    return Err(fastctx_storage::Error::new("outgoing answer identity changed"));
                }
                rows.put("undeliverable-answers", &key, &owing.to)?;
                rows.put_until(
                    "answer-outcomes",
                    &key,
                    &serde_json::json!({"at":jiff::Timestamp::now(),"outcome":"impossible","reason":reason}),
                    Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(30 * 24)),
                )?;
                settled(rows, &key)?;
                rows.delete("outgoing-answers", &key)?;
                Ok(())
            }
        })?;
        node.log.line(format_args!("outgoing answer can no longer be delivered: {reason}"));
        Ok(())
    })
    .await
    .map_err(io::Error::other)?
}

pub(crate) async fn attempt(
    directory: &ConversationDir,
    key: &fastctx_types::NoticeId,
    line: fastctx_types::residents::MessageLine,
) -> io::Result<()> {
    let (directory, key) = (directory.clone(), key.to_string());
    tokio::task::spawn_blocking(move || {
        prepare(&directory)?;
        crate::residents::activity::prepare(&directory, "messages")?;
        let value = serde_json::to_value(&line.what)?;
        let bytes = serde_json::to_vec(&line)?.len();
        records::write_at(&directory, fastctx_storage::Priority::Normal, bytes, move |rows| {
            if rows.get::<serde_json::Value>("answer-attempts", &key)?.as_ref() == Some(&value) {
                return Ok(());
            }
            crate::residents::activity::message_in(rows, &line)?;
            rows.put("answer-attempts", &key, &value)
        })
    })
    .await
    .map_err(io::Error::other)?
}

pub(crate) fn settled(rows: &records::Rows<'_>, key: &str) -> fastctx_storage::Result<()> {
    rows.delete("answer-retries", key)?;
    if let Some(attempt) = rows.get::<serde_json::Value>("answer-attempts", key)? {
        rows.put_until(
            "answer-attempts",
            key,
            &attempt,
            Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(30 * 24)),
        )?;
    }
    Ok(())
}
