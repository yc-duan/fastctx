//! A drill of withheld streams (S05) and records of a kind not
// known yet taken in later (S09); not kept.
use fastctx_content::v2::{Authorizer, Commit, Entry, Header, HubSigner, KindRecord, Library, Op, Received, Signer, Taking, Verifier};
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
struct Knows(bool);
impl Authorizer for Knows {
    fn authorize(&self, _: &Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> { Ok(()) }
    fn knows(&self, kind: u16, _: u16) -> bool { self.0 || kind != 1 }
}
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-letgo-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h", "k"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    producer.install("s", b"d".to_vec(), 1).unwrap();
    let c = |stream: &str, records| Commit { scope: "s".into(), stream: stream.into(), records, leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap_or(0) };
    // S05: a stream withheld keeps what was never sent, and takes no commit until given back.
    producer.raw_commit(c("t", vec![rec("a")]), signer.clone()).unwrap();
    producer.withhold("s", "t", &[1; 32]).unwrap();
    assert_eq!(producer.resubmits("s", "t", 10).unwrap().len(), 1, "the unsent record is kept to commit again");
    assert!(producer.item("s", "t", b"a").unwrap().is_none());
    let refused = producer.raw_commit(c("t", vec![rec("b")]), signer.clone());
    assert!(refused.is_err(), "a withheld stream takes no commit: {refused:?}");
    assert_eq!(producer.resubmit("s", "t", 10, signer.clone()).unwrap(), 0, "nor a recommit");
    producer.hold_again("s", producer.holding("s").unwrap().unwrap(), vec!["t".into()]).unwrap();
    assert_eq!(producer.resubmit("s", "t", 10, signer.clone()).unwrap(), 1, "given back, what waited is committed again");
    producer.raw_commit(c("t", vec![rec("b")]), signer.clone()).unwrap();
    assert!(producer.item("s", "t", b"a").unwrap().is_some() && producer.item("s", "t", b"b").unwrap().is_some());
    // S09: a hub that does not know kind 1 keeps its records out of the index, and takes them in once it does.
    hub.install("s", b"d".to_vec(), 1).unwrap();
    producer.raw_commit(c("u", vec![rec("x"), rec("y")]), signer.clone()).unwrap();
    let frames = producer.export("s", "u", 0, 10).unwrap();
    let anyone = |_: &ObjectRef| true;
    let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let hub_signer: Arc<dyn HubSigner> = Arc::new(Fake);
    let taken = hub.take("s", frames[0].clone(), &Fake, &open, Arc::new(Knows(false)), hub_signer.clone()).unwrap();
    assert!(matches!(taken, Received::Applied { .. }), "an unknown kind does not refuse the frame: {taken:?}");
    assert!(hub.item("s", "u", b"x").unwrap().is_none(), "kept out of the index");
    assert_eq!(hub.reconsider(Arc::new(Knows(false)), true).unwrap(), 0, "still not known: still out");
    assert_eq!(hub.reconsider(Arc::new(Knows(true)), true).unwrap(), 2, "known now: both records go in");
    assert!(hub.item("s", "u", b"x").unwrap().is_some() && hub.item("s", "u", b"y").unwrap().is_some());
    assert_eq!(hub.reconsider(Arc::new(Knows(true)), true).unwrap(), 0, "and only once");
    // A stream that stays on the device that orders it was never on its way to a hub: its
    // holding ended, nothing of it is kept as unsent, while another stream's unsent write is.
    let (keeper, _) = Library::open(&base.join("k")).unwrap();
    keeper.install("s", b"d".to_vec(), 1).unwrap();
    let held = keeper.holding("s").unwrap().unwrap();
    for stream in ["stays", "goes"] {
        let op = Op { id: format!("op:{stream}"), digest: [7; 32], kind: "talk".into() };
        let write = Commit { scope: "s".into(), stream: stream.into(), records: vec![rec(stream)], leases: vec![], durable: true, op: Some((op, true)), holding: held };
        keeper.raw_commit(write, signer.clone()).unwrap();
    }
    keeper.revoke_keeping_but("s", &[1; 32], |stream| stream == "stays").unwrap();
    while keeper.purging().unwrap() { keeper.purge().unwrap(); }
    assert!(keeper.refused("op:stays").unwrap().is_none(), "nothing of a stream kept here waits for its user");
    assert!(keeper.refused("op:goes").unwrap().is_some(), "an unsent write of a stream on its way to the hub is kept for its user");
    println!("v2 letgo drill: ok");
}
