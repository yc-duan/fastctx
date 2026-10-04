//! Real filesystem failure at the copy/publication/retirement boundary.

#[cfg(windows)]
#[test]
fn directory_move_defers_for_an_open_descendant_even_with_delete_sharing() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    let destination = home.path().join("archive");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("runtime.lock"), b"user bytes").unwrap();
    let holder = std::fs::OpenOptions::new().read(true).write(true).open(source.join("runtime.lock")).unwrap();
    assert!(super::rename_new(&source, &destination).is_err());
    assert_eq!(std::fs::read(source.join("runtime.lock")).unwrap(), b"user bytes");
    assert!(!destination.exists());
    drop(holder);
    super::rename_new(&source, &destination).unwrap();
    assert_eq!(std::fs::read(destination.join("runtime.lock")).unwrap(), b"user bytes");
}

#[cfg(windows)]
#[test]
fn copy_resumes_after_a_nested_source_directory_was_retired() {
    use std::os::windows::fs::OpenOptionsExt;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    std::fs::create_dir_all(source.join("z")).unwrap();
    std::fs::write(source.join("a"), b"held").unwrap();
    std::fs::write(source.join("z/child"), b"nested").unwrap();
    let slot = home.path().join("slot");
    std::fs::create_dir(&slot).unwrap();
    let target = slot.join("archive");
    let object = super::Object::read(&source).unwrap().unwrap();
    let hold = std::fs::OpenOptions::new().read(true).share_mode(3).open(source.join("a")).unwrap();
    assert!(super::tree::transfer(&object, &target, &[], &[]).is_err());
    // Earlier sequential retirement could remove this empty directory before
    // reaching the occupied file. Its checkpoint remains recoverable.
    std::fs::remove_dir(source.join("z")).unwrap();
    assert!(!source.join("z").exists());
    drop(hold);
    super::tree::transfer(&object, &target, &[], &[]).unwrap();
    assert!(!source.exists());
    assert_eq!(std::fs::read(target.join("z/child")).unwrap(), b"nested");
    assert_eq!(std::fs::read(target.join("a")).unwrap(), b"held");
}

#[test]
fn completed_copy_is_recognized_without_the_batch_result() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("edited"), b"keep user bytes").unwrap();
    let slot = home.path().join("slot");
    std::fs::create_dir(&slot).unwrap();
    let destination = slot.join("archive");
    let object = super::Object::read(&source).unwrap().unwrap();
    super::tree::transfer(&object, &destination, &[], &[]).unwrap();
    assert!(!source.exists());
    assert!(super::completed_copy(&object, &destination).unwrap());
    std::fs::write(destination.join("edited"), b"changed archive").unwrap();
    assert!(super::completed_copy(&object, &destination).is_err());
    assert_eq!(std::fs::read(destination.join("edited")).unwrap(), b"changed archive");
}

#[cfg(windows)]
#[test]
fn occupied_source_and_edited_archive_survive_recovery() {
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a"), b"original").unwrap();
    fs::hard_link(source.join("a"), source.join("b")).unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let slot = home.path().join("slot");
    fs::create_dir(&slot).unwrap();
    let target = slot.join("archive");
    let object = super::Object::read(&source).unwrap().unwrap();
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(source.join("a"))
        .unwrap();
    // Exercise the cross-volume mechanism on any Windows test filesystem;
    // the open sharing mode supplies a real retirement failure after publication.
    let retirement = super::tree::transfer(&object, &target, &[], &[]).unwrap_err();
    assert!(target.exists(), "copy was not published: {retirement}");
    assert_eq!(fs::read(source.join("a")).unwrap(), b"original");
    assert_eq!(fs::read(target.join("a")).unwrap(), b"original");
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
    fs::write(target.join("a"), b"user edited archive").unwrap();
    drop(hold);
    let error = super::tree::transfer(&object, &target, &[], &[]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(fs::read(source.join("a")).unwrap(), b"original");
    assert_eq!(fs::read(target.join("a")).unwrap(), b"user edited archive");
    assert_eq!(fs::read(target.join("b")).unwrap(), b"user edited archive");
}
