//! Collection progresses beside an unfinished staging lease and a live writer.
use super::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub(crate) fn run(root: &Path) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let database = Database::open_guarded(
        &root.join("node.sqlite3"),
        SCHEMA,
        crate::conversation_store::fences::guard,
        crate::conversation_store::schema::initialize,
    )?;
    let mut garbage = Vec::new();
    for index in 0..96u64 {
        let mut body = vec![b'x'; PIECE];
        body[..8].copy_from_slice(&index.to_be_bytes());
        let staged = stage(&database, "payload-drill", body, Priority::Normal)?;
        garbage.push(staged.reference.hash.clone());
        database.write_now(128, move |db| finish(db, "payload-drill", &staged))?;
    }
    let pending = Arc::new(Pin("held-stage".into()));
    LIVE.lock().expect("pins").insert(pending.0.clone(), Arc::downgrade(&pending));
    database.write_now(512,|db| {
        db.execute("INSERT INTO record_payload_leases(conversation,identity,hash) VALUES('payload-drill','held-stage','unfinished')",[])?;
        db.execute("INSERT INTO record_payload_manifests(conversation,hash,bytes,body) VALUES('payload-drill','unfinished',32768,?1)",[br#"["not-written-yet"]"#.as_slice()])?;
        Ok(())
    })?;
    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let (started, ready) = std::sync::mpsc::channel();
    let (working, ending, count) = (database.clone(), stop.clone(), writes.clone());
    let writer = std::thread::spawn(move || -> Result<()> {
        let _role = fastctx_platform::roles::scoped(fastctx_platform::roles::Role::Blocking);
        let mut index = 0u64;
        while !ending.load(Ordering::Acquire) {
            let mut body = vec![b'y'; PIECE * 2];
            body[..8].copy_from_slice(&index.to_be_bytes());
            let staged = stage(&working, "payload-drill", body.clone(), Priority::Normal)?;
            working.write_now(512, move |db| {
                link(db, "payload-drill", "live", "value", Some(&staged.reference))?;
                finish(db, "payload-drill", &staged)
            })?;
            working.read_now(|db| {
                let tx = db.unchecked_transaction()?;
                assert_eq!(read_root(&tx, "payload-drill", "live", "value")?.as_deref(), Some(body.as_slice()));
                Ok(())
            })?;
            count.fetch_add(1, Ordering::Release);
            if index == 0 {
                started.send(()).map_err(|error| Error::new(error.to_string()))?;
            }
            index += 1;
        }
        Ok(())
    });
    ready.recv_timeout(std::time::Duration::from_secs(10))?;
    let before = writes.load(Ordering::Acquire);
    let collected = collect(&database, "payload-drill");
    let after = writes.load(Ordering::Acquire);
    stop.store(true, Ordering::Release);
    writer.join().map_err(|_| "payload writer panicked")??;
    collected?;
    assert!(after > before, "the writer did not overlap collection");
    let hashes = serde_json::to_string(&garbage)?;
    database.read_now(|db| {
        let remaining:i64=db.query_row("SELECT count(*) FROM record_payload_manifests WHERE conversation='payload-drill' AND hash IN (SELECT value FROM json_each(?1))",[hashes],|row|row.get(0))?;
        assert_eq!(remaining,0,"continuous writes prevented old garbage collection");
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM record_payload_leases WHERE identity='held-stage')",[],|row|row.get::<_,bool>(0))?);
        Ok(())
    })?;
    drop(pending);
    collect(&database, "payload-drill")?;
    database.read_now(|db| {
        assert_eq!(db.query_row("SELECT count(*) FROM record_payload_manifests WHERE conversation='payload-drill'",[],|row|row.get::<_,i64>(0))?,1);
        assert_eq!(db.query_row("SELECT count(*) FROM record_payload_members p LEFT JOIN record_payload_manifests m ON m.conversation=p.conversation AND m.hash=p.manifest WHERE m.hash IS NULL",[],|row|row.get::<_,i64>(0))?,0);
        let tx=db.unchecked_transaction()?;
        assert_eq!(read_root(&tx,"payload-drill","live","value")?.expect("retained root").len(),PIECE*2);
        Ok(())
    })?;
    let body = vec![b'z'; PIECE];
    let staged = stage(&database, "damaged-payload", body.clone(), Priority::Normal)?;
    database.write_now(256, move |db| {
        link(db, "damaged-payload", "live", "value", Some(&staged.reference))?;
        finish(db, "damaged-payload", &staged)?;
        db.execute("UPDATE record_payload_chunks SET body=zeroblob(32769) WHERE conversation='damaged-payload'", [])?;
        Ok(())
    })?;
    let error =
        stage(&database, "damaged-payload", body, Priority::Normal).err().expect("oversized corrupt piece accepted");
    assert_eq!(error.kind(), ErrorKind::Corrupt);
    println!(
        "payload GC: 96 old manifests reclaimed with a live incomplete lease and {} concurrent replacements; current root intact, abandoned staging reclaimed",
        after - before
    );
    Ok(())
}
