//! A drill of a hub's owner library taking frames.
use fastctx_content::v2::{Authorizer, Commit, Cutoff, Entry, EpochKind, Header, Successor, HubSigner, KindRecord, LEASE, Library, Received, Signer, Taking, Verifier};
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
impl HubSigner for Fake {
    fn sign(&self, statement: &[u8]) -> Vec<u8> { statement[..8].to_vec() }
}
struct NoDelete;
impl Authorizer for NoDelete {
    fn authorize(&self, _frame: &fastctx_content::v2::Header, record: &KindRecord, before: Option<&Entry>) -> Result<(), String> {
        if record.record.item == b"locked" && before.is_some() { return Err("locked items do not change".into()); }
        Ok(())
    }
}
fn rec(item: &str, objects: Vec<ObjectRef>) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-hub-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let auth: Arc<dyn Authorizer> = Arc::new(NoDelete);
    let hub_signer: Arc<dyn HubSigner> = Arc::new(Fake);
    producer.install("s", b"d".to_vec(), 1).unwrap();
    hub.install("s", b"d".to_vec(), 1).unwrap();
    let blob = producer.put(&mut vec![8u8; 100 * 1024].as_slice(), "p", LEASE).unwrap();
    let c = |records| Commit { scope: "s".into(), stream: "t".into(), records, leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) };
    producer.raw_commit(c(vec![rec("a", vec![]), rec("locked", vec![])]), signer.clone()).unwrap();
    producer.raw_commit(c(vec![rec("b", vec![blob.object])]), signer.clone()).unwrap();
    let frames = producer.export("s", "t", 0, 10).unwrap();
    let anyone = |_: &ObjectRef| true;
    let nobody = |_: &ObjectRef| false;
    let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let take = |frame, holding: &Taking<'_>| hub.take("s", frame, &Fake, holding, auth.clone(), hub_signer.clone()).unwrap();
    // Out of order: a gap; then in order with change numbers.
    assert_eq!(take(frames[1].clone(), &open), Received::Missing(vec![blob.object]), "the object is put first");
    let hub_blob = hub.put(&mut vec![8u8; 100 * 1024].as_slice(), "hub", LEASE).unwrap();
    assert!(matches!(take(frames[1].clone(), &open), Received::Gap { from: 0, to: 1 }));
    let first = take(frames[0].clone(), &open);
    assert!(matches!(first, Received::Applied { change: 1, .. }), "{first:?}");
    // The object is present but the source could not read it and proved nothing.
    let closed = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &nobody, proven: &|_: &[u8; 32]| false };
    assert_eq!(take(frames[1].clone(), &closed), Received::Prove(vec![blob.object]));
    let proven_hash = blob.object.hash;
    let proven = move |hash: &[u8; 32]| *hash == proven_hash;
    let proving = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &nobody, proven: &proven };
    let second = take(frames[1].clone(), &proving);
    assert!(matches!(second, Received::Applied { change: 2, .. }), "{second:?}");
    assert_eq!(take(frames[0].clone(), &open), first, "the same frame is answered as before");
    assert!(hub.item("s", "t", b"b").unwrap().is_some());
    // A successor epoch's frames before the predecessor's tail is in: parked, unread.
    let c2 = |records| Commit { scope: "s".into(), stream: "t2".into(), records, leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) };
    producer.raw_commit(c2(vec![rec("locked", vec![])]), signer.clone()).unwrap();
    let tail = producer.export("s", "t2", 0, 1).unwrap().remove(0);
    producer.new_epoch("s", "t2", 1, b"e1".to_vec()).unwrap();
    producer.raw_commit(c2(vec![rec("c", vec![])]), signer.clone()).unwrap();
    producer.raw_commit(c2(vec![rec("locked", vec![])]), signer.clone()).unwrap();
    producer.raw_commit(c2(vec![rec("d", vec![hub_blob.object])]), signer.clone()).unwrap();
    let later = producer.export("s", "t2", 1, 10).unwrap();
    assert_eq!(later.len(), 3);
    let cut = Cutoff { next: 1, head: Header::decode(&tail.header).unwrap().chain() };
    let successor = Successor { device: [1; 32], key: [2; 32], kind: EpochKind::Source };
    hub.open_epoch("s", "t2", successor, hub_signer.clone()).unwrap();
    let (_, none) = hub.handover("s", "t2", successor, cut, auth.clone(), hub_signer.clone()).unwrap();
    assert_eq!(none.applied, 0, "the tail is not in yet");
    let parked = take(later[0].clone(), &open);
    assert!(matches!(parked, Received::Parked { sequence: 1, .. }), "{parked:?}");
    assert!(matches!(take(later[1].clone(), &open), Received::Parked { sequence: 2, .. }));
    assert!(matches!(take(later[2].clone(), &open), Received::Parked { sequence: 3, .. }));
    assert!(hub.item("s", "t2", b"c").unwrap().is_none(), "parked frames are not read");
    // Parked objects hold their bytes past the grace.
    hub.release(hub_blob).unwrap();
    let db = fastctx_storage::rusqlite::Connection::open(base.join("h").join("assets.sqlite3")).unwrap();
    db.execute("UPDATE objects SET placed = placed - 90000000", []).unwrap();
    assert_eq!(hub.collect().unwrap().objects, 0);
    // The tail comes: c applies, the change to `locked` is refused, and so is what follows it.
    let Received::Applied { drained, .. } = take(tail.clone(), &open) else { panic!("the tail applies") };
    assert_eq!(drained.applied, 1, "c applied, the locked change refused and the run ends");
    assert_eq!(drained.refused.map(|r| r.header), Some(later[1].header.clone()));
    assert!(hub.item("s", "t2", b"c").unwrap().is_some());
    assert!(hub.item("s", "t2", b"d").unwrap().is_none());
    assert!(matches!(take(later[1].clone(), &open), Received::Rejected { .. }));
    assert!(matches!(take(later[2].clone(), &open), Received::Rejected { .. }), "later frames of the epoch are refused too");
    let prefixes: i64 = db.query_row("SELECT COUNT(DISTINCT prefix) FROM change_log", [], |r| r.get(0)).unwrap();
    assert_eq!(prefixes, 4);
    let history: i64 = db.query_row("SELECT COUNT(*) FROM parked_history", [], |r| r.get(0)).unwrap();
    assert_eq!(history, 3, "every parked accumulator is kept for good");
    // Several frames in one take (§18 E2): one that refers to an object one
    // before it does ends the take before it — the first one's keeping
    // consumes the object's proof — and, asked alone, it is judged afresh.
    let c3 = |records| Commit { scope: "s".into(), stream: "t3".into(), records, leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) };
    producer.raw_commit(c3(vec![rec("x1", vec![blob.object])]), signer.clone()).unwrap();
    producer.raw_commit(c3(vec![rec("x2", vec![blob.object])]), signer.clone()).unwrap();
    let both = producer.export("s", "t3", 0, 10).unwrap();
    let taken = hub.take_many("s", both.clone(), &Fake, &proving, auth.clone(), hub_signer.clone()).unwrap();
    assert!(matches!(taken[..], [Received::Applied { .. }]), "{taken:?}");
    assert!(hub.item("s", "t3", b"x2").unwrap().is_none(), "not taken with the first");
    assert_eq!(take(both[1].clone(), &closed), Received::Prove(vec![blob.object]), "alone, its proof let go, asked for one");
    // A known frame's objects count too.
    let again = hub.take_many("s", both.clone(), &Fake, &proving, auth.clone(), hub_signer.clone()).unwrap();
    assert_eq!(again, taken, "the first told as before, the second not taken with it");
    println!("v2 hub drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
