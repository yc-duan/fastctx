//! A user's moved subtree cannot be retired through an alias left behind.

#[test]
fn retirement_refuses_a_redirected_ancestor_with_the_same_descendant_identity() {
    use std::fs;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    fs::create_dir_all(source.join("a/b")).unwrap();
    fs::write(source.join("a/b/f"), b"user bytes").unwrap();
    let object = super::Object::read(&source).unwrap().unwrap();
    let locks = super::super::LockedFiles::new();
    let manifest = super::Manifest::collect(&object, &locks).unwrap();
    let slot = home.path().join("slot");
    fs::create_dir(&slot).unwrap();
    let destination = slot.join("archive");
    super::copy(&manifest, &destination, &locks, &super::References::new()).unwrap();
    fs::write(slot.join("copy-manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let saved = home.path().join("saved-a");
    fs::rename(source.join("a"), &saved).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&saved, source.join("a")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&saved, source.join("a")).unwrap();
    assert!(super::transfer(&object, &destination, &[], &[]).is_err());
    assert_eq!(fs::read(saved.join("b/f")).unwrap(), b"user bytes");
    assert_eq!(fs::read(destination.join("a/b/f")).unwrap(), b"user bytes");
}
