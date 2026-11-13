//! A drill of frames set aside as evidence (aside.rs, fff AT02/AT03; design §29 4, "一帧一对象的旁证
// 保活"): a replica's frames past where a put-back hub stands are kept whole with the one object they refer to,
// however collection runs, and go with their holding period; not kept.
use fastctx_content::v2::{Commit, EpochFact, EpochKind, Header, Holder, HubVerifier, Imported, KindRecord, LEASE, Library, Signed, Signer, Verifier};
use fastctx_content::{ObjectRef, Record, State};
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
impl Verifier for Fake {
    fn verify(&self, _: &Header, bytes: &[u8], signature: &[u8]) -> bool { bytes == signature }
}
struct AnyHub;
impl HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0() -> Signed {
    let fact = EpochFact { scope: "s".into(), stream: "t".into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: EpochKind::Source, predecessor: None, follows: [0; 32] };
    Signed { fact: fact.encode(), signature: vec![] }
}
fn rec(item: &str, objects: Vec<ObjectRef>) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.into(), order: item.into(), state: State::Is { body: item.into(), objects } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-aside-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "d"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (p, _) = Library::open(&base.join("p")).unwrap();
    let (d, _) = Library::open(&base.join("d")).unwrap();
    for lib in [&p, &d] { lib.install("s", b"d".to_vec(), 1).unwrap(); }
    d.accept_epoch("s", epoch0(), &AnyHub).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let bytes = vec![7u8; 100 * 1024];
    let lease = p.put(&mut bytes.as_slice(), "p", LEASE).unwrap();
    let object = lease.object;
    let commit = |records: Vec<KindRecord>, leases| {
        let holding = p.holding("s").unwrap().unwrap();
        p.raw_commit(Commit { scope: "s".into(), stream: "t".into(), records, leases, durable: true, op: None, holding }, signer.clone()).unwrap();
    };
    commit(vec![rec("a", vec![])], vec![]);
    commit(vec![rec("b", vec![object])], vec![lease]);
    commit(vec![rec("c", vec![])], vec![]);
    // The replica fetched the object as it came, and holds the three frames.
    let fetched = d.put(&mut bytes.as_slice(), "fetch", LEASE).unwrap();
    let holding = d.holding("s").unwrap().unwrap();
    for frame in p.export("s", "t", 0, 10).unwrap() {
        let got = d.import("s", frame, &Fake, Holder::Replica, holding).unwrap();
        assert!(matches!(got, Imported::Applied { .. }), "{got:?}");
    }
    d.release(fetched).unwrap();
    assert!(d.item("s", "t", b"b").unwrap().is_some_and(|e| e.objects == vec![object]));
    // The hub, put back, stands after the first frame: the replica sets aside what is past it.
    assert_eq!(d.set_aside("s", "t", 1, "the hub was put back").unwrap(), 1, "the stream goes on from where the hub stands");
    assert!(d.item("s", "t", b"a").unwrap().is_some(), "what the hub holds stays");
    assert!(d.item("s", "t", b"b").unwrap().is_none() && d.item("s", "t", b"c").unwrap().is_none(), "cut back before the frame set aside");
    let db = fastctx_storage::rusqlite::Connection::open(base.join("d").join("assets.sqlite3")).unwrap();
    let evidence: i64 = db.query_row("SELECT COUNT(*) FROM evidence", [], |row| row.get(0)).unwrap();
    assert_eq!(evidence, 2, "both frames past the hub's place, whole");
    // Nothing in the stream refers to the object now and no lease holds it; collection without the grace keeps it
    // for the evidence.
    let first = d.collect_now().unwrap();
    assert_eq!(first.objects, 0, "{first:?}");
    assert_eq!(d.size(&object.hash).unwrap(), Some(object.size), "the evidence's one object kept");
    let read = d.read(&object.hash, 0, 1 << 20).unwrap();
    assert_eq!(read, bytes, "whole");
    // Its holding period let go: the evidence goes, and the object with it.
    d.revoke("s").unwrap();
    while d.purging().unwrap() { d.purge().unwrap(); }
    let after: i64 = db.query_row("SELECT COUNT(*) FROM evidence", [], |row| row.get(0)).unwrap();
    assert_eq!(after, 0, "gone with its period");
    let second = d.collect_now().unwrap();
    assert_eq!(second.objects, 1, "{second:?}");
    assert_eq!(d.size(&object.hash).unwrap(), None);
    drop(db);
    let _ = std::fs::remove_dir_all(&base);
    println!("v2 aside drill: ok");
}
