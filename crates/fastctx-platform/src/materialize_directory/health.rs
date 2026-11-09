//! One platform lifecycle check: uncertain publication, restart and generation handover.
use super::*;
use crate::materialize_file::{self, FileCheckpoint, FileJournal, FileMode, FilePlan, StagedFile};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = crate::owned_directory::delete(&self.0);
    }
}
struct Journal {
    candidate: Option<FileIdentity>,
    fail_installed: bool,
}
impl FileJournal for Journal {
    fn record(&mut self, _: &FilePlan, checkpoint: FileCheckpoint) -> io::Result<()> {
        if let FileCheckpoint::Prepared { identity } = checkpoint {
            self.candidate = Some(identity);
        }
        if self.fail_installed && matches!(checkpoint, FileCheckpoint::Installed { .. }) {
            return Err(io::Error::other("injected Runtime commit failure"));
        }
        Ok(())
    }
}
impl DirectoryJournal for Journal {
    fn record(&mut self, _: &DirectoryPlan, _: DirectoryCheckpoint) -> io::Result<()> {
        Ok(())
    }
}
fn name(value: &str) -> Name {
    Name::new(value).unwrap()
}
fn journal() -> Journal {
    Journal { candidate: None, fail_installed: false }
}
fn check(file: &mut fs::File, body: &[u8]) -> io::Result<()> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes == body { Ok(()) } else { Err(io::Error::new(io::ErrorKind::InvalidData, "wrong generation bytes")) }
}
fn plan(parent: &Directory, index: u8, target: &str, body: &[u8]) -> FilePlan {
    FilePlan {
        operation: OperationId::from_bytes([index; 32]),
        parent: parent.identity(),
        stage: name(&format!("stage-{index}")),
        target: name(target),
        expected: parent.file_identity(&name(target)).unwrap(),
        bytes: body.len() as u64,
        content: Digest::of(body),
        mode: FileMode { read_only: false, executable: false },
    }
}
fn put(parent: &Directory, index: u8, body: &[u8]) -> io::Result<()> {
    let intent = plan(parent, index, "SKILL.md", body);
    let mut stage = StagedFile::create(parent.clone(), intent, &mut journal()).map_err(io::Error::other)?;
    stage.write_all(body).map_err(io::Error::other)?;
    stage
        .prepare(&mut journal(), |f, _| check(f, body))
        .map_err(io::Error::other)?
        .publish(&mut journal(), || Ok(()))
        .map_err(io::Error::other)?;
    Ok(())
}
fn verify(directory: &Directory, generation: &Generation) -> io::Result<()> {
    let body = if generation.content == Digest::of(b"old") { b"old" } else { b"new" };
    check(&mut directory.open_file(&name("SKILL.md"))?, body)
}

#[test]
fn publication_recovery_preserves_handles_and_never_exchanges_back() -> Result<(), Box<dyn std::error::Error>> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = std::env::var_os("FASTCTX_TEST_TMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base)?;
    let base = fs::canonicalize(base)?;
    let fixture = Fixture(base.join(format!(
        "materialize-health-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let root = Directory::open_root(&fixture.0, true)?;
    let first = root.create_directory(&name("generation-1"))?;
    let second = root.create_directory(&name("generation-2"))?;
    put(&first, 1, b"old")?;
    let intent = plan(&second, 2, "SKILL.md", b"new");
    let mut failed = Journal { candidate: None, fail_installed: true };
    let mut stage = StagedFile::create(second.clone(), intent.clone(), &mut failed)?;
    stage.write_all(b"new")?;
    let error = stage.prepare(&mut failed, |f, _| check(f, b"new"))?.publish(&mut failed, || Ok(())).unwrap_err();
    assert_eq!(error.visibility, Visibility::Published);
    materialize_file::recover_prepared(
        second.clone(),
        intent,
        failed.candidate.unwrap(),
        &mut journal(),
        |f, _| check(f, b"new"),
        || Ok(()),
    )?;
    let old = Generation { name: name("generation-1"), identity: first.identity(), content: Digest::of(b"old") };
    let new = Generation { name: name("generation-2"), identity: second.identity(), content: Digest::of(b"new") };
    let initial = DirectoryPlan {
        operation: OperationId::from_bytes([3; 32]),
        parent: root.identity(),
        stable: name("stable"),
        wanted: old.clone(),
        previous: None,
        slot: None,
    };
    let ready = publish(&root, &initial, &mut journal(), verify, || Ok(()))?;
    let mut opened = fs::File::open(ready.path().join("SKILL.md"))?;
    let next = DirectoryPlan {
        operation: OperationId::from_bytes([4; 32]),
        parent: root.identity(),
        stable: name("stable"),
        wanted: new,
        previous: Some(old),
        slot: Some(ready.slot_identity()),
    };
    publish(&root, &next, &mut journal(), verify, || Ok(()))?;
    for _ in 0..2 {
        let ready = recover(&root, &next, &mut journal(), verify, || Ok(()))?;
        assert_eq!(fs::read(ready.path().join("SKILL.md"))?, b"new");
        assert!(ready.context_path().ends_with('/'));
    }
    check(&mut opened, b"old")?;
    let outside = root.create_directory(&name("outside"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.display_path(), fixture.0.join("link"))?;
    #[cfg(windows)]
    crate::directory_alias::create(outside.display_path(), &fixture.0.join("link"))?;
    assert!(root.open_directory(&name("link")).is_err());
    assert!(name("stage").could_alias(&name("ＳＴＡＧＥ")));
    Ok(())
}
