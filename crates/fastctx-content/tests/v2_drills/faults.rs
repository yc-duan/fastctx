//! A drill of faults at each step that keeps an object (design §28.40
//! five): a full disk, a sync, a rename, a directory's sync or the
//! registering that fails as an object is published or received, and a
//! database commit refused for want of space or kept with its outcome
//! untold. Each time the step errs, nothing is taken as present that is not
//! wholly there, nothing present before is lost, and done again — after a
//! restart where the fault left a transfer half recorded — it goes through,
//! the object read back whole and present once.
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, Library};
use fastctx_platform::lab::{arm_fault, disarm_faults};
use fastctx_storage::rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::path::Path;

fn read_whole(library: &Library, object: &ObjectRef) -> Vec<u8> {
    let mut back = Vec::new();
    while (back.len() as u64) < object.size {
        let piece = library.read(&object.hash, back.len() as u64, 1024 * 1024).unwrap();
        assert!(!piece.is_empty(), "an object present reads to its end");
        back.extend(piece);
    }
    back
}

fn live_lives(dir: &Path, object: &ObjectRef) -> i64 {
    let db = Connection::open(dir.join("assets.sqlite3")).unwrap();
    db.query_row("SELECT COUNT(*) FROM objects WHERE hash = ?1 AND state = 1", params![&object.hash[..]], |row| row.get(0)).unwrap()
}

/// Publishes `bytes` with a fault armed at `point` (its `nth` pass, in the
/// database `detail` names), then again: what each time must hold.
fn publish(dir: &Path, library: &Library, point: &str, detail: Option<&str>, nth: u64, bytes: &[u8], kept: &ObjectRef) {
    let object = ObjectRef { hash: Sha256::digest(bytes).into(), size: bytes.len() as u64 };
    arm_fault(point, detail, nth);
    let failed = library.put(&mut &bytes[..], "drill", LEASE);
    disarm_faults();
    let error = failed.expect_err(&format!("{point}: the put errs"));
    if point == "commit-unknown" {
        // Kept, its outcome untold: present, and doing it again leases it.
        assert_eq!(library.size(&object.hash).unwrap(), Some(object.size), "{point}: {error}");
    } else {
        assert_eq!(library.size(&object.hash).unwrap(), None, "{point}: nothing present after {error}");
    }
    let lease = library.put(&mut &bytes[..], "drill", LEASE).unwrap_or_else(|error| panic!("{point}: again: {error}"));
    assert_eq!(lease.object, object);
    assert_eq!(read_whole(library, &object), bytes, "{point}: read back whole");
    assert_eq!(live_lives(dir, &object), 1, "{point}: present once");
    library.release(lease).unwrap();
    assert_eq!(read_whole(library, kept).len() as u64, kept.size, "{point}: what was present before stays");
}

/// Receives `bytes` with a fault armed at `point`, opening the library
/// again after it as a restart would, and goes on until it is present.
fn receive(dir: &Path, point: &str, bytes: &[u8]) {
    let object = ObjectRef { hash: Sha256::digest(bytes).into(), size: bytes.len() as u64 };
    let digests: Vec<[u8; 32]> = bytes.chunks(CHUNK_BYTES as usize).map(|chunk| Sha256::digest(chunk).into()).collect();
    let chunk = |i: u64| &bytes[(i * CHUNK_BYTES) as usize..(((i + 1) * CHUNK_BYTES) as usize).min(bytes.len())];
    {
        let (library, _) = Library::open(dir).unwrap();
        let mut transfer = library.receive(object, digests.clone(), Some("hub".into())).unwrap().unwrap();
        arm_fault(point, None, 1);
        let mut erred = false;
        for i in transfer.missing() {
            // A chunk that erred is given again: written if it was not, kept
            // to be synced with the next if it was.
            if transfer.chunk(i, chunk(i)).is_err() {
                erred = true;
                transfer.chunk(i, chunk(i)).unwrap_or_else(|error| panic!("{point}: chunk {i} again: {error}"));
            }
        }
        if transfer.settle().is_err() {
            erred = true;
        }
        let finished = transfer.finish("drill", LEASE);
        disarm_faults();
        erred |= finished.is_err();
        assert!(erred, "{point}: the fault was met");
        if let Ok(lease) = finished {
            library.release(lease).unwrap();
        }
        if library.size(&object.hash).unwrap().is_some() {
            assert_eq!(read_whole(&library, &object), bytes, "{point}: present only whole");
        }
    }
    // Opened again, as after a restart: what was recorded goes on.
    let (library, _) = Library::open(dir).unwrap();
    if let Some(mut transfer) = library.receive(object, digests.clone(), None).unwrap() {
        for i in transfer.missing() {
            transfer.chunk(i, chunk(i)).unwrap();
        }
        let lease = transfer.finish("drill", LEASE).unwrap_or_else(|error| panic!("{point}: finished again: {error}"));
        library.release(lease).unwrap();
    }
    assert_eq!(read_whole(&library, &object), bytes, "{point}: received whole");
    assert_eq!(live_lives(dir, &object), 1, "{point}: present once");
}

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-faults-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (library, _) = Library::open(&dir).unwrap();
    let before: Vec<u8> = (0..2 * 1024 * 1024 + 5).map(|i| (i * 13 % 241) as u8).collect();
    let kept = library.put(&mut before.as_slice(), "drill", LEASE).unwrap().object;
    let large = |seed: u64| -> Vec<u8> { (0..3 * 1024 * 1024 + 17).map(|i| ((i * 31 + seed) % 251) as u8).collect() };
    let points: [(&str, Option<&str>, u64); 7] = [
        ("full-object-write", None, 1),
        ("object-sync", None, 1),
        ("object-rename", None, 1),
        ("object-directory-sync", None, 1),
        ("object-register", None, 1),
        ("full-commit", Some("assets.sqlite3"), 1),
        ("commit-unknown", Some("assets.sqlite3"), 2),
    ];
    for (seed, (point, detail, nth)) in points.into_iter().enumerate() {
        publish(&dir, &library, point, detail, nth, &large(seed as u64), &kept);
    }
    // A short object, kept in the database beside the rows.
    publish(&dir, &library, "full-object-bytes", None, 1, b"a short object", &kept);
    publish(&dir, &library, "full-commit", Some("asset-bytes.sqlite3"), 1, b"another short object", &kept);
    drop(library);
    // What a fault left behind is cleared as the library opens again.
    let (library, _) = Library::open(&dir).unwrap();
    assert_eq!(read_whole(&library, &kept), before);
    drop(library);
    let received = |seed: u64| -> Vec<u8> { (0..4 * CHUNK_BYTES + 999).map(|i| ((i * 7 + seed) % 253) as u8).collect() };
    for (seed, point) in
        ["full-receive-write", "receive-sync", "receive-range", "receive-finish-sync", "receive-rename", "receive-register"]
            .into_iter()
            .enumerate()
    {
        receive(&dir, point, &received(seed as u64 + 100));
    }
    let (library, _) = Library::open(&dir).unwrap();
    assert_eq!(read_whole(&library, &kept), before, "what was present before every fault stays");
    drop(library);
    println!("v2 faults drill: ok");
    let _ = std::fs::remove_dir_all(&dir);
}
