//! A drill of the change feed of a hub's owner library
// (design §28.1); not kept.
use fastctx_content::v2::{Authorizer, Commit, Entry, Header, HubSigner, KindRecord, Library, Received, Signer, Taking, Verifier};
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
struct All;
impl Authorizer for All {
    fn authorize(&self, _: &Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> { Ok(()) }
    fn knows(&self, _: u16, _: u16) -> bool { true }
}
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-changes-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let hub_signer: Arc<dyn HubSigner> = Arc::new(Fake);
    let anyone = |_: &ObjectRef| true;
    // An empty library: nothing changed, its generation made once.
    let empty = hub.changes(None, 0, None, 10).unwrap();
    assert!(empty.rows.is_empty() && empty.top == 0 && !empty.more);
    let generation = empty.generation.clone();
    assert_eq!(hub.changes(None, 0, None, 10).unwrap().generation, generation, "the same generation until renewed");
    // Three scopes, a stream each; the timeline one written last.
    for (scope, stream) in [("s1", "settings"), ("s2", "notes"), ("s3", "timeline")] {
        producer.install(scope, b"d".to_vec(), 1).unwrap();
        hub.install(scope, b"d".to_vec(), 1).unwrap();
        let holding = producer.holding(scope).unwrap().unwrap();
        producer.raw_commit(Commit { scope: scope.into(), stream: stream.into(), records: vec![rec("a")], leases: vec![], durable: true, op: None, holding }, signer.clone()).unwrap();
        let frames = producer.export(scope, stream, 0, 10).unwrap();
        let taking = Taking { period: hub.holding(scope).unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
        let taken = hub.take(scope, frames[0].clone(), &Fake, &taking, Arc::new(All), hub_signer.clone()).unwrap();
        assert!(matches!(taken, Received::Applied { .. }), "{taken:?}");
    }
    let all = hub.changes(None, 0, None, 10).unwrap();
    assert_eq!(all.top, 3);
    let order: Vec<_> = all.rows.iter().map(|row| (row.change, row.scope.as_str(), row.stream.as_str())).collect();
    assert_eq!(order, vec![(3, "s3", "timeline"), (2, "s2", "notes"), (1, "s1", "settings")], "newest first");
    assert!(all.rows.iter().all(|row| row.next == 1 && row.bytes > 0));
    // Paging down: a page of two, then the rest below where it stopped.
    let first = hub.changes(None, 0, None, 2).unwrap();
    assert!(first.more && first.rows.len() == 2 && first.through == 2);
    let rest = hub.changes(None, 0, Some(first.through), 2).unwrap();
    assert_eq!(rest.rows.iter().map(|row| row.change).collect::<Vec<_>>(), vec![1]);
    assert!(!rest.more);
    // After where a device stands only what changed since.
    assert!(hub.changes(None, 3, None, 10).unwrap().rows.is_empty());
    // One stream changes again: one row for it, at the new number; bytes add up.
    let before = all.rows.iter().find(|row| row.scope == "s1").unwrap().bytes;
    let holding = producer.holding("s1").unwrap().unwrap();
    producer.raw_commit(Commit { scope: "s1".into(), stream: "settings".into(), records: vec![rec("b")], leases: vec![], durable: true, op: None, holding }, signer.clone()).unwrap();
    let frames = producer.export("s1", "settings", 1, 10).unwrap();
    let taking = Taking { period: hub.holding("s1").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    hub.take("s1", frames[0].clone(), &Fake, &taking, Arc::new(All), hub_signer.clone()).unwrap();
    let since = hub.changes(None, 3, None, 10).unwrap();
    assert_eq!(since.rows.len(), 1);
    assert_eq!((since.rows[0].change, since.rows[0].scope.as_str(), since.rows[0].next), (4, "s1", 2));
    assert!(since.rows[0].bytes > before, "bytes add up: {} then {}", before, since.rows[0].bytes);
    assert_eq!(since.rows[0].remade, 0, "never indexed again: no remade number");
    assert_eq!(hub.changes(None, 0, None, 10).unwrap().rows.len(), 3, "one row a stream");
    // By class: the timeline is its own class, apart from small state.
    let timeline = hub.changes(Some(2), 0, None, 10).unwrap();
    assert_eq!(timeline.rows.iter().map(|row| row.stream.as_str()).collect::<Vec<_>>(), vec!["timeline"]);
    let small = hub.changes(Some(1), 0, None, 10).unwrap();
    assert_eq!(small.rows.len(), 2);
    // A new generation after being put back.
    let renewed = hub.renew_generation().unwrap();
    assert_ne!(renewed, generation);
    assert_eq!(hub.changes(None, 0, None, 1).unwrap().generation, renewed);
    // A device's place in a library, forgotten on leaving.
    producer.fed("h", "account:a:1", &renewed, 4, 7).unwrap();
    assert_eq!(producer.feed("h", "account:a:1").unwrap(), Some((renewed.clone(), 4)));
    assert_eq!(producer.feed_owners("h").unwrap(), vec!["account:a:1".to_string()]);
    producer.join("h", &[1; 32], false).unwrap();
    producer.leave("h", &[1; 32]).unwrap();
    assert!(producer.feed("h", "account:a:1").unwrap().is_none(), "left: its places go");
    // Commits are told for upload once each.
    let committed = producer.committed();
    assert!(committed.contains("s1") && committed.contains("s3"), "{committed:?}");
    assert!(producer.committed().is_empty(), "taken once");
    let _ = std::fs::remove_dir_all(&base);
    println!("changes drill ok");
}
