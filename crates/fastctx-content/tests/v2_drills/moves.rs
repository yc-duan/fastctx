//! A drill of moves under a release (design §28.10).
use fastctx_content::v2::{
    Authority, Authorizer, Commit, EpochFact, EpochKind, Entry, Header, Holder, HubSigner, HubVerifier, Imported, KindRecord, Library, Moved, Received, Taken,
    Release, Signer, Successor, Taking, Verifier,
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
/// A device's signature in the drill: the statement and the key's first byte.
fn signed_by(key: [u8; 32], release: &Release) -> Vec<u8> {
    let mut signature = release.statement();
    signature.push(key[0]);
    signature
}
fn verify(key: &[u8; 32], statement: &[u8], signature: &[u8]) -> bool {
    signature.len() == statement.len() + 1 && signature.starts_with(statement) && signature[statement.len()] == key[0]
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-moves-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h", "b"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (producer, _) = Library::open(&base.join("p")).unwrap();
    let (hub, _) = Library::open_hub(&base.join("h")).unwrap();
    let (b_lib, _) = Library::open(&base.join("b")).unwrap();
    for lib in [&producer, &hub, &b_lib] { lib.install("s", b"d".to_vec(), 1).unwrap(); }
    let fake: Arc<dyn Signer> = Arc::new(Fake);
    let (auth, hs): (Arc<dyn Authorizer>, Arc<dyn HubSigner>) = (Arc::new(Anyone), Arc::new(Fake));
    let commit = |stream: &str, item: &str| {
        producer.raw_commit(Commit { scope: "s".into(), stream: stream.into(), records: vec![rec(item)], leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap() }, fake.clone()).unwrap();
    };
    let anyone = |_: &ObjectRef| true;
    let open = Taking { period: hub.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let take = |frame| hub.take("s", frame, &Fake, &open, auth.clone(), hs.clone()).unwrap();
    let chain = |frame: &fastctx_content::v2::RawFrame| Header::decode(&frame.header).unwrap().chain();
    let to_b = Successor { device: [3; 32], key: [4; 32], kind: EpochKind::Source };
    let to_c = Successor { device: [5; 32], key: [6; 32], kind: EpochKind::Source };
    let move_to = |stream: &str, machine: &str, successor: Successor, release: &Release, signature: &[u8]| {
        hub.move_stream("s", stream, machine, successor, Authority::Release { release, signature, verify: &verify }, auth.clone(), hs.clone())
    };

    // Five frames written; the hub took three, the device it goes to carries the last two.
    for n in 0..5 { commit("t", &format!("t{n}")); }
    let t = producer.export("s", "t", 0, 10).unwrap();
    for frame in &t[..3] { assert!(matches!(take(frame.clone()), Received::Applied { .. })); }
    let release = Release { hub: "h".into(), scope: "s".into(), stream: "t".into(), epoch: 0, to: "b".into(), to_key: to_b.key, next: 5, head: chain(&t[4]) };
    let good = signed_by([2; 32], &release);
    assert!(move_to("t", "b", to_b, &release, &good).is_err(), "past what the hub holds: refused until the tail is sent on");
    for frame in &t[3..] { assert!(matches!(take(frame.clone()), Received::Applied { .. }), "the tail, sent on by the device it goes to"); }
    assert!(move_to("t", "b", to_b, &release, &signed_by([9; 32], &release)).is_err(), "not the predecessor's signature");
    assert!(move_to("t", "c", to_c, &release, &good).is_err(), "released to another machine");
    // AP01: another device under b's name since cannot use it.
    let other_b = Successor { device: [8; 32], key: [9; 32], kind: EpochKind::Source };
    assert!(move_to("t", "b", other_b, &release, &good).is_err(), "another device given the name since");
    let mut wrong = release.clone();
    wrong.head = [7; 32];
    assert!(move_to("t", "b", to_b, &wrong, &signed_by([2; 32], &wrong)).is_err(), "a place the predecessor's frames do not pass through");
    let Moved::Handed { descriptor, .. } = move_to("t", "b", to_b, &release, &good).unwrap() else { panic!("handed") };
    let fact = EpochFact::decode(&descriptor.fact).unwrap();
    assert_eq!((fact.epoch, fact.device, fact.start), (1, [3; 32], 5));
    let Moved::Handed { descriptor: again, .. } = move_to("t", "b", to_b, &release, &good).unwrap() else { panic!("the same again") };
    assert_eq!(again, descriptor, "asked again, the same answer");

    // AN03: released short of what the hub took — it wrote on and sent that too — the cut is the furthest.
    for n in 0..4 { commit("u", &format!("u{n}")); }
    let u = producer.export("s", "u", 0, 10).unwrap();
    for frame in &u { assert!(matches!(take(frame.clone()), Received::Applied { .. })); }
    let short = Release { hub: "h".into(), scope: "s".into(), stream: "u".into(), epoch: 0, to: "b".into(), to_key: to_b.key, next: 2, head: chain(&u[1]) };
    let Moved::Handed { descriptor: cut, .. } = move_to("u", "b", to_b, &short, &signed_by([2; 32], &short)).unwrap() else { panic!("handed") };
    assert_eq!(EpochFact::decode(&cut.fact).unwrap().start, 4, "at the furthest the hub holds");

    // Handed on since — b lets t go to c — b's move asked again is told so (AN02).
    let onward = Release { hub: "h".into(), scope: "s".into(), stream: "t".into(), epoch: 1, to: "c".into(), to_key: to_c.key, next: 5, head: fact.follows };
    let Moved::Handed { descriptor: to_c_epoch, .. } = move_to("t", "c", to_c, &onward, &signed_by([4; 32], &onward)).unwrap() else { panic!("onward") };
    let Moved::Superseded { epochs } = move_to("t", "b", to_b, &release, &good).unwrap() else { panic!("handed on since") };
    assert_eq!(epochs, vec![to_c_epoch.clone()]);
    // AP02: c lets it go back to b — a later move to the same device, from a later epoch — and b takes it again.
    let to_c_fact = EpochFact::decode(&to_c_epoch.fact).unwrap();
    let back = Release { hub: "h".into(), scope: "s".into(), stream: "t".into(), epoch: 2, to: "b".into(), to_key: to_b.key, next: 5, head: to_c_fact.follows };
    let Moved::Handed { descriptor: back_epoch, .. } = move_to("t", "b", to_b, &back, &signed_by([6; 32], &back)).unwrap() else { panic!("back to b") };
    assert_eq!(EpochFact::decode(&back_epoch.fact).unwrap().epoch, 3, "b's second epoch");

    // Adopted by the account that opened it, the device it lived on lost: from the epoch it was seen in, at what the hub holds.
    for n in 0..3 { commit("w", &format!("w{n}")); }
    for frame in producer.export("s", "w", 0, 10).unwrap() { assert!(matches!(take(frame), Received::Applied { .. })); }
    let adopt = |from| hub.move_stream("s", "w", "b", to_b, Authority::Adopt { from }, auth.clone(), hs.clone());
    assert!(adopt(1).is_err(), "not the epoch it is in");
    let Moved::Handed { descriptor: adopted, .. } = adopt(0).unwrap() else { panic!("adopted") };
    assert_eq!(EpochFact::decode(&adopted.fact).unwrap().start, 3, "at what the hub holds");
    let Moved::Handed { descriptor: again, .. } = adopt(0).unwrap() else { panic!("the same again") };
    assert_eq!(again, adopted);
    // §29 4: an export the old device made before the account took the conversation back fails after: the epoch it
    // would let go of is over, and what that device writes on in it is not taken into the stream.
    let w = producer.export("s", "w", 0, 10).unwrap();
    let stale = Release { hub: "h".into(), scope: "s".into(), stream: "w".into(), epoch: 0, to: "c".into(), to_key: to_c.key, next: 3, head: chain(&w[2]) };
    assert!(!matches!(move_to("w", "c", to_c, &stale, &signed_by([2; 32], &stale)), Ok(Moved::Handed { .. })), "an export from before the take-back moves nothing");
    commit("w", "w3");
    let late = producer.export("s", "w", 3, 1).unwrap().remove(0);
    assert!(!matches!(take(late), Received::Applied { .. }), "the old device's later frame is not in the stream");
    assert!(hub.item("s", "w", b"w3").unwrap().is_none());

    // On b: not before it holds the stream up to where its epoch begins, and nothing of it is known then;
    // then taken and opened in one transaction; once a later one is known, history alone.
    assert_eq!(b_lib.take_over("s", descriptor.clone(), &Fake).unwrap(), Taken::Behind, "behind the epoch's start");
    assert_eq!(b_lib.known_last_epoch("s", "t").unwrap(), None, "nothing of the epoch known while behind it");
    let held = b_lib.holding("s").unwrap().unwrap();
    // As a replica takes it: the hub's word on the epoch the frames are of, then the frames.
    let (epochs, _) = hub.succession("s", "t").unwrap();
    b_lib.accept_epoch("s", epochs[0].clone(), &Fake).unwrap();
    for frame in &t {
        let got = b_lib.import("s", frame.clone(), &Fake, Holder::Replica, held).unwrap();
        assert!(matches!(got, Imported::Applied { .. }), "{got:?}");
    }
    assert_eq!(b_lib.take_over("s", descriptor.clone(), &Fake).unwrap(), Taken::Opened, "opened");
    assert_eq!(b_lib.take_over("s", descriptor.clone(), &Fake).unwrap(), Taken::Opened, "the same again");
    b_lib.accept_epoch("s", to_c_epoch, &Fake).unwrap();
    let stale = fastctx_content::v2::Signed { fact: descriptor.fact.clone(), signature: descriptor.signature.clone() };
    assert_eq!(b_lib.take_over("s", stale, &Fake).unwrap(), Taken::History, "handed on since: history alone");
    let _ = std::fs::remove_dir_all(&base);
    println!("v2 moves drill: ok");
}
