//! A drill of records waiting behind an earlier record of their
// item still out (V01): not kept.
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
struct AnyHub;
impl fastctx_content::v2::HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0(stream: &str) -> fastctx_content::v2::Signed {
    let fact = fastctx_content::v2::EpochFact { scope: "s".into(), stream: stream.into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: fastctx_content::v2::EpochKind::Source, predecessor: None, follows: [0; 32] };
    fastctx_content::v2::Signed { fact: fact.encode(), signature: vec![] }
}
/// Kind 2 always known, kind 3 once `.0`. `.1`: B only after A (else B only from nothing); A from nothing.
struct Rules(bool, bool);
impl Authorizer for Rules {
    fn authorize(&self, _: &Header, record: &KindRecord, prior: Option<&Entry>) -> Result<(), String> {
        let State::Is { body, .. } = &record.record.state else { return Ok(()) };
        let prior = prior.map(|entry| entry.body.as_slice());
        let ok = match body.as_slice() {
            b"A" => prior.is_none(),
            b"B" if self.1 => prior == Some(b"A"),
            b"B" => prior.is_none(),
            _ => true,
        };
        if ok { Ok(()) } else { Err("no".into()) }
    }
    fn knows(&self, kind: u16, _: u16) -> bool { self.0 || kind != 3 }
}
fn rec(kind: u16, item: &str, body: &str) -> KindRecord {
    KindRecord { kind, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: body.as_bytes().to_vec(), objects: vec![] } } }
}
fn body(library: &Library, stream: &str, item: &str) -> Option<String> {
    library.item("s", stream, item.as_bytes()).unwrap().map(|entry| String::from_utf8_lossy(&entry.body).into_owned())
}
fn open(base: &std::path::Path, name: &str, hub: bool) -> Library {
    std::fs::create_dir_all(base.join(name)).unwrap();
    let (library, _) = if hub { Library::open_hub(&base.join(name)).unwrap() } else { Library::open(&base.join(name)).unwrap() };
    library.install("s", b"d".to_vec(), 1).unwrap();
    library
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-defer-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let producer = open(&base, "p", false);
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let c = |stream: &str, records| Commit { scope: "s".into(), stream: stream.into(), records, leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) };
    producer.raw_commit(c("t", vec![rec(3, "i", "A"), rec(2, "i", "B")]), signer.clone()).unwrap();
    let frames = producer.export("s", "t", 0, 10).unwrap();
    let anyone = |_: &ObjectRef| true;
    let hub_signer: Arc<dyn HubSigner> = Arc::new(Fake);
    let take = |hub: &Library, rules: Rules| {
        let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
        let taken = hub.take("s", frames[0].clone(), &Fake, &open, Arc::new(rules), hub_signer.clone()).unwrap();
        assert!(matches!(taken, Received::Applied { .. }), "{taken:?}");
    };
    let import = |replica: &Library| {
        replica.accept_epoch("s", epoch0("t"), &AnyHub).unwrap();
        let holding = replica.holding("s").unwrap().unwrap();
        replica.import("s", frames[0].clone(), &Fake, Holder::Replica, holding).unwrap();
    };
    // Case 1 (B after A): the hub knows both; a replica knowing only kind 2 waits, and agrees once it knows kind 3.
    let hub = open(&base, "h1", true);
    take(&hub, Rules(true, true));
    assert_eq!(body(&hub, "t", "i").as_deref(), Some("B"));
    let replica = open(&base, "r1", false);
    replica.know(Arc::new(Rules(false, true)));
    import(&replica);
    assert_eq!(body(&replica, "t", "i"), None, "B waits behind A, not refused");
    replica.know(Arc::new(Rules(true, true)));
    replica.reconsider(Arc::new(Rules(true, true)), true).unwrap();
    assert_eq!(body(&replica, "t", "i").as_deref(), Some("B"), "known at last, A then B, as the hub has it");
    // Case 2 (B only from nothing): a hub not knowing kind 3 lets nothing in for i; once it does, A then B judged:
    // B refused, A stands — as a replica that knew both from the first finds.
    let hub = open(&base, "h2", true);
    take(&hub, Rules(false, false));
    assert_eq!(body(&hub, "t", "i"), None, "B waits behind A at the hub too");
    hub.reconsider(Arc::new(Rules(true, false)), true).unwrap();
    assert_eq!(body(&hub, "t", "i").as_deref(), Some("A"));
    let fresh = open(&base, "r2", false);
    fresh.know(Arc::new(Rules(true, false)));
    import(&fresh);
    assert_eq!(body(&fresh, "t", "i").as_deref(), Some("A"), "the same as the hub");
    // A later frame of i at the hub while A still waits: it waits too.
    let hub3 = open(&base, "h3", true);
    take(&hub3, Rules(false, true));
    producer.raw_commit(c("t", vec![rec(2, "i", "C")]), signer.clone()).unwrap();
    let later = producer.export("s", "t", 2, 10).unwrap();
    let open3 = Taking { period: hub3.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let taken = hub3.take("s", later[0].clone(), &Fake, &open3, Arc::new(Rules(false, true)), hub_signer.clone()).unwrap();
    assert!(matches!(taken, Received::Applied { .. }), "{taken:?}");
    assert_eq!(body(&hub3, "t", "i"), None, "C waits behind A as well");
    hub3.reconsider(Arc::new(Rules(true, true)), true).unwrap();
    assert_eq!(body(&hub3, "t", "i").as_deref(), Some("C"), "A, B, C in their order");
    // X01: what waited in an ended holding period holds nothing back in the next.
    producer.raw_commit(c("u", vec![rec(2, "x", "K0")]), signer.clone()).unwrap();
    producer.raw_commit(c("u", vec![rec(3, "x", "U1")]), signer.clone()).unwrap();
    let both = producer.export("s", "u", 0, 10).unwrap();
    assert_eq!(both.len(), 2);
    let r4 = open(&base, "r4", false);
    r4.know(Arc::new(Rules(false, true)));
    r4.accept_epoch("s", epoch0("u"), &AnyHub).unwrap();
    let h1 = r4.holding("s").unwrap().unwrap();
    for frame in &both {
        r4.import("s", frame.clone(), &Fake, Holder::Replica, h1).unwrap();
    }
    assert_eq!(body(&r4, "u", "x").as_deref(), Some("K0"), "K0 in, U1 out of the index in the first period");
    r4.revoke("s").unwrap();
    r4.install("s", b"d".to_vec(), 2).unwrap();
    let h2 = r4.holding("s").unwrap().unwrap();
    assert_ne!(h1, h2);
    let _ = r4.accept_epoch("s", epoch0("u"), &AnyHub);
    r4.import("s", both[0].clone(), &Fake, Holder::Replica, h2).unwrap();
    assert_eq!(body(&r4, "u", "x").as_deref(), Some("K0"), "the old period's waiting holds back no known record of the new one");
    while r4.purging().unwrap() { r4.purge().unwrap(); }
    r4.import("s", both[1].clone(), &Fake, Holder::Replica, h2).unwrap();
    assert_eq!(body(&r4, "u", "x").as_deref(), Some("K0"), "still K0 once U1 waits in the new period");
    println!("v2 defer drill: ok");
}
