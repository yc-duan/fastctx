//! A drill of a holder's side of succession and cursor checks.
use fastctx_content::v2::{
    Authorizer, Commit, Cutoff, Entry, EpochKind, Header, HubSigner, HubVerifier, Imported, KindRecord, Library, Received, Signer,
    Standing, Successor, Taking, Verifier, Holder,
};
use fastctx_content::{ObjectRef, Record, State};
use sha2::{Digest, Sha256};
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
struct Other;
impl Signer for Other {
    fn device(&self) -> [u8; 32] { [3; 32] }
    fn key(&self) -> [u8; 32] { [4; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
impl Verifier for Fake {
    fn verify(&self, _: &Header, bytes: &[u8], signature: &[u8]) -> bool { bytes == signature }
}
impl HubSigner for Fake {
    fn sign(&self, statement: &[u8]) -> Vec<u8> { Sha256::digest(statement).to_vec() }
}
impl HubVerifier for Fake {
    fn verify(&self, fact: &[u8], signature: &[u8]) -> bool { Sha256::digest(fact).as_slice() == signature }
}
struct Anyone;
impl Authorizer for Anyone {
    fn authorize(&self, _frame: &fastctx_content::v2::Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> { Ok(()) }
}
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-succession-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h", "r", "r2"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let (rep, _) = Library::open(&base.join("r")).unwrap();
    let (rep2, _) = Library::open(&base.join("r2")).unwrap();
    for lib in [&producer, &hub, &rep, &rep2] { lib.install("s", b"d".to_vec(), 1).unwrap(); }
    let (fake, other): (Arc<dyn Signer>, Arc<dyn Signer>) = (Arc::new(Fake), Arc::new(Other));
    let (auth, hs): (Arc<dyn Authorizer>, Arc<dyn HubSigner>) = (Arc::new(Anyone), Arc::new(Fake));
    let pc = |stream: &str, item: &str, signer: &Arc<dyn Signer>| {
        producer.raw_commit(Commit { scope: "s".into(), stream: stream.into(), records: vec![rec(item)], leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap() }, signer.clone()).unwrap();
    };
    let anyone = |_: &ObjectRef| true;
    let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let take = |frame| hub.take("s", frame, &Fake, &open, auth.clone(), hs.clone()).unwrap();
    let chain = |frame: &fastctx_content::v2::RawFrame| Header::decode(&frame.header).unwrap().chain();
    let next_device = Successor { device: [3; 32], key: [4; 32], kind: EpochKind::Source };

    // The hub: g0 taken, the stream handed over at 3, g1 and g2 never come, g3 parked, then a gap fact.
    for n in 0..3 { pc("g", &format!("g{n}"), &fake); }
    let g = producer.export("s", "g", 0, 10).unwrap();
    assert!(matches!(take(g[0].clone()), Received::Applied { .. }));
    hub.handover("s", "g", next_device, Cutoff { next: 3, head: chain(&g[2]) }, auth.clone(), hs.clone()).unwrap();
    producer.new_epoch("s", "g", 1, b"e".to_vec()).unwrap();
    pc("g", "g3", &other);
    let g3 = producer.export("s", "g", 3, 1).unwrap().remove(0);
    assert!(matches!(take(g3.clone()), Received::Parked { .. }));
    let (_, drained) = hub.gap("s", "g", "gone", auth.clone(), hs.clone()).unwrap();
    assert_eq!((drained.applied, drained.through), (1, Some(4)));
    let (epochs, gaps) = hub.succession("s", "g").unwrap();
    assert_eq!((epochs.len(), gaps.len()), (2, 1));

    // A replica takes frames only under the hub's descriptors, and skips the gap on its word.
    let period = rep.holding("s").unwrap().unwrap();
    let from_hub = hub.export("s", "g", 0, 10).unwrap();
    assert_eq!(from_hub.len(), 2, "g0 and g3");
    assert_eq!(rep.import("s", from_hub[1].clone(), &Fake, Holder::Replica, period).unwrap(), Imported::Epoch(1));
    let forged = fastctx_content::v2::Signed { fact: epochs[1].fact.clone(), signature: vec![0; 32] };
    assert!(rep.accept_epoch("s", forged, &Fake).is_err(), "only the hub's word");
    for epoch in &epochs { rep.accept_epoch("s", epoch.clone(), &Fake).unwrap(); rep.accept_epoch("s", epoch.clone(), &Fake).unwrap(); }
    assert!(matches!(rep.import("s", from_hub[0].clone(), &Fake, Holder::Replica, period).unwrap(), Imported::Applied { next: 1, .. }));
    assert_eq!(rep.import("s", from_hub[1].clone(), &Fake, Holder::Replica, period).unwrap(), Imported::Gap { from: 1, to: 2 });
    rep.accept_gap("s", gaps[0].clone(), &Fake).unwrap();
    assert!(matches!(rep.import("s", from_hub[1].clone(), &Fake, Holder::Replica, period).unwrap(), Imported::Applied { next: 4, .. }));
    assert!(rep.item("s", "g", b"g3").unwrap().is_some());
    // A gap fact taken before the cursor gets there skips when it does.
    let period2 = rep2.holding("s").unwrap().unwrap();
    for epoch in &epochs { rep2.accept_epoch("s", epoch.clone(), &Fake).unwrap(); }
    rep2.accept_gap("s", gaps[0].clone(), &Fake).unwrap();
    // The frame that reaches the gap's start skips it at once (M06), with nothing after it.
    let gap = fastctx_content::v2::GapFact::decode(&gaps[0].fact).unwrap();
    assert!(matches!(rep2.import("s", from_hub[0].clone(), &Fake, Holder::Replica, period2).unwrap(), Imported::Applied { next: 3, .. }));
    let standing2 = rep2.standing("s").unwrap();
    assert_eq!(standing2, vec![("g".to_string(), 2, gap.follows)]);
    assert_eq!(rep2.shared_end("s", "g", &[(2, gap.follows)]).unwrap(), Some(2), "a gap's end is a shared place (M09)");
    assert!(matches!(rep2.import("s", from_hub[1].clone(), &Fake, Holder::Replica, period2).unwrap(), Imported::Applied { next: 4, .. }));
    assert_eq!(hub.check_cursor(&[7; 32], "s", "g", (period2, 2, gap.follows)).unwrap(), Standing::Agrees, "a gap's end is a place the hub holds");

    // The hub's word on cursors: agreeing ones are kept as progress; differing ones get the ends to find where they part.
    let standing = rep.standing("s").unwrap();
    assert_eq!(standing, vec![("g".to_string(), 3, chain(&g3))]);
    assert_eq!(hub.check_cursor(&[7; 32], "s", "g", (period, 3, chain(&g3))).unwrap(), Standing::Agrees);
    let Standing::Differs(ends) = hub.check_cursor(&[7; 32], "s", "g", (period, 3, [9; 32])).unwrap() else { panic!("differs") };
    assert_eq!(ends, vec![(3, chain(&g3)), (2, gap.follows), (0, chain(&g[0]))]);
    assert_eq!(rep.shared_end("s", "g", &ends).unwrap(), Some(3));
    assert_eq!(rep.shared_end("s", "g", &ends[1..]).unwrap(), Some(2), "a gap's end is a shared place (M09)");
    rep.truncate("s", "g", Some(2)).unwrap();
    assert_eq!(rep.standing("s").unwrap().into_iter().find(|(name, _, _)| name == "g"), Some(("g".to_string(), 2, gap.follows)), "cut back to a gap's end");

    // The cutoff on a replica: a predecessor's frame past it is refused, so is a frame of the wrong device.
    for n in 0..3 { pc("c", &format!("c{n}"), &fake); }
    let c = producer.export("s", "c", 0, 10).unwrap();
    assert!(matches!(take(c[0].clone()), Received::Applied { .. }));
    hub.handover("s", "c", next_device, Cutoff { next: 2, head: chain(&c[1]) }, auth.clone(), hs.clone()).unwrap();
    assert!(matches!(take(c[1].clone()), Received::Applied { .. }));
    for epoch in hub.succession("s", "c").unwrap().0 { rep.accept_epoch("s", epoch, &Fake).unwrap(); }
    for frame in hub.export("s", "c", 0, 10).unwrap() { assert!(matches!(rep.import("s", frame, &Fake, Holder::Replica, period).unwrap(), Imported::Applied { .. })); }
    assert!(rep.import("s", c[2].clone(), &Fake, Holder::Replica, period).is_err(), "past the cutoff, from a peer");
    producer.new_epoch("s", "c", 1, b"e".to_vec()).unwrap();
    pc("c", "c3", &fake);
    let wrong = producer.export("s", "c", 3, 1).unwrap().remove(0);
    assert!(rep.import("s", wrong, &Fake, Holder::Replica, period).is_err(), "epoch 1 is the other device's");

    println!("v2 succession drill: ok");
    let _ = std::fs::remove_dir_all(&base);
}
