//! A legacy account plan exercises the record job's CAS and retained answer.

use super::*;
use fastctx_types::{
    account_records::{Request, Response},
    plan::{Author, Body},
    submission::Submission,
};

pub(super) async fn exercise(base: &Path, world: &World) -> Result<()> {
    // The queue's resource key must accommodate the full public record-key bound.
    let key = "p".repeat(256);
    let mut expected = None;
    for markdown in ["LEGACY_FIRST", "LEGACY_SECOND"] {
        let body = Body { markdown: markdown.into(), author: Author::User, owner: None, submission: None };
        let written = world
            .account_records(
                None,
                None,
                Request::Write {
                    kind: fastctx_types::plan::KIND.into(),
                    key: key.clone(),
                    expected,
                    body: serde_json::to_value(body)?,
                },
            )
            .await?;
        let Response::Written { metadata } = written else { anyhow::bail!("legacy fixture conflicted") };
        expected = Some(metadata.version);
    }
    let before = ran_after(base, jiff::Timestamp::now(), 0, "job.ran.ask/delete-record").await?;
    let request = Request::Delete { kind: fastctx_types::plan::KIND.into(), key: key.clone(), expected };
    let named = Submission { name: "placement-delete-maximum-key".into(), waits_ms: u32::MAX };
    let first =
        fastctx_net::submission::scope(Some(named.clone()), world.account_records(None, None, request.clone())).await?;
    let again = fastctx_net::submission::scope(Some(named), world.account_records(None, None, request)).await?;
    let after = ran_after(base, jiff::Timestamp::now(), before + 1, "job.ran.ask/delete-record").await?;
    let read = world
        .account_records(None, None, Request::Read { kind: fastctx_types::plan::KIND.into(), key: key.clone() })
        .await?;
    let deleted =
        matches!(read, Response::Read { record: Some(ref record) } if record.metadata.deleted && record.body.is_null());
    let written = matches!(&first, Response::Written { metadata } if metadata.deleted && Some(metadata.version) == expected.map(|version| version + 1));
    record(
        "a maximum-length plan key is deleted once and same-name CAS replay returns the original answer",
        written && first == again && deleted && after == before + 1,
        json!({"key_bytes":key.len(),"same_answer":first==again,"ran_delta":after-before,"reply":first}),
    );
    Ok(())
}
