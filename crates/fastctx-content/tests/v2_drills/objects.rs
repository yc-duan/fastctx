//! A drill of the v2 object store.
use fastctx_content::v2::{LEASE, Library};
use fastctx_storage::rusqlite::{Connection, params};
use std::time::Duration;

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-objects-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (library, started) = Library::open(&dir).unwrap();
    assert_eq!(started, Default::default());
    // Small and large puts.
    let small = library.put(&mut &b"hello asset"[..], "drill", LEASE).unwrap();
    let big: Vec<u8> = (0..3 * 1024 * 1024 + 17).map(|i| (i * 31 % 251) as u8).collect();
    let large = library.put(&mut big.as_slice(), "drill", LEASE).unwrap();
    assert_eq!(library.size(&small.object.hash).unwrap(), Some(11));
    assert_eq!(library.read(&small.object.hash, 6, 100).unwrap(), b"asset");
    assert_eq!(library.read(&large.object.hash, 1024 * 1024 - 3, 10).unwrap(), big[1024 * 1024 - 3..1024 * 1024 + 7]);
    assert_eq!(library.chunks(&large.object.hash).unwrap().len(), 4);
    // Putting what is present writes nothing new and leases it again.
    let again = library.put(&mut big.as_slice(), "drill", LEASE).unwrap();
    assert_eq!(again.object, large.object);
    assert_ne!(again.id, large.id);
    // Nothing is collected while leased or within the grace.
    assert_eq!(library.collect().unwrap().objects, 0);
    for lease in [small, large, again] {
        library.release(lease).unwrap();
    }
    assert_eq!(library.collect().unwrap().objects, 0, "within the grace");
    // Past the grace, both go: rows, bytes and the file.
    let db = Connection::open(dir.join("assets.sqlite3")).unwrap();
    db.execute("UPDATE objects SET placed = placed - 90000000", []).unwrap();
    let collected = library.collect().unwrap();
    assert_eq!(collected.objects, 2, "{collected:?}");
    assert_eq!(library.size(&large.object.hash).unwrap(), None);
    let hex: String = large.object.hash.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!dir.join("asset-objects").join(&hex[..2]).join(format!("{hex}.1")).exists());
    // Lives are numbered across every object and never given again: a hash
    // put back after it was collected, or while a tombstone of it stands,
    // is a life no deleter of an old one can meet (I5, R01).
    let first = library.put(&mut big.as_slice(), "drill", LEASE).unwrap();
    library.release(first).unwrap();
    let was: i64 = db.query_row("SELECT life FROM objects WHERE hash = ?1 AND state = 1", params![&first.object.hash[..]], |r| r.get(0)).unwrap();
    assert!(was > 1, "past the lives collected before: {was}");
    db.execute("UPDATE objects SET state = 2 WHERE hash = ?1", params![&first.object.hash[..]]).unwrap();
    let reborn = library.put(&mut big.as_slice(), "drill", LEASE).unwrap();
    let life: i64 = db.query_row("SELECT life FROM objects WHERE hash = ?1 AND state = 1", params![&reborn.object.hash[..]], |r| r.get(0)).unwrap();
    assert!(life > was);
    assert!(dir.join("asset-objects").join(&hex[..2]).join(format!("{hex}.{was}")).exists(), "the tombstone's file is its collector's");
    drop(library);
    // What a stop leaves: a tombstone whose file is still there, an orphan
    // file, an incoming part, and a published file the power took.
    let other: Vec<u8> = vec![7u8; 200 * 1024];
    let (library, _) = Library::open(&dir).unwrap();
    let doomed = library.put(&mut other.as_slice(), "drill", LEASE).unwrap();
    library.release(doomed).unwrap();
    drop(library);
    let dhex: String = doomed.object.hash.iter().map(|b| format!("{b:02x}")).collect();
    let dlife: i64 = db.query_row("SELECT life FROM objects WHERE hash = ?1", params![&doomed.object.hash[..]], |r| r.get(0)).unwrap();
    db.execute("UPDATE objects SET state = 2 WHERE hash = ?1", params![&doomed.object.hash[..]]).unwrap();
    let bucket = dir.join("asset-objects").join(&hex[..2]);
    std::fs::write(bucket.join(format!("{hex}.9")), b"orphan").unwrap();
    std::fs::write(dir.join("asset-objects").join("incoming").join("put-1-1.part"), b"half").unwrap();
    std::fs::remove_file(bucket.join(format!("{hex}.{life}"))).unwrap();
    let (library, started) = Library::open(&dir).unwrap();
    println!("started: {started:?}");
    assert_eq!(started.tombstones, 1);
    assert_eq!(started.lost, 1);
    assert_eq!(started.orphans, 1);
    assert_eq!(started.incoming, 1);
    assert!(!dir.join("asset-objects").join(&dhex[..2]).join(format!("{dhex}.{dlife}")).exists());
    let state: i64 = db.query_row("SELECT state FROM objects WHERE hash = ?1 AND life = ?2", params![&reborn.object.hash[..], life], |r| r.get(0)).unwrap();
    assert_eq!(state, 0, "lost file goes back to preparing");
    let wanted: i64 = db.query_row("SELECT COUNT(*) FROM wanted WHERE hash = ?1", params![&reborn.object.hash[..]], |r| r.get(0)).unwrap();
    assert_eq!(wanted, 1);
    let _ = library;
    let _ = Duration::ZERO;
    println!("v2 objects drill: ok");
    let _ = std::fs::remove_dir_all(&dir);
}
