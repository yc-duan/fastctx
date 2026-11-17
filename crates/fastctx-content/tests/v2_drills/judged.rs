//! A drill of records judged where their frame stands (U02,
// U03): not kept.
use fastctx_content::v2::{Authorizer, Commit, Entry, Header, Holder, HubSigner, KindRecord, Library, Received, Signer, Taking, Verifier};
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
/// Kind 2 always known; kind 1 once `.0`. A from nothing, B only after A,
/// C from nothing or after B; "bad" never.
struct Rules(bool);
impl Authorizer for Rules {
    fn authorize(&self, _: &Header, record: &KindRecord, prior: Option<&Entry>) -> Result<(), String> {
        let State::Is { body, .. } = &record.record.state else { return Ok(()) };
        let prior = prior.map(|entry| entry.body.as_slice());
        let ok = match body.as_slice() {
            b"A" => prior.is_none(),
            b"B" => prior == Some(b"A"),
            b"C" => prior.is_none() || prior == Some(b"B"),
            b"bad" => false,
            _ => true,
        };
        if ok { Ok(()) } else { Err(format!("{:?} after {:?}", String::from_utf8_lossy(body), prior.map(String::from_utf8_lossy))) }
    }
    fn knows(&self, kind: u16, _: u16) -> bool { self.0 || kind != 1 }
}
fn rec(kind: u16, item: &str, body: &str) -> KindRecord {
    KindRecord { kind, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: body.as_bytes().to_vec(), objects: vec![] } } }
}
struct AnyHub;
impl fastctx_content::v2::HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0(stream: &str) -> fastctx_content::v2::Signed {
    let fact = fastctx_content::v2::EpochFact { scope: "s".into(), stream: stream.into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: fastctx_content::v2::EpochKind::Source, predecessor: None, follows: [0; 32] };
    fastctx_content::v2::Signed { fact: fact.encode(), signature: vec![] }
}
fn body(library: &Library, stream: &str, item: &str) -> Option<String> {
    library.item("s", stream, item.as_bytes()).unwrap().map(|entry| String::from_utf8_lossy(&entry.body).into_owned())
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-judged-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h", "r"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let (replica, _) = Library::open(&base.join("r")).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    producer.install("s", b"d".to_vec(), 1).unwrap();
    hub.install("s", b"d".to_vec(), 1).unwrap();
    replica.install("s", b"d".to_vec(), 1).unwrap();
    let c = |stream: &str, records| Commit { scope: "s".into(), stream: stream.into(), records, leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) };
    // F: A then B of item i, a kind the hub does not know yet; G: C of i, a kind it knows.
    producer.raw_commit(c("t", vec![rec(1, "i", "A"), rec(1, "i", "B")]), signer.clone()).unwrap();
    producer.raw_commit(c("t", vec![rec(2, "i", "C")]), signer.clone()).unwrap();
    // F2: "bad" of item j, a kind the hub does not know yet.
    producer.raw_commit(c("u", vec![rec(1, "j", "bad")]), signer.clone()).unwrap();
    let anyone = |_: &ObjectRef| true;
    let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let hub_signer: Arc<dyn HubSigner> = Arc::new(Fake);
    let mut frames = Vec::new();
    for stream in ["t", "u"] {
        for frame in producer.export("s", stream, 0, 10).unwrap() {
            let taken = hub.take("s", frame.clone(), &Fake, &open, Arc::new(Rules(false)), hub_signer.clone()).unwrap();
            assert!(matches!(taken, Received::Applied { .. }), "{taken:?}");
            frames.push(frame);
        }
    }
    assert_eq!(body(&hub, "t", "i"), None, "C waits behind A and B, still out (V01)");
    // U03: once kind 1 is known, A, B and C are judged in their order, each where it stands.
    let went = hub.reconsider(Arc::new(Rules(true)), true).unwrap();
    assert_eq!(went, 3, "A, B and C all let in");
    assert_eq!(body(&hub, "t", "i").as_deref(), Some("C"), "C is the latest");
    assert_eq!(body(&hub, "u", "j"), None, "bad refused");
    hub.truncate("s", "t", Some(1)).unwrap();
    assert_eq!(body(&hub, "t", "i").as_deref(), Some("B"), "back to F alone, the item is as F left it");
    hub.reconsider(Arc::new(Rules(true)), true).unwrap();
    hub.truncate("s", "u", None).unwrap();
    // U02: a replica that knows kind 1 judges as the hub did.
    replica.know(Arc::new(Rules(true)));
    let holding = replica.holding("s").unwrap().unwrap();
    for stream in ["t", "u"] { replica.accept_epoch("s", epoch0(stream), &AnyHub).unwrap(); }
    for frame in frames {
        let imported = replica.import("s", frame, &Fake, Holder::Replica, holding).unwrap();
        eprintln!("imported {imported:?}");
    }
    assert_eq!(body(&replica, "t", "i").as_deref(), Some("C"), "A, B, then C after B");
    assert_eq!(body(&replica, "u", "j"), None, "bad refused here too");
    replica.truncate("s", "t", Some(1)).unwrap();
    assert_eq!(body(&replica, "t", "i").as_deref(), Some("B"));
    println!("v2 judged drill: ok");
}
