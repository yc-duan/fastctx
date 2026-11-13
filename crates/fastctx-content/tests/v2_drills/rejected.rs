//! A drill of the producer's side of a rejected frame.
use fastctx_content::v2::{Commit, KindRecord, LEASE, Library, Op, Signer};
use fastctx_content::{Record, State};
use fastctx_storage::rusqlite::{Connection, params};
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
fn rec(item: &str, state: State) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state } }
}
fn count(db: &Connection, sql: &str) -> i64 { db.query_row(sql, [], |r| r.get(0)).unwrap() }

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-rejected-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (library, _) = Library::open(&dir).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    library.install("s", b"d".to_vec(), 1).unwrap();
    let attachment = library.put(&mut vec![6u8; 80 * 1024].as_slice(), "drill", LEASE).unwrap();
    let words = Op { id: "dev:7".into(), digest: [7; 32], kind: "talk".into() };
    let c = |records, leases, op| Commit { scope: "s".into(), stream: "t".into(), records, leases, durable: true, op, holding: library.holding("s").unwrap().unwrap_or(0) };
    library.raw_commit(c(vec![rec("a", State::Is { body: b"a".to_vec(), objects: vec![] })], vec![], None), signer.clone()).unwrap();
    let rejected_at = library.raw_commit(c(vec![rec("b", State::Is { body: b"b".to_vec(), objects: vec![attachment.object] })], vec![attachment], Some((words.clone(), true))), signer.clone()).unwrap().first;
    library.raw_commit(c(vec![rec("c", State::Is { body: b"c".to_vec(), objects: vec![] })], vec![], None), signer.clone()).unwrap();
    let db = Connection::open(dir.join("assets.sqlite3")).unwrap();
    // The hub rejects the frame at b: b and c leave the stream, kept to commit again.
    assert_eq!(library.rejected("s", "t", rejected_at).unwrap(), 2);
    assert!(library.item("s", "t", b"b").unwrap().is_none() && library.item("s", "t", b"c").unwrap().is_none());
    assert!(library.item("s", "t", b"a").unwrap().is_some(), "the frames before stay applied");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM frames"), 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM object_holds WHERE kind = 'resubmit'"), 1);
    let waiting = library.resubmits("s", "t", 1000).unwrap();
    assert_eq!(waiting.len(), 2);
    assert_eq!(waiting[0].op.as_deref(), Some("dev:7"));
    // Past the grace the attachment, referred to by nothing but the hold, stays (I2).
    db.execute("UPDATE objects SET placed = placed - 90000000, unreferenced = unreferenced - 90000000", []).unwrap();
    assert_eq!(library.collect().unwrap().objects, 0);
    assert!(library.size(&attachment.object.hash).unwrap().is_some());
    // A new epoch; b is committed again there, c is refused (a projection: it goes).
    library.new_epoch("s", "t", 1, b"epoch-1".to_vec()).unwrap();
    assert_eq!(library.resubmit("s", "t", 1, signer.clone()).unwrap(), 1);
    let b = library.item("s", "t", b"b").unwrap().unwrap();
    assert_eq!(b.revision, rejected_at, "it lands where the rejected frame stood, in the new epoch");
    let epoch: i64 = db.query_row("SELECT epoch FROM frames WHERE first = ?1", params![rejected_at as i64], |r| r.get(0)).unwrap();
    assert_eq!(epoch, 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM object_holds WHERE kind = 'resubmit'"), 1, "c still waits in the same run");
    let c_waiting = library.resubmits("s", "t", 1000).unwrap().remove(0);
    library.refuse("s", "t", c_waiting.id, "not writable now").unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM object_holds"), 0, "the run is done: its holds go");
    assert!(library.resubmits("s", "t", 1000).unwrap().is_empty());
    assert_eq!(library.collect().unwrap().objects, 0, "the new frame refers to the attachment");
    // A true source's write that cannot be written keeps its object until the user drops it.
    let second = library.put(&mut vec![5u8; 90 * 1024].as_slice(), "drill", LEASE).unwrap();
    let words2 = Op { id: "dev:8".into(), digest: [8; 32], kind: "talk".into() };
    let at = library.raw_commit(c(vec![rec("d", State::Is { body: b"d".to_vec(), objects: vec![second.object] })], vec![second], Some((words2, true))), signer.clone()).unwrap().first;
    library.rejected("s", "t", at).unwrap();
    let d = library.resubmits("s", "t", 1000).unwrap().remove(0);
    library.refuse("s", "t", d.id, "not writable now").unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM object_holds WHERE kind = 'refused' AND holder = 'dev:8'"), 1);
    db.execute("UPDATE objects SET placed = placed - 90000000, unreferenced = unreferenced - 90000000", []).unwrap();
    assert_eq!(library.collect().unwrap().objects, 0, "held for the user");
    library.dropped("dev:8").unwrap();
    db.execute("UPDATE objects SET placed = placed - 90000000, unreferenced = unreferenced - 90000000", []).unwrap();
    assert_eq!(library.collect().unwrap().objects, 1, "dropped: collected after its grace");
    println!("v2 rejected drill: ok");
    let _ = std::fs::remove_dir_all(&dir);
}
