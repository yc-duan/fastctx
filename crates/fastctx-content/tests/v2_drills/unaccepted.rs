//! A drill of AH01 (design §28.10): what a device wrote of a stream the hub never took, the hub having it
// from another machine, is let go and the hub's frames come in its place; not kept.
use fastctx_content::v2::{
    Authorizer, Commit, Entry, Header, Holder, HubSigner, HubVerifier, Imported, KindRecord, Library, Received, Signer, Taking, Verifier,
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
struct OnB;
impl Signer for OnB {
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
    fn authorize(&self, _: &Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> { Ok(()) }
}
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-unaccepted-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["a", "h", "b"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (a_lib, _) = Library::open(&base.join("a")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let (b_lib, _) = Library::open(&base.join("b")).unwrap();
    for lib in [&a_lib, &hub, &b_lib] { lib.install("s", b"d".to_vec(), 1).unwrap(); }
    let (on_a, on_b): (Arc<dyn Signer>, Arc<dyn Signer>) = (Arc::new(Fake), Arc::new(OnB));
    let (auth, hs): (Arc<dyn Authorizer>, Arc<dyn HubSigner>) = (Arc::new(Anyone), Arc::new(Fake));
    let commit = |lib: &Library, signer: &Arc<dyn Signer>, item: &str| {
        lib.raw_commit(Commit { scope: "s".into(), stream: "timeline".into(), records: vec![rec(item)], leases: vec![], durable: true, op: None, holding: lib.holding("s").unwrap().unwrap() }, signer.clone()).unwrap();
    };
    let anyone = |_: &ObjectRef| true;
    let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };

    // A wrote five, the hub took them all.
    for n in 0..5 { commit(&a_lib, &on_a, &format!("a{n}")); }
    let frames = a_lib.export("s", "timeline", 0, 10).unwrap();
    for frame in &frames { assert!(matches!(hub.take("s", frame.clone(), &Fake, &open, auth.clone(), hs.clone()).unwrap(), Received::Applied { .. })); }
    // B, the conversation come to it, wrote two of its own before the hub described the scope; it sent none.
    commit(&b_lib, &on_b, "b0");
    commit(&b_lib, &on_b, "b1");
    b_lib.keep_checkpoint("timeline/x", b"where b stood".to_vec()).unwrap();
    assert_eq!(b_lib.unaccepted("s", &[3; 32]).unwrap(), vec![("timeline".to_string(), 2)], "B's own stream, never taken");
    assert!(b_lib.unaccepted("s", &[1; 32]).unwrap().is_empty(), "not another device's");
    // Let go, with where its producer stood, in one transaction.
    assert!(b_lib.discard_unaccepted("s", "timeline", &[3; 32], Some("timeline/x"), None).unwrap());
    assert!(b_lib.unaccepted("s", &[3; 32]).unwrap().is_empty());
    assert_eq!(b_lib.checkpoint("timeline/x").unwrap(), None, "the producer makes it again");
    assert_eq!(b_lib.streams("s").unwrap().into_iter().find(|(s, _)| s == "timeline").map(|(_, n)| n), Some(0), "the stream is empty here");
    assert!(b_lib.page("s", "timeline", None, 10).unwrap().is_empty(), "nothing of it is read");
    assert!(!b_lib.discard_unaccepted("s", "timeline", &[3; 32], None, None).unwrap(), "nothing more to let go");
    // A true source's stream is never let go this way (AQ06).
    assert!(!b_lib.discard_unaccepted("s", "words", &[3; 32], None, None).unwrap());
    // Then the hub's frames come in its place, as a replica's.
    let (epochs, _) = hub.succession("s", "timeline").unwrap();
    b_lib.accept_epoch("s", epochs[0].clone(), &Fake).unwrap();
    let held = b_lib.holding("s").unwrap().unwrap();
    for frame in &frames {
        let got = b_lib.import("s", frame.clone(), &Fake, Holder::Replica, held).unwrap();
        assert!(matches!(got, Imported::Applied { .. }), "{got:?}");
    }
    assert_eq!(b_lib.page("s", "timeline", None, 10).unwrap().len(), 5, "A's five");
    let _ = std::fs::remove_dir_all(&base);
    println!("v2 unaccepted drill: ok");
}
