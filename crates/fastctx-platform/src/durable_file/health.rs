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
    assert!(cache.state.lock().unwrap().inflight.is_empty());
    let refused = root.join("refused");
    fs::write(&refused, b"not a directory").unwrap();
    for _ in 0..32 {
        assert!(cache.prepare(&refused, &|_| {}).is_err());
        let mut state = cache.state.lock().unwrap();
        assert!(state.inflight.is_empty());
        assert!(!state.confirmed.contains(&refused));
    }
    fs::remove_file(&refused).unwrap();
    cache.prepare(&refused, &|_| {}).unwrap();
    let interrupted = root.join("interrupted");
    assert!(
        std::panic::catch_unwind(|| cache.prepare(&interrupted, &|entry| {
            if entry == interrupted {
                panic!("interrupt after mkdir");
            }
        }))
        .is_err()
    );
    assert!(cache.state.lock().unwrap().inflight.is_empty());
    cache.prepare(&interrupted, &|_| {}).unwrap();

    let limited = Cache { limit: 8192, ..Cache::default() };
    for scope in 0..32 {
        let bucket = root.join(format!("scope-{scope}")).join("ab").join("cd");
        limited.prepare(&bucket, &|_| {}).unwrap();
        let state = limited.state.lock().unwrap();
        assert!(state.inflight.is_empty());
        assert!(state.confirmed.bytes <= limited.limit);
    }
    assert!(limited.state.lock().unwrap().confirmed.evictions > 0);
    let evicted = root.join("scope-0").join("ab").join("cd");
    let refreshed = AtomicUsize::new(0);
    limited
        .prepare(&evicted, &|entry| {
            if entry == evicted {
                refreshed.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
    assert_eq!(refreshed.load(Ordering::SeqCst), 1, "eviction requires a fresh confirmation");

    let mut packed = Confirmed::default();
    let first = root.join("sparse").join("00");
    packed.remember(&first, CACHE_BYTES);
    let baseline = packed.bytes;
    for second in 0..256 {
        packed.remember(&first.join(format!("{second:02x}")), CACHE_BYTES);
    }
    assert!(packed.bytes < baseline + 1024, "one second-level bitmap covers its 256 directories");
    for first_byte in 1..256 {
        packed.remember(&root.join("sparse").join(format!("{first_byte:02x}")).join("00"), CACHE_BYTES);
    }
    assert!(packed.bytes < baseline + 256 * 1024, "second-level bitmaps allocate only for touched prefixes");
    fs::remove_dir_all(&root).unwrap();
}
