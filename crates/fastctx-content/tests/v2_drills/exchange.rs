//! A drill of v2 export/import and cross-period rules.
use fastctx_content::v2::{Commit, Header, Holder, Imported, KindRecord, LEASE, Library, Signer, Verifier};
use fastctx_content::{Record, State};
use fastctx_storage::rusqlite::{Connection, params};
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.iter().rev().copied().collect() }
}
impl Verifier for Fake {
    fn verify(&self, _: &Header, bytes: &[u8], signature: &[u8]) -> bool { signature == bytes.iter().rev().copied().collect::<Vec<_>>() }
}
fn rec(item: &str, state: State) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state } }
}
fn row(db: &Connection, item: &str) -> Option<(i64, i64, i64)> {
    db.query_row("SELECT state, revision, holding FROM items WHERE item = ?1", params![item.as_bytes().to_vec()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok()
}

struct AnyHub;
impl fastctx_content::v2::HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0(scope: &str, stream: &str) -> fastctx_content::v2::Signed {
    let fact = fastctx_content::v2::EpochFact { scope: scope.into(), stream: stream.into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: fastctx_content::v2::EpochKind::Source, predecessor: None, follows: [0; 32] };
    fastctx_content::v2::Signed { fact: fact.encode(), signature: vec![] }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-exchange-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (pdir, rdir) = (base.join("producer"), base.join("replica"));
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::create_dir_all(&rdir).unwrap();
    let (producer, _) = Library::open(&pdir).unwrap();
    let (replica, _) = Library::open(&rdir).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    producer.install("s", b"d".to_vec(), 1).unwrap();
    let lease = producer.put(&mut vec![3u8; 90 * 1024].as_slice(), "drill", LEASE).unwrap();
    for (records, leases) in [
        (vec![rec("x", State::Is { body: b"one".to_vec(), objects: vec![lease.object] }), rec("y", State::Is { body: b"y".to_vec(), objects: vec![] })], vec![lease]),
        (vec![rec("x", State::Gone)], vec![]),
    ] {
        producer.raw_commit(Commit { scope: "s".into(), stream: "t".into(), records, leases, durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) }, signer.clone()).unwrap();
    }
    let frames = producer.export("s", "t", 0, 10).unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].objects, vec![lease.object.hash]);
    // Replica, period 1: gap first, then in order; same frame again is Already; a missing object is wanted.
    replica.install("s", b"d".to_vec(), 1).unwrap();
    replica.accept_epoch("s", epoch0("s", "t"), &AnyHub).unwrap();
    assert_eq!(replica.import("s", frames[1].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Gap { from: 0, to: 1 });
    assert!(matches!(replica.import("s", frames[0].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Applied { next: 2, .. }));
    assert_eq!(replica.import("s", frames[0].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Already);
    assert!(matches!(replica.import("s", frames[1].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Applied { next: 3, .. }));
    let db = Connection::open(rdir.join("assets.sqlite3")).unwrap();
    let wanted: i64 = db.query_row("SELECT COUNT(*) FROM wanted", [], |r| r.get(0)).unwrap();
    assert_eq!(wanted, 1);
    assert_eq!(replica.wanted_of(&["s".to_string()], 10, &[]).unwrap().len(), 1, "wanted through the scope that refers to it");
    assert!(replica.wanted_of(&["elsewhere".to_string()], 10, &[]).unwrap().is_empty());
    assert_eq!(row(&db, "x"), Some((1, 2, 1)), "x is a tombstone at revision 2 in period 1");
    // A hub requires the object.
    let (hub, _) = Library::open(&base.join("hub")).unwrap();
    hub.install("s", b"d".to_vec(), 1).unwrap();
    hub.accept_epoch("s", epoch0("s", "t"), &AnyHub).unwrap();
    assert!(hub.import("s", frames[0].clone(), &Fake, Holder::Hub, hub.holding("s").unwrap().unwrap_or(0)).is_err());
    // A tampered frame does not verify; another frame at a held position is a conflict.
    let mut tampered = frames[0].clone();
    tampered.signature[0] ^= 1;
    assert!(replica.import("s", tampered, &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).is_err());
    // Revoked and granted again before the purge: period 2 refills from the start.
    replica.revoke("s").unwrap();
    assert!(replica.wanted_of(&["s".to_string()], 10, &[]).unwrap().is_empty(), "not once revoked");
    assert_eq!(replica.install("s", b"d".to_vec(), 2).unwrap(), 2);
    assert!(matches!(replica.import("s", frames[0].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Applied { next: 2, .. }));
    assert_eq!(row(&db, "x"), Some((1, 2, 1)), "the older revision does not bring x back over period 1's tombstone");
    assert_eq!(replica.wanted_of(&["s".to_string()], 10, &[]).unwrap().len(), 1, "wanted again in period 2");
    assert_eq!(row(&db, "y"), Some((0, 1, 2)), "y moved into period 2 at its own revision");
    // The purge of period 1 runs between period 2's frames: its tombstone becomes a floor.
    loop { if replica.purge().unwrap().finished > 0 { break; } }
    assert_eq!(row(&db, "x"), Some((2, 2, 1)), "a revision floor guards x");
    assert!(matches!(replica.import("s", frames[1].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Applied { next: 3, .. }));
    assert_eq!(row(&db, "x"), Some((1, 2, 2)), "period 2's tombstone replaced the floor");
    let floors: i64 = db.query_row("SELECT COUNT(*) FROM items WHERE state = 2", [], |r| r.get(0)).unwrap();
    assert_eq!(floors, 0);
    println!("v2 exchange drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
