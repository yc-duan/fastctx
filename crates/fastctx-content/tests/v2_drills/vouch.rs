//! A drill of a hub letting go of its copy of a large object the device
//! ordering a conversation's stream vouched for: kept unvouched, vouched
//! under another key, longer than the machines take from one another, or
//! leased; let go once vouched for by the device ordering the stream now;
//! put again after, taken as new.
use fastctx_content::v2::{Authorizer, Commit, Entry, Header, HubSigner, KindRecord, LEASE, Library, Received, Signer, Taking, Verifier};
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
struct Anything;
impl Authorizer for Anything {
    fn authorize(&self, _frame: &Header, _record: &KindRecord, _before: Option<&Entry>) -> Result<(), String> { Ok(()) }
}
fn rec(item: &str, objects: Vec<ObjectRef>) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-vouch-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let scope = "c/h/m/conv";
    producer.install(scope, b"d".to_vec(), 1).unwrap();
    hub.install(scope, b"d".to_vec(), 1).unwrap();
    let bytes = vec![8u8; 100 * 1024];
    let blob = producer.put(&mut bytes.as_slice(), "p", LEASE).unwrap();
    let commit = Commit { scope: scope.into(), stream: "timeline".into(), records: vec![rec("a", vec![blob.object])], leases: vec![], durable: true, op: None, holding: producer.holding(scope).unwrap().unwrap_or(0) };
    producer.raw_commit(commit, signer.clone()).unwrap();
    let frames = producer.export(scope, "timeline", 0, 10).unwrap();
    let anyone = |_: &ObjectRef| true;
    let open = Taking { period: hub.holding(scope).unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let lease = hub.put(&mut bytes.as_slice(), "hub", LEASE).unwrap();
    let taken = hub.take(scope, frames[0].clone(), &Fake, &open, Arc::new(Anything), Arc::new(Fake)).unwrap();
    assert!(matches!(taken, Received::Applied { .. }), "{taken:?}");
    let (epoch, key) = hub.last_epoch(scope, "timeline").unwrap().expect("its first epoch, the device's");
    let longest = 64 << 20;
    let count = |longest: u64| hub.reclaim_vouched(None, 100, longest, false).unwrap().1;
    // Leased as it was put: kept.
    assert_eq!(count(longest).objects, 0);
    assert!(count(longest).kept.keys().any(|why| why.contains("sent or fetched")), "{:?}", count(longest));
    hub.release(lease).unwrap();
    // Not vouched for: kept.
    assert!(count(longest).kept.keys().any(|why| why.contains("not vouched")), "{:?}", count(longest));
    // Vouched for under another key: kept.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &[9; 32], &[blob.object.hash]).unwrap(), 1);
    assert!(count(longest).kept.keys().any(|why| why.contains("not the stream's now")), "{:?}", count(longest));
    // An object no frame of the stream refers to is passed by.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[[7; 32]]).unwrap(), 0);
    // Vouched for by the device ordering it now: may go, but for one longer
    // than the machines take from one another.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[blob.object.hash]).unwrap(), 1);
    assert!(count(50 * 1024).kept.keys().any(|why| why.contains("longer")), "{:?}", count(50 * 1024));
    let counted = count(longest);
    assert_eq!((counted.objects, counted.bytes), (1, bytes.len() as u64), "{counted:?}");
    assert!(hub.size(&blob.object.hash).unwrap().is_some(), "counted, not let go");
    let (next, gone) = hub.reclaim_vouched(None, 100, longest, true).unwrap();
    assert_eq!((next, gone.objects), (None, 1));
    assert!(hub.size(&blob.object.hash).unwrap().is_none(), "let go");
    assert!(hub.item(scope, "timeline", b"a").unwrap().is_some(), "its frame stays");
    // Put again, it is taken as new.
    let again = hub.put(&mut bytes.as_slice(), "again", LEASE).unwrap();
    assert_eq!(again.object, blob.object);
    assert_eq!(hub.size(&blob.object.hash).unwrap(), Some(bytes.len() as u64));
    println!("v2 vouch drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
