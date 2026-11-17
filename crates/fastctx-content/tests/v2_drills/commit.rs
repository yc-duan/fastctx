//! A drill of v2 commits.
use fastctx_content::v2::{Commit, Header, KindRecord, LEASE, Library, Signer, open};
use fastctx_content::{ObjectRef, Record, State};
use fastctx_storage::rusqlite::{Connection, params};
use std::sync::Arc;

/// A frame row as read: header, signature, body and objects.
type FrameRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.iter().rev().copied().collect() }
}

fn rec(item: &str, state: State) -> KindRecord {
    KindRecord { kind: 7, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state } }
}

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-commit-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (library, _) = Library::open(&dir).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    library.provisional("scope-a").unwrap();
    let lease = library.put(&mut vec![9u8; 100 * 1024].as_slice(), "drill", LEASE).unwrap();
    let object: ObjectRef = lease.object;
    // Many records span two frames by count.
    let mut records: Vec<KindRecord> = (0..5000).map(|i| rec(&format!("item-{i:05}"), State::Is { body: vec![b'x'; 10], objects: vec![] })).collect();
    records[3] = rec("item-00003", State::Is { body: b"with object".to_vec(), objects: vec![object] });
    let committed = library.raw_commit(Commit { scope: "scope-a".into(), stream: "timeline".into(), records, leases: vec![lease], durable: true, op: None, holding: library.holding("scope-a").unwrap().unwrap_or(0) }, signer.clone()).unwrap();
    assert_eq!((committed.first, committed.count, committed.frames), (0, 5000, 2));
    let db = Connection::open(dir.join("assets.sqlite3")).unwrap();
    let leases: i64 = db.query_row("SELECT COUNT(*) FROM leases", [], |r| r.get(0)).unwrap();
    assert_eq!(leases, 0, "the commit let go of its lease");
    let refs: i64 = db.query_row("SELECT count FROM object_refs WHERE hash = ?1", params![&object.hash[..]], |r| r.get(0)).unwrap();
    assert_eq!(refs, 1);
    let held: i64 = db.query_row("SELECT COUNT(*) FROM frame_objects WHERE hash = ?1", params![&object.hash[..]], |r| r.get(0)).unwrap();
    assert_eq!(held, 1);
    // Frames decode, chain and carry their signature.
    let mut previous = [0u8; 32];
    let mut stmt = db.prepare("SELECT header, signature, body, head FROM frames ORDER BY first").unwrap();
    let frames: Vec<FrameRow> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(|r| r.unwrap()).collect();
    let mut total = 0;
    for (header, signature, body, head) in &frames {
        let parsed = Header::decode(header).unwrap();
        assert_eq!(parsed.previous, previous);
        assert_eq!(signature, &header.iter().rev().copied().collect::<Vec<_>>());
        assert_eq!(parsed.chain().to_vec(), *head);
        total += open(&parsed, body).unwrap().len();
        previous = parsed.chain();
    }
    assert_eq!(total, 5000);
    assert_eq!(committed.head, previous);
    // The record with the object is replaced by a later revision without it, then deleted.
    library.raw_commit(Commit { scope: "scope-a".into(), stream: "timeline".into(), records: vec![rec("item-00003", State::Is { body: b"no object".to_vec(), objects: vec![] })], leases: vec![], durable: false, op: None, holding: library.holding("scope-a").unwrap().unwrap_or(0) }, signer.clone()).unwrap();
    let refs: i64 = db.query_row("SELECT COUNT(*) FROM object_refs WHERE hash = ?1", params![&object.hash[..]], |r| r.get(0)).unwrap();
    assert_eq!(refs, 0, "no index row refers to it");
    let unreferenced: Option<i64> = db.query_row("SELECT unreferenced FROM objects WHERE hash = ?1", params![&object.hash[..]], |r| r.get(0)).unwrap();
    assert!(unreferenced.is_some());
    library.raw_commit(Commit { scope: "scope-a".into(), stream: "timeline".into(), records: vec![rec("item-00003", State::Gone)], leases: vec![], durable: true, op: None, holding: library.holding("scope-a").unwrap().unwrap_or(0) }, signer.clone()).unwrap();
    let (state, revision): (i64, i64) = db.query_row("SELECT state, revision FROM items WHERE item = ?1", params![b"item-00003".to_vec()], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((state, revision), (1, 5001), "a tombstone at its revision");
    // A record referring to an object that is not here is refused (I3).
    let missing = ObjectRef { hash: [5; 32], size: 3 };
    let refused = library.raw_commit(Commit { scope: "scope-a".into(), stream: "timeline".into(), records: vec![rec("x", State::Is { body: vec![], objects: vec![missing] })], leases: vec![], durable: true, op: None, holding: library.holding("scope-a").unwrap().unwrap_or(0) }, signer.clone());
    assert!(refused.is_err());
    // Installing the provisional scope keeps its period; revoking it ends the period and purges.
    let holding = library.install("scope-a", b"desc".to_vec(), 1).unwrap();
    assert_eq!(holding, 1);
    library.revoke("scope-a").unwrap();
    assert!(library.raw_commit(Commit { scope: "scope-a".into(), stream: "timeline".into(), records: vec![rec("y", State::Gone)], leases: vec![], durable: true, op: None, holding: library.holding("scope-a").unwrap().unwrap_or(0) }, signer.clone()).is_err(), "no writes after revocation");
    let mut passes = 0;
    loop {
        let purged = library.purge().unwrap();
        passes += 1;
        if purged.finished > 0 { break; }
        assert!(passes < 20);
    }
    let left: i64 = db.query_row("SELECT (SELECT COUNT(*) FROM items) + (SELECT COUNT(*) FROM frames) + (SELECT COUNT(*) FROM frame_objects) + (SELECT COUNT(*) FROM object_refs)", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0, "the ended period's rows are gone");
    // Granted again: a new period.
    assert_eq!(library.install("scope-a", b"desc".to_vec(), 2).unwrap(), 2);
    println!("v2 commit drill: ok ({passes} purge passes)");
    let _ = std::fs::remove_dir_all(&dir);
}
