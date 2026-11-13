//! A drill of the networks a device's library is on (leaving a
// network clears its assets; FF04-FF07, AI05-AI10); not kept.
use fastctx_content::v2::{Commit, Library, Signer};
use fastctx_content::{Record, State};
use fastctx_content::v2::KindRecord;
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } }
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-networks-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (lib, _) = Library::open(&base).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let device = [1u8; 32];
    let commit = |lib: &Library, scope: &str, item: &str| {
        let holding = lib.holding(scope).unwrap().unwrap_or(0);
        lib.raw_commit(Commit { scope: scope.into(), stream: "t".into(), records: vec![rec(item)], leases: vec![], durable: true, op: None, holding }, signer.clone())
    };
    // Not on a network: nothing is begun or installed for it.
    assert!(lib.network("h1").unwrap().is_none());
    let n1 = lib.join("h1", &device, false).unwrap();
    assert_eq!(lib.joined().unwrap(), vec!["h1".to_string()]);
    assert_eq!(lib.join("h1", &device, false).unwrap(), n1, "joined again while on it: the same term");
    // A provisional scope and an installed one of h1, each with a record.
    lib.propose_on(&n1, "p1", b"proposal".to_vec()).unwrap();
    commit(&lib, "p1", "a").unwrap();
    lib.install_on(&n1, "i1", b"descriptor".to_vec(), 1).unwrap();
    commit(&lib, "i1", "b").unwrap();
    // Another network's scope is not touched by leaving h1.
    let n2 = lib.join("h2", &device, false).unwrap();
    lib.propose_on(&n2, "p2", b"proposal2".to_vec()).unwrap();
    commit(&lib, "p2", "c").unwrap();
    // Leaving h1 revokes both of its scopes in one go.
    assert_eq!(lib.leave("h1", &device).unwrap(), 2);
    assert!(lib.holding("p1").unwrap().is_none() && lib.holding("i1").unwrap().is_none());
    assert!(lib.holding("p2").unwrap().is_some(), "h2's scope stays");
    assert!(lib.left_held().unwrap().is_empty());
    assert!(lib.network("h1").unwrap().is_none());
    // FF04/AI07: a writer that looked before leaving begins and installs nothing after.
    let late = lib.propose_on(&n1, "p3", b"late".to_vec());
    assert!(late.is_err(), "a proposal under the ended term is refused: {late:?}");
    assert!(lib.install_on(&n1, "i1", b"descriptor".to_vec(), 2).is_err(), "an install under the ended term is refused");
    assert!(lib.holding("p3").unwrap().is_none() && lib.holding("i1").unwrap().is_none());
    // A commit under the ended period is refused (holding checked in the commit).
    assert!(commit(&lib, "p1", "late").is_err());
    // Purges finish by themselves, a pass at a time.
    let mut passes = 0;
    while lib.purging().unwrap() { lib.purge().unwrap(); passes += 1; assert!(passes < 100); }
    assert!(lib.item("p1", "t", b"a").unwrap().is_none() && lib.item("i1", "t", b"b").unwrap().is_none(), "purged");
    assert!(lib.item("p2", "t", b"c").unwrap().is_some());
    // FF06: joined again, a scope begun before and never described begins a new period.
    let n1b = lib.join("h1", &device, false).unwrap();
    assert_ne!(n1b, n1, "a new term");
    lib.propose_on(&n1b, "p1", b"proposal".to_vec()).unwrap();
    assert!(lib.holding("p1").unwrap().is_some(), "the provisional scope is held again");
    commit(&lib, "p1", "d").unwrap();
    // One a hub described is held again only when a hub installs it.
    lib.propose_on(&n1b, "i1", b"proposal".to_vec()).unwrap();
    assert!(lib.holding("i1").unwrap().is_none(), "described before: not begun again by a proposal");
    lib.install_on(&n1b, "i1", b"descriptor".to_vec(), 3).unwrap();
    assert!(lib.holding("i1").unwrap().is_some());
    // A stop between: scopes of a network the library is not on, still held, are found.
    lib.leave("h2", &device).unwrap();
    assert!(lib.left_held().unwrap().is_empty());
    // Joining revokes what an earlier term left held (simulated by a hub-side install of h3's scope).
    lib.install("x", b"d".to_vec(), 1).unwrap();
    assert!(lib.left_held().unwrap().is_empty(), "a scope of no network (a hub's own) is never found as left");

    // E01: a scope installed is pulled whole once in each holding period; a description that says otherwise asks again.
    assert!(lib.unpulled_of("h1").unwrap().contains("i1"), "installed, not yet pulled whole");
    let h = lib.holding("i1").unwrap().unwrap();
    lib.pulled("i1", h).unwrap();
    assert!(!lib.unpulled_of("h1").unwrap().contains("i1"));
    lib.install_on(&n1b, "i1", b"descriptor".to_vec(), 3).unwrap();
    assert!(!lib.unpulled_of("h1").unwrap().contains("i1"), "the same descriptor: still pulled");
    lib.install_on(&n1b, "i1", b"descriptor2".to_vec(), 4).unwrap();
    assert!(lib.unpulled_of("h1").unwrap().contains("i1"), "described otherwise: pulled whole again");
    assert_eq!(lib.held_listing("i1").unwrap().map(|l| l.description).as_deref(), Some(&b"descriptor2"[..]));
    // HH04: what the hub says it may hold of the scope, changed alone, asks for it whole again.
    let h = lib.holding("i1").unwrap().unwrap();
    lib.allow("i1", "", "").unwrap();
    lib.pulled("i1", h).unwrap();
    lib.allow("i1", "", "").unwrap();
    assert!(!lib.unpulled_of("h1").unwrap().contains("i1"), "the same: still pulled");
    // AV01: who logged in here reads it, changed alone, pulls nothing and is kept.
    lib.allow("i1", "", "alice@1").unwrap();
    assert!(!lib.unpulled_of("h1").unwrap().contains("i1"), "readers changed alone: still pulled");
    assert_eq!(lib.held_listing("i1").unwrap().and_then(|l| l.readers).as_deref(), Some("alice@1"));
    lib.allow("i1", "logged", "alice@1").unwrap();
    assert!(lib.unpulled_of("h1").unwrap().contains("i1"), "may hold otherwise: pulled whole again");
    assert_eq!(lib.held_listing("i1").unwrap().and_then(|l| l.allowed).as_deref(), Some("logged"));
    // AJ04: a membership begun anew while the library has the network on starts a new term, what the old one held revoked.
    let before = lib.holding("i1").unwrap();
    let n1c = lib.join("h1", &device, true).unwrap();
    assert_ne!(n1c, n1b, "a new term");
    assert!(before.is_some() && lib.holding("i1").unwrap().is_none(), "the old term's scope revoked");
    // GG02: a writer pinned to a holding period writes nothing into a later one.
    lib.install_on(&n1c, "i1", b"descriptor2".to_vec(), 5).unwrap();
    assert!(lib.unpulled_of("h1").unwrap().contains("i1"), "held anew: pulled whole again");
    let new_h = lib.holding("i1").unwrap().unwrap();
    assert_ne!(Some(new_h), before);
    let w = fastctx_content::v2::Writer::<fastctx_content::v2::Timeline>::new(&lib, "i1", "timeline").unwrap().in_holding(before.unwrap());
    let write = fastctx_content::v2::Write { item: b"z".to_vec(), order: b"z".to_vec(), value: None, objects: vec![] };
    let refused = w.write_all(vec![write], vec![], false, signer.clone()).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(refused.contains("holding period"), "the old period's writer is refused: {refused}");
    // GG01: a library of an earlier layout is refused, named; one of this build's opens again.
    let old = base.join("old");
    std::fs::create_dir_all(&old).unwrap();
    {
        let c = fastctx_storage::rusqlite::Connection::open(old.join(fastctx_content::v2::FILE)).unwrap();
        c.execute_batch("CREATE TABLE stamps (name TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID; CREATE TABLE scopes (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE);").unwrap();
    }
    let refused = Library::open(&old).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(refused.contains("earlier layout"), "{refused}");
    let fresh = base.join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    drop(Library::open(&fresh).unwrap());
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(Library::open(&fresh).is_ok(), "this build's layout opens again");
    let _ = std::fs::remove_dir_all(&base);
    println!("networks drill ok");
}
