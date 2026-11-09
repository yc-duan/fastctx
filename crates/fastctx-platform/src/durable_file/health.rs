//! The mkdir-to-confirm interval cannot be skipped by a concurrent publisher.
use super::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[test]
fn directory_publication_confirmation_lifecycle() {
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "fastctx-directory-health-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let target = root.join("scope");
    let cache = Arc::new(Cache::default());
    let confirmations = Arc::new(AtomicUsize::new(0));
    let (created, ready) = mpsc::channel();
    let (release, continue_after_mkdir) = mpsc::channel();
    let a = cache.clone();
    let path = target.clone();
    let counted = confirmations.clone();
    let first = thread::spawn(move || {
        a.prepare(&path, &|entry| {
            if entry == path {
                assert!(entry.is_dir());
                counted.fetch_add(1, Ordering::SeqCst);
                created.send(()).unwrap();
                continue_after_mkdir.recv().unwrap();
            }
        })
        .unwrap()
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    let b = cache.clone();
    let path = target.clone();
    let counted = confirmations.clone();
    let (entered, started) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let second = thread::spawn(move || {
        entered.send(()).unwrap();
        b.prepare(&path, &|entry| {
            if entry == path {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
        done.send(()).unwrap();
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(finished.recv_timeout(Duration::from_millis(50)), Err(mpsc::RecvTimeoutError::Timeout)));
    release.send(()).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    finished.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(confirmations.load(Ordering::SeqCst), 1);
    Cache::default()
        .prepare(&target, &|entry| {
            if entry == target {
                confirmations.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    assert_eq!(confirmations.load(Ordering::SeqCst), 2, "a fresh process must reconfirm an existing directory");
    fs::remove_dir(&target).unwrap();
    fs::remove_dir(&root).unwrap();
}
