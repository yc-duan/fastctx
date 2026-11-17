//! A drill of v2 receipts, window pruning and truncation.
use fastctx_content::v2::{Commit, Header, Holder, Imported, KindRecord, Library, Receipt, Said, Signer, Verifier};
use fastctx_content::{Record, State};
use sha2::{Digest, Sha256};
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
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } }
}

struct AnyHub;
impl fastctx_content::v2::HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0(scope: &str, stream: &str) -> fastctx_content::v2::Signed {
    let fact = fastctx_content::v2::EpochFact { scope: scope.into(), stream: stream.into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: fastctx_content::v2::EpochKind::Source, predecessor: None, follows: [0; 32] };
    fastctx_content::v2::Signed { fact: fact.encode(), signature: vec![] }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-window-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("p")).unwrap();
    std::fs::create_dir_all(base.join("r")).unwrap();
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    producer.install("s", b"d".to_vec(), 1).unwrap();
    let mut firsts = Vec::new();
    for item in ["a", "b", "c"] {
        firsts.push(producer.raw_commit(Commit { scope: "s".into(), stream: "log".into(), records: vec![rec(item)], leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) }, signer.clone()).unwrap().first);
    }
    let frames = producer.export("s", "log", 0, 10).unwrap();
    let digest = |i: usize| Header::decode(&frames[i].header).unwrap().digest;
    let receipt = |i: usize, said| Receipt { stream: "log".into(), epoch: 0, first: firsts[i], digest: digest(i), said, signature: vec![i as u8] };
    producer.receipt("s", receipt(0, Said::Applied { change: 1, prefix: [1; 32] }), 1).unwrap();
    producer.receipt("s", receipt(1, Said::Applied { change: 2, prefix: [2; 32] }), 1).unwrap();
    producer.receipt("s", receipt(2, Said::Parked { sequence: 1, accumulated: Sha256::digest(b"acc").into() }), 1).unwrap();
    let unwitnessed = producer.unwitnessed(None, 1000).unwrap().receipts;
    assert_eq!(unwitnessed.len(), 3, "every unwitnessed receipt is reported: {unwitnessed:?}");
    assert!(unwitnessed.iter().any(|(_, r)| r.said == Said::Applied { change: 2, prefix: [2; 32] }));
    // Nothing goes before a checkpoint covers it.
    assert_eq!(producer.prune("s", "log", firsts[1], true).unwrap().frames, 0);
    assert_eq!(producer.checkpointed("s", 1, 0, 1).unwrap(), 1);
    let pruned = producer.prune("s", "log", firsts[1], true).unwrap();
    assert_eq!((pruned.frames, pruned.items, pruned.floor), (1, 1, firsts[1]), "only the witnessed prefix");
    assert!(producer.item("s", "log", b"a").unwrap().is_none());
    assert_eq!(producer.unwitnessed(None, 1000).unwrap().receipts.len(), 2, "change 2 and the parked frame are still reported");
    producer.checkpointed("s", 2, 1, 1).unwrap();
    assert!(producer.unwitnessed(None, 1000).unwrap().receipts.is_empty());
    // A replica cut back to the last agreeing sequence rebuilds and takes the rest again.
    let (replica, _) = Library::open(&base.join("r")).unwrap();
    replica.install("s", b"d".to_vec(), 1).unwrap();
    replica.accept_epoch("s", epoch0("s", "log"), &AnyHub).unwrap();
    for frame in &frames { replica.import("s", frame.clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(); }
    replica.truncate("s", "log", Some(firsts[0])).unwrap();
    assert!(replica.item("s", "log", b"a").unwrap().is_some());
    assert!(replica.item("s", "log", b"b").unwrap().is_none() && replica.item("s", "log", b"c").unwrap().is_none());
    assert!(matches!(replica.import("s", frames[1].clone(), &Fake, Holder::Replica, replica.holding("s").unwrap().unwrap_or(0)).unwrap(), Imported::Applied { .. }));
    assert!(replica.item("s", "log", b"b").unwrap().is_some());
    println!("v2 window drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
