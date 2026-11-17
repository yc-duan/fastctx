//! A drill of the fixes for codex-7965d54c's review of the v2 library.
use fastctx_content::v2::{CHUNK_BYTES, Commit, Header, Holder, Imported, KindRecord, LEASE, Library, Op, Receipt, Said, Signer, Verifier, open};
use fastctx_content::{ObjectRef, Record, State};
use fastctx_storage::rusqlite::{Connection, params};
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
fn rec(item: &str, order: &str, body: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: order.as_bytes().to_vec(), state: State::Is { body: body.as_bytes().to_vec(), objects: vec![] } } }
}

struct AnyHub;
impl fastctx_content::v2::HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0(scope: &str, stream: &str) -> fastctx_content::v2::Signed {
    let fact = fastctx_content::v2::EpochFact { scope: scope.into(), stream: stream.into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: fastctx_content::v2::EpochKind::Source, predecessor: None, follows: [0; 32] };
    fastctx_content::v2::Signed { fact: fact.encode(), signature: vec![] }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-review-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["a", "b", "c"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let (lib, _) = Library::open(&base.join("a")).unwrap();
    for scope in ["s1", "s2"] { lib.install(scope, b"d".to_vec(), 1).unwrap(); }
    let commit = |scope: &str, records, op| Commit { scope: scope.into(), stream: "t".into(), records, leases: vec![], durable: true, op, holding: lib.holding(scope).unwrap().unwrap() };
    let op = |id: &str| Op { id: id.into(), digest: Sha256::digest(id.as_bytes()).into(), kind: "talk".into() };
    // J02: the same stream and item in two scopes, each with its own operation.
    let at1 = lib.raw_commit(commit("s1", vec![rec("x", "1", "words in s1")], Some((op("dev:1"), true))), signer.clone()).unwrap().first;
    lib.raw_commit(commit("s2", vec![rec("x", "1", "words in s2")], Some((op("dev:2"), true))), signer.clone()).unwrap();
    lib.rejected("s1", "t", at1).unwrap();
    let waiting = lib.resubmits("s1", "t", 1000).unwrap();
    assert_eq!(waiting[0].op.as_deref(), Some("dev:1"), "s1's record knows its own operation");
    // J01: refused, the write as made stays in its operation, with why.
    lib.refuse("s1", "t", waiting[0].id, "no longer writable").unwrap();
    drop(lib);
    let (lib, _) = Library::open(&base.join("a")).unwrap();
    let (records, why) = lib.refused("dev:1").unwrap().expect("kept after a restart");
    assert_eq!((records.len(), why.as_str()), (1, "no longer writable"));
    assert_eq!(records[0].record.state, State::Is { body: b"words in s1".to_vec(), objects: vec![] });
    assert!(lib.refused("dev:2").unwrap().is_none());
    lib.dropped("dev:1").unwrap();
    assert!(lib.refused("dev:1").unwrap().is_none());
    // J06: a commit made in a period that ended is refused.
    let stale = lib.holding("s2").unwrap().unwrap();
    lib.revoke("s2").unwrap();
    lib.install("s2", b"d".to_vec(), 2).unwrap();
    let late = Commit { scope: "s2".into(), stream: "t".into(), records: vec![rec("y", "2", "late")], leases: vec![], durable: true, op: None, holding: stale };
    assert!(lib.raw_commit(late, signer.clone()).is_err());
    // J04: empty and repeated order keys page through whole.
    let held = lib.holding("s1").unwrap().unwrap();
    let pc = |records| Commit { scope: "s1".into(), stream: "p".into(), records, leases: vec![], durable: false, op: None, holding: held };
    lib.raw_commit(pc(vec![rec("e", "", "empty order"), rec("a", "k", "1"), rec("b", "k", "2"), rec("c", "k", "3")]), signer.clone()).unwrap();
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = lib.page("s1", "p", after.clone(), 1).unwrap();
        let Some(last) = page.last() else { break };
        after = Some((last.order.clone(), last.item.clone()));
        seen.extend(page.into_iter().map(|e| e.item));
    }
    assert_eq!(seen, vec![b"e".to_vec(), b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    // J05: a frame's count is checked before anything is sized by it.
    let frames = lib.export("s1", "p", 0, 1).unwrap();
    let mut header = Header::decode(&frames[0].header).unwrap();
    header.count = 0;
    assert!(open(&header, &frames[0].body).is_err());
    header.count = 5000;
    assert!(open(&header, &frames[0].body).is_err());
    header.count = 3;
    assert!(open(&header, &frames[0].body).is_err(), "more records than said");
    // J07, J08, T01: a parked then an applied receipt for one frame are both kept and both reported; the outbox moves past it.
    let digest = Header::decode(&frames[0].header).unwrap().digest;
    let receipt = |said| Receipt { stream: "p".into(), epoch: 0, first: 0, digest, said, signature: vec![7] };
    lib.receipt("s1", receipt(Said::Parked { sequence: 1, accumulated: [3; 32] }), 1).unwrap();
    let db = Connection::open(base.join("a").join("assets.sqlite3")).unwrap();
    let uploaded: i64 = db.query_row("SELECT uploaded FROM outbox", [], |r| r.get(0)).unwrap();
    assert_eq!(uploaded, 4, "a parked frame counts as uploaded");
    lib.receipt("s1", receipt(Said::Applied { change: 9, prefix: [9; 32] }), 1).unwrap();
    let reported: Vec<_> = lib.unwitnessed(None, 1000).unwrap().receipts.into_iter().filter(|(s, _)| s == "s1").map(|(_, r)| r.said).collect();
    assert!(reported.contains(&Said::Parked { sequence: 1, accumulated: [3; 32] }) && reported.contains(&Said::Applied { change: 9, prefix: [9; 32] }), "{reported:?}");
    // J09: a replica cut back to the empty start takes the stream again from its first frame.
    let (rep, _) = Library::open(&base.join("b")).unwrap();
    rep.install("s1", b"d".to_vec(), 1).unwrap();
    rep.accept_epoch("s1", epoch0("s1", "p"), &AnyHub).unwrap();
    let period = rep.holding("s1").unwrap().unwrap();
    for frame in lib.export("s1", "p", 0, 10).unwrap() { rep.import("s1", frame, &Fake, Holder::Replica, period).unwrap(); }
    rep.truncate("s1", "p", None).unwrap();
    assert!(rep.page("s1", "p", None, 10).unwrap().is_empty());
    let again = rep.import("s1", lib.export("s1", "p", 0, 1).unwrap().remove(0), &Fake, Holder::Replica, period).unwrap();
    assert!(matches!(again, Imported::Applied { .. }), "{again:?}");
    // J06 for imports: a period that ended takes nothing.
    rep.revoke("s1").unwrap();
    rep.install("s1", b"d".to_vec(), 2).unwrap();
    assert!(rep.import("s1", lib.export("s1", "p", 0, 1).unwrap().remove(0), &Fake, Holder::Replica, period).is_err());
    // J03: a transfer under a wrong manifest fails whole and leaves the way open for a right one.
    let (dst, _) = Library::open(&base.join("c")).unwrap();
    let bytes: Vec<u8> = (0..(2 * CHUNK_BYTES + 5)).map(|i| (i % 249) as u8).collect();
    let object = ObjectRef { hash: Sha256::digest(&bytes).into(), size: bytes.len() as u64 };
    let wrong: Vec<u8> = vec![1u8; bytes.len()];
    let wrong_digests: Vec<[u8; 32]> = wrong.chunks(CHUNK_BYTES as usize).map(|c| Sha256::digest(c).into()).collect();
    let mut t = dst.receive(object, wrong_digests.clone(), None).unwrap().unwrap();
    for (i, c) in wrong.chunks(CHUNK_BYTES as usize).enumerate() { t.chunk(i as u64, c).unwrap(); }
    assert!(t.finish("drill", LEASE).is_err());
    let right: Vec<[u8; 32]> = bytes.chunks(CHUNK_BYTES as usize).map(|c| Sha256::digest(c).into()).collect();
    let mut t = dst.receive(object, right, None).unwrap().unwrap();
    assert_eq!(t.missing(), vec![0, 1, 2], "a fresh transfer");
    for (i, c) in bytes.chunks(CHUNK_BYTES as usize).enumerate() { t.chunk(i as u64, c).unwrap(); }
    t.finish("drill", LEASE).unwrap();
    assert_eq!(dst.chunks(&object.hash).unwrap().len(), 3);
    let _ = params![1];
    println!("v2 review drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
