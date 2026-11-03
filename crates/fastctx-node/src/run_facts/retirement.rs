//! Output-file retention and fact retention are different lifetimes. These roots
//! belong to retained SQL facts and disappear when those actual owners disappear.
use super::projections;
use fastctx_storage::{Result, rusqlite::Connection};

pub(crate) const SCHEMA: &str = "
CREATE TRIGGER IF NOT EXISTS run_call_projection_delete AFTER DELETE ON run_calls BEGIN
    DELETE FROM record_payload_roots WHERE conversation=OLD.conversation AND identity=OLD.call AND family IN ('run-notes','run-spans');
END;
CREATE TRIGGER IF NOT EXISTS run_job_projection_delete AFTER DELETE ON run_jobs BEGIN
    DELETE FROM record_payload_roots WHERE conversation=OLD.conversation AND family='run-job-notes' AND identity=json_array(OLD.call,OLD.machine,OLD.job);
END;
";

pub(super) fn unavailable(db: &Connection, conversation: &str, call: &str) -> Result<()> {
    for family in [projections::NOTES, projections::SPANS] {
        crate::record_payloads::link(db, conversation, family, call, None)?;
    }
    Ok(())
}

#[cfg(feature = "storage-drill")]
pub(super) fn drill(database: &fastctx_storage::Database, update: &fastctx_types::run_facts::JobFacts) -> Result<()> {
    use crate::record_payloads as payloads;
    let call = "retire-\"quoted\"-α";
    let mut stored =
        database.read_now(|db| super::sql::head(db, "conversation", &update.call_id))?.expect("existing call");
    stored.call_id = call.into();
    let prepared =
        projections::Call::prepare(database, "conversation", &mut stored, fastctx_storage::Priority::Normal)?;
    let notes = payloads::stage(database, "conversation", b"[]".to_vec(), fastctx_storage::Priority::Normal)?;
    let mut job = update.clone();
    job.call_id = call.into();
    job.notes = None;
    let saved = job.clone();
    database.write_now(4096, move |db| {
        super::sql::save(db, "conversation", &stored)?;
        prepared.install(db, "conversation", call, false)?;
        super::sql::save_job(db, "conversation", &saved, Some(&notes.reference))?;
        assert!(payloads::root(db, "conversation", projections::NOTES, call)?.is_some());
        assert!(payloads::root(db, "conversation", projections::SPANS, call)?.is_some());
        assert!(payloads::root(db, "conversation", projections::JOB_NOTES, &projections::job_key(&saved)?)?.is_some());
        payloads::finish(db, "conversation", &notes)
    })?;
    let deleting = job.clone();
    database.write_now(128, move |db| {
        db.execute(
            "DELETE FROM run_jobs WHERE conversation='conversation' AND call=?1 AND machine=?2 AND job=?3",
            (call, deleting.machine.as_str(), deleting.job.as_str()),
        )?;
        assert!(
            payloads::root(db, "conversation", projections::JOB_NOTES, &projections::job_key(&deleting)?)?.is_none()
        );
        assert!(payloads::root(db, "conversation", projections::NOTES, call)?.is_some());
        let mut stored = super::sql::head(db, "conversation", call)?.expect("retained call");
        stored.unavailable = Some("no detailed executor facts".into());
        super::sql::save(db, "conversation", &stored)?;
        assert!(payloads::root(db, "conversation", projections::NOTES, call)?.is_none());
        assert!(payloads::root(db, "conversation", projections::SPANS, call)?.is_none());
        Ok(())
    })?;
    let mut stored = database.read_now(|db| super::sql::head(db, "conversation", call))?.expect("retained call");
    stored.unavailable = None;
    let prepared =
        projections::Call::prepare(database, "conversation", &mut stored, fastctx_storage::Priority::Normal)?;
    database.write_now(1024, move |db| {
        super::sql::save(db, "conversation", &stored)?;
        prepared.install(db, "conversation", call, false)?;
        db.execute("DELETE FROM run_calls WHERE conversation='conversation' AND call=?1", [call])?;
        assert!(payloads::root(db, "conversation", projections::NOTES, call)?.is_none());
        assert!(payloads::root(db, "conversation", projections::SPANS, call)?.is_none());
        Ok(())
    })?;
    assert!(
        !super::acceptance::receive(database, "conversation", &job, |_| panic!("retired call revived"))
            .map_err(|error| fastctx_storage::Error::new(error.to_string()))?
    );
    println!(
        "run projections: job/call owner deletion detached roots; unavailable call detached projections; late frame did not recreate the deleted call"
    );
    Ok(())
}
