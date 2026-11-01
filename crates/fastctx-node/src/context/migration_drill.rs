//! Withdrawn context bodies leave neither verified nor damaged migration copies.
use super::{
    records,
    storage::{self, Block, State},
};
use crate::record_migration::archive;
use fastctx_platform::disk;
use fastctx_types::{ResidentId, layout::WorldDir};
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = crate::record_database::open(&world);
    let service = crate::record_database::register(&world, &database);
    service.start()?;
    let id: ResidentId = "codex-aaaa12".parse()?;
    let directory = world.conversation(&id);
    disk::create_dir_all(storage::directory(&world, &id))?;
    let block = |text: &str, transient| Block {
        revision: 1,
        written_epoch: 0,
        source: crate::profile::session::hash(text),
        version: "kept".into(),
        text: text.into(),
        transient,
        by: None,
        fact: "private fact".into(),
        full: true,
    };
    let plan = storage::directory(&world, &id).join("plan.json");
    let skills = storage::directory(&world, &id).join("skills.json");
    disk::write(&plan, serde_json::to_vec(&block("PRIVATE_PLAN", true))?)?;
    disk::write(&skills, serde_json::to_vec(&block("OWN_SKILLS", false))?)?;
    disk::write(storage::state_path(&world, &id), serde_json::to_vec(&State::default())?)?;
    let plan_copy = archive::verified_path(world.path(), id.as_str(), &plan, &archive::fingerprint(&plan)?)?;
    let skills_copy = archive::verified_path(world.path(), id.as_str(), &skills, &archive::fingerprint(&skills)?)?;
    records::prepare(&world, &id)?;
    assert!(disk::exists(&plan_copy)? && disk::exists(&skills_copy)?);
    records::repair(&world, &id)?;
    assert!(!disk::exists(&plan_copy)? && disk::exists(&skills_copy)?);
    let projected: Block = serde_json::from_slice(&disk::read(&plan)?)?;
    assert!(projected.transient && projected.text.is_empty() && projected.fact.is_empty());
    let project = storage::directory(&world, &id).join("project.json");
    disk::write(&project, b"{PRIVATE_PROJECT")?;
    archive::damaged(world.path(), id.as_str(), "context-blocks", &project, "invalid old source")?;
    let unrelated = directory.path().join("other.json");
    disk::write(&unrelated, b"{UNRELATED")?;
    archive::damaged(world.path(), id.as_str(), "other", &unrelated, "independent invalid source")?;
    storage::set(&world, &id, "project", "", super::Change { by: None, version: "", fact: "", full: true })?;
    assert_eq!(archive::damaged_count(world.path())?, 1);
    let older = world.path().join("archive/older-release").join(id.as_str()).join("context");
    disk::create_dir_all(&older)?;
    let withdrawn = older.join("plan.json.original");
    let neighbor = older.join("plan-extra.json.original");
    disk::write(&withdrawn, b"PRIVATE_OLDER_PLAN")?;
    disk::write(&neighbor, b"UNRELATED_NEIGHBOR")?;
    // Stop after the SQL intent, before the file projection has run.
    database.write_now(128, |db| {
        db.execute("INSERT INTO context_archive_withdrawals VALUES('codex-aaaa12','plan')", [])?;
        db.execute("INSERT OR IGNORE INTO context_projection_dirty VALUES('codex-aaaa12')", [])?;
        Ok(())
    })?;
    drop(service);
    drop(database);
    let database = crate::record_database::open(&world);
    let service = crate::record_database::register(&world, &database);
    service.start()?;
    records::repair(&world, &id)?;
    assert!(!disk::exists(&withdrawn)? && disk::exists(&neighbor)? && disk::exists(&skills_copy)?);
    let pending: i64 = database
        .read_now(|db| Ok(db.query_row("SELECT count(*) FROM context_archive_withdrawals", [], |row| row.get(0))?))?;
    assert_eq!(pending, 0);
    println!(
        "context withdrawal: transient original removed, damaged project removed after clear, unrelated source retained, cross-version intent recovered after reopening"
    );
    Ok(())
}
