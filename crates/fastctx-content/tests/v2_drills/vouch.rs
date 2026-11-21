//! A drill of a hub letting go of its copy of a large object the device
//! ordering a conversation's stream vouched for: kept unvouched, vouched
//! under another key, read back too long ago or before its device came to
//! the network, told anew without it, longer than the machines take from one
//! another, or leased; let go once vouched for by the device ordering the
//! stream now; put again after, taken as new.
use fastctx_content::v2::{
    Able, Authorizer, Commit, Entry, Header, HubSigner, KindRecord, LEASE, Library, Received, Settled, Signer, Taking, VOUCHED_FOR, Verifier,
};
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
    // Every voucher may hold its scope, on the network since `since`.
    let able = |since: i64| -> Able { hub.vouchers(None, 100).unwrap().into_iter().map(|voucher| (voucher, since)).collect() };
    let everyone = || able(0);
    let count = |longest: u64| hub.reclaim_vouched(None, 100, longest, false, everyone()).unwrap().1;
    // Read back whole where it was made.
    assert!(producer.verify_object(&blob.object.hash).unwrap(), "whole");
    assert!(!producer.verify_object(&[3; 32]).unwrap(), "not here");
    // Leased as it was put: kept.
    assert_eq!(count(longest).objects, 0);
    assert!(count(longest).kept.keys().any(|why| why.contains("sent or fetched")), "{:?}", count(longest));
    hub.release(lease).unwrap();
    // Not vouched for: kept.
    assert!(count(longest).kept.keys().any(|why| why.contains("not vouched")), "{:?}", count(longest));
    // Vouched for under another key, or in another epoch: not taken.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &[9; 32], &[blob.object.hash], 0, false).unwrap(), None);
    assert_eq!(hub.vouch(scope, "timeline", epoch + 1, &key, &[blob.object.hash], 0, false).unwrap(), None);
    assert!(count(longest).kept.keys().any(|why| why.contains("not vouched")), "{:?}", count(longest));
    // An object no frame of the stream refers to is passed by.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[[7; 32]], 0, false).unwrap(), Some(0));
    // Read back longer ago than a vouch counts: kept.
    let long_ago = VOUCHED_FOR.as_millis() as u64 + 60_000;
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[blob.object.hash], long_ago, false).unwrap(), Some(1));
    assert!(count(longest).kept.keys().any(|why| why.contains("too long ago")), "{:?}", count(longest));
    // Told anew without it — not read back whole there any more: kept.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[blob.object.hash], 0, false).unwrap(), Some(1));
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[], 0, true).unwrap(), Some(0));
    assert!(count(longest).kept.keys().any(|why| why.contains("not vouched")), "{:?}", count(longest));
    // Vouched for by the device ordering it now: may go, but for one longer
    // than the machines take from one another, where that device may not
    // hold its conversation now, or came to the network after it read it
    // back.
    assert_eq!(hub.vouch(scope, "timeline", epoch, &key, &[blob.object.hash], 0, true).unwrap(), Some(1));
    assert!(count(50 * 1024).kept.keys().any(|why| why.contains("longer")), "{:?}", count(50 * 1024));
    let nobody = hub.reclaim_vouched(None, 100, longest, false, Default::default()).unwrap().1;
    assert!(nobody.kept.keys().any(|why| why.contains("may not hold")), "{nobody:?}");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let later = now + 60 * 60 * 1000;
    let came_later = hub.reclaim_vouched(None, 100, longest, false, able(later)).unwrap().1;
    assert!(came_later.kept.keys().any(|why| why.contains("came to the network")), "{came_later:?}");
    let counted = count(longest);
    assert_eq!((counted.objects, counted.bytes), (1, bytes.len() as u64), "{counted:?}");
    assert!(hub.size(&blob.object.hash).unwrap().is_some(), "counted, not let go");
    let (next, gone) = hub.reclaim_vouched(None, 100, longest, true, everyone()).unwrap();
    assert_eq!((next, gone.objects, gone.freed, gone.stuck), (None, 1, bytes.len() as u64, 0));
    assert!(hub.size(&blob.object.hash).unwrap().is_none(), "let go");
    assert_eq!(hub.settle_tombstones(None, 10).unwrap(), Settled::default(), "no tombstone left");
    assert!(hub.item(scope, "timeline", b"a").unwrap().is_some(), "its frame stays");
    // Put again, it is taken as new.
    let again = hub.put(&mut bytes.as_slice(), "again", LEASE).unwrap();
    assert_eq!(again.object, blob.object);
    assert_eq!(hub.size(&blob.object.hash).unwrap(), Some(bytes.len() as u64));
    println!("v2 vouch drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
