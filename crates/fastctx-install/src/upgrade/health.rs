//! Checkpoints survive process interruption and real Windows sharing failures.

use std::fs;

use super::{Plan, archive, paths, record, run};
use crate::apply::Places;

fn places(home: &std::path::Path) -> Places {
    Places {
        root: home.join(".fastctx"),
        world: home.join(".fastctx/world"),
        codex_home: home.join(".codex"),
        claude_home: home.join(".claude"),
        claude: None,
        fastctx: std::env::current_exe().unwrap(),
    }
}

#[test]
fn pending_batch_recovers_after_move_without_result_checkpoint() {
    let home = tempfile::tempdir().unwrap();
    let places = places(home.path());
    fs::create_dir(&places.root).unwrap();
    let source = home.path().join("runtime");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("edited"), b"user bytes").unwrap();
    let path = places.root.join("upgrade-clean.json");
    let mut record = record::Record::open(&path).unwrap();
    record.begin_batch();
    let object = archive::Object::read(&source).unwrap().unwrap();
    let (index, target, copying) = record.prepare_archive(&object, "runtime", "retain", &places.root).unwrap();
    record.save().unwrap();
    assert_eq!(super::summary(&places).unwrap().unwrap().waiting, 1);

    #[cfg(windows)]
    let reader = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap()
    };
    archive::transfer(&object, &target, &[], copying, &[], || Ok(())).unwrap();
    record.archive_complete(index).unwrap();
    #[cfg(windows)]
    {
        let before = fs::read(&path).unwrap();
        assert!(record.save().unwrap_err().to_string().contains("next startup"));
        assert_eq!(fs::read(&path).unwrap(), before);
        drop(reader);
    }
    drop(record);
    let mut record = record::Record::open(&path).unwrap();
    record.recover(&paths::Paths::legacy().unwrap(), &places).unwrap();
    record.save().unwrap();
    assert_eq!(fs::read(target.join("edited")).unwrap(), b"user bytes");
    assert!(!source.exists());
    let summary = super::summary(&places).unwrap().unwrap();
    assert_eq!((summary.archived, summary.waiting), (1, 0));
    let mut record = record::Record::open(&path).unwrap();
    record.recover(&paths::Paths::legacy().unwrap(), &places).unwrap();
    record.save().unwrap();
    assert!(!record.wrote());
}

#[test]
fn old_per_file_checkpoint_is_finished_before_directory_archive() {
    let home = tempfile::tempdir().unwrap();
    let places = places(home.path());
    fs::create_dir(&places.root).unwrap();
    let source = home.path().join("runtime");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("old.lock"), b"old user edit").unwrap();
    fs::write(source.join("added"), b"new user file").unwrap();
    let path = places.root.join("upgrade-clean.json");
    let mut record = record::Record::open(&path).unwrap();
    record.begin_batch();
    let child = archive::Object::read(&source.join("old.lock")).unwrap().unwrap();
    let (_, old_target, _) = record.prepare_archive(&child, "runtime", "old step", &places.root).unwrap();
    record.save().unwrap();
    drop(record);

    let mut record = record::Record::open(&path).unwrap();
    let mut plan = Plan::default();
    plan.archive(&source, "runtime", "whole directory");
    run::execute(&mut plan, &mut record, &places).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read(old_target).unwrap(), b"old user edit");
    let summary = super::summary(&places).unwrap().unwrap();
    assert_eq!((summary.archived, summary.waiting), (2, 0));
    let record: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let directory = std::path::PathBuf::from(record["archives"][1]["destination"].as_str().unwrap());
    assert_eq!(fs::read(directory.join("added")).unwrap(), b"new user file");
}
