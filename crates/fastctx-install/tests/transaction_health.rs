use std::fs;
use std::io::Read;

use fastctx_install::transaction::{FileChange, Snapshot, Transaction, cleanup_directories};

#[test]
fn host_files_and_record_share_a_transaction() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("codex/config.toml");
    let guidance = home.path().join("codex/AGENTS.md");
    let record = home.path().join("fastctx/installed.json");
    let receipt = Transaction::new([
        FileChange::write(Snapshot::read(&config).unwrap(), b"configured".as_slice()),
        FileChange::write(Snapshot::read(&guidance).unwrap(), b"guidance".as_slice()),
    ])
    .commit_with_record(Snapshot::read(&record).unwrap(), |created| Ok(serde_json::to_vec(created)?))
    .unwrap();
    assert_eq!(receipt.changed.len(), 3);
    assert_eq!(receipt.created.files.len(), 3);
    assert_eq!(receipt.created.directories.len(), 2);
    assert!(receipt.cleanup.is_empty(), "{:?}", receipt.cleanup);
    let recorded: fastctx_install::transaction::Created = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(recorded.files, receipt.created.files);

    let before = Snapshot::read(&config).unwrap();
    let mut reader = fs::File::open(&config).unwrap();
    Transaction::new([FileChange::write(before.clone(), b"new".as_slice())]).commit().unwrap();
    let mut held = Vec::new();
    reader.read_to_end(&mut held).unwrap();
    assert_eq!(held, b"configured");
    drop(reader);
    let failure = Transaction::new([FileChange::write(before, b"stale".as_slice())]).commit().unwrap_err();
    assert!(failure.to_string().contains("changed since"));
    assert_eq!(fs::read(&config).unwrap(), b"new");

    Transaction::new([&config, &guidance, &record].map(|p| FileChange::delete(Snapshot::read(p).unwrap())))
        .commit()
        .unwrap();
    assert!(cleanup_directories(&recorded.directories).is_empty());
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn unchanged_inputs_guard_receipt_publication() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let record = home.path().join("applied.json");
    let active = home.path().join("pending.json");
    fs::write(&config, b"observed").unwrap();
    let receipt = Transaction::new([FileChange::write(Snapshot::read(&config).unwrap(), b"observed".as_slice())])
        .commit_with_record_durable(&active, Snapshot::read(&record).unwrap(), |_| Ok(b"receipt".to_vec()))
        .unwrap();
    assert_eq!(receipt.changed.len(), 1);
    assert_eq!(fs::read(&config).unwrap(), b"observed");
    assert!(receipt.preserved.is_empty());
    assert_eq!(fs::read_dir(home.path().join("apply-history")).unwrap().count(), 0);
    assert!(!active.exists());
    fs::remove_file(&record).unwrap();

    let before = Snapshot::read(&config).unwrap();
    let failure = Transaction::new([FileChange::write(before, b"concurrent user's words".as_slice())])
        .commit_with_record_durable(&active, Snapshot::read(&record).unwrap(), |_| {
            fs::write(&config, b"changed while planning")?;
            Ok(b"must not commit".to_vec())
        })
        .unwrap_err();
    assert!(failure.to_string().contains("changed since"));
    assert_eq!(fs::read(&config).unwrap(), b"changed while planning");
    assert!(!record.exists());
}

#[cfg(windows)]
#[test]
fn denied_publication_restores_writes_deletions_and_owned_directories() {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let deleted = dir.path().join("deleted");
    let locked = dir.path().join("locked");
    let new = dir.path().join("new/sub/file");
    for path in [&first, &deleted, &locked] {
        fs::write(path, b"original").unwrap();
    }
    let name: Vec<u16> = deleted.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: name is a terminated path to this test's own regular file.
    assert_ne!(
        unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
                name.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN,
            )
        },
        0
    );
    fs::write(format!("{}:note", deleted.display()), b"original stream").unwrap();
    let metadata = fs::metadata(&deleted).unwrap();
    let hold = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(&locked).unwrap();
    let failure = Transaction::new([
        FileChange::write(Snapshot::read(&first).unwrap(), b"changed".as_slice()),
        FileChange::delete(Snapshot::read(&deleted).unwrap()),
        FileChange::write(Snapshot::read(&new).unwrap(), b"created".as_slice()),
        FileChange::write(Snapshot::read(&locked).unwrap(), b"cannot publish".as_slice()),
    ])
    .commit()
    .unwrap_err();
    drop(hold);
    assert_eq!(failure.cause.path.file_name(), locked.file_name());
    assert!(failure.rollback.is_empty(), "{failure}");
    assert!(failure.cleanup.is_empty(), "{failure}");
    for path in [&first, &deleted, &locked] {
        assert_eq!(fs::read(path).unwrap(), b"original");
    }
    assert!(!dir.path().join("new").exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    let restored = fs::metadata(&deleted).unwrap();
    assert_eq!(restored.file_attributes(), metadata.file_attributes());
    assert_eq!(restored.creation_time(), metadata.creation_time());
    assert_eq!(restored.last_write_time(), metadata.last_write_time());
    assert_eq!(fs::read(format!("{}:note", deleted.display())).unwrap(), b"original stream");
}

#[cfg(windows)]
#[test]
fn running_executable_is_replaced_without_stopping_its_process() {
    use fastctx_install::executable::{cleanup_retired, installed_path};
    use std::io::{BufRead, Write};
    use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let home = tempfile::tempdir().unwrap();
    let installed = installed_path(home.path());
    let current = std::env::current_exe().unwrap();
    Transaction::new([FileChange::copy_executable(Snapshot::read(&installed).unwrap(), &current).unwrap()])
        .commit()
        .unwrap();
    let mut child = Command::new(&installed)
        .args(["--exact", "image_child", "--nocapture"])
        .env_clear()
        .env("FASTCTX_INSTALL_IMAGE_CHILD", "1")
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert_ne!(output.read_line(&mut line).unwrap(), 0, "child exited before ready: {line}");
        if line.contains("IMAGE_READY") {
            break;
        }
        line.clear();
    }
    let result = || {
        let source = home.path().join("new.exe");
        let mut bytes = fs::read(&current).unwrap();
        bytes.extend_from_slice(b"new image revision");
        fs::write(&source, &bytes).unwrap();
        let locked = home.path().join("locked-config");
        fs::write(&locked, b"old config").unwrap();
        let hold =
            fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(&locked).unwrap();
        let failure = Transaction::new([
            FileChange::copy_executable(Snapshot::read(&installed).unwrap(), &source).unwrap(),
            FileChange::write(Snapshot::read(&locked).unwrap(), b"new config".as_slice()),
        ])
        .commit()
        .unwrap_err();
        assert!(failure.rollback.is_empty(), "{failure}");
        assert_eq!(fs::read(&installed).unwrap(), fs::read(&current).unwrap());
        drop(hold);
        let receipt =
            Transaction::new([FileChange::copy_executable(Snapshot::read(&installed).unwrap(), &source).unwrap()])
                .commit()
                .unwrap();
        assert_eq!(fs::read(&installed).unwrap(), bytes);
        assert!(child.try_wait().unwrap().is_none());
        assert!(!receipt.cleanup.is_empty(), "running old image must retain its last link");
    };
    // Release the fixture even if an assertion panics, so no test process leaks.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(result));
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(child.wait().unwrap().success());
    let receipt =
        Transaction::new([FileChange::copy_executable(Snapshot::read(&installed).unwrap(), &installed).unwrap()])
            .commit()
            .unwrap();
    assert!(receipt.changed.is_empty());
    assert!(receipt.cleanup.is_empty());
    assert!(cleanup_retired(&installed).is_empty());
    assert_eq!(fs::read_dir(installed.parent().unwrap()).unwrap().count(), 1);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[cfg(windows)]
#[test]
fn image_child() {
    if std::env::var_os("FASTCTX_INSTALL_IMAGE_CHILD").is_none() {
        return;
    }
    println!("IMAGE_READY");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}
