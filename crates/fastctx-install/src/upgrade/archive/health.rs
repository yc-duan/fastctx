//! Real filesystem failure at the copy/publication/retirement boundary.

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
