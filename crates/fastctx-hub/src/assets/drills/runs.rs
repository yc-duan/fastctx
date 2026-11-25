//! A drill of the first epoch of a sentinel's wakes (registry 9).
use crate::assets::*;
use fastctx_content::v2::{Commit, ConversationSentinel, Kind as _, KindRecord, Library};
use fastctx_content::{Record, State};
use fastctx_net::directory::{Kind, Member};
use fastctx_net::sign::{Domain, SecretKey, heard_word};
use fastctx_types::assets::{Frame, Readers, Received, Reply, Request};
use fastctx_types::{AccountName, MachineName};

struct Machine(SecretKey);
impl fastctx_content::v2::Signer for Machine {
    fn device(&self) -> [u8; 32] { *self.0.public().as_bytes() }
    fn key(&self) -> [u8; 32] { *self.0.public().as_bytes() }
    fn sign(&self, header: &[u8]) -> Vec<u8> { self.0.sign(Domain::SyncSourceBatch, header).to_bytes().to_vec() }
}

fn member(name: &str, key: &SecretKey, account: &AccountName) -> Arc<Member> {
    Arc::new(Member {
        name: MachineName::try_from(name.to_string()).unwrap(),
        was: Vec::new(), key: key.public(), kind: Kind::Computer, logins: [account.clone()].into_iter().collect(),
        home: Some(account.clone()), managed: false, code: None, group: None, direct: Vec::new(), relay: Vec::new(),
        added: jiff::Timestamp::now(), online: false, since: None, through: None, reach: None, paths: None, strain: None,
        workbench: None, settled: None,
        logins_number: None,
    })
}

fn hex8(key: &SecretKey) -> String {
    key.public().as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn sentinel_runs_begun_by_a_home() {
    let base = std::env::temp_dir().join(format!("hub-runs-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = AccountName::try_from("root".to_string()).unwrap();
    crate::init(&dir, crate::New {
        name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(),
        public: vec![address], listen: address, admin: root.clone(), password: "a long enough password 123".into(),
    })
    .unwrap();
    let mut store = crate::store::Store::open_logged(&dir, None).unwrap();
    let assets = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    let (k1, k2) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
    store.book.directory.members.push(member("m1", &k1, &root));
    store.book.directory.members.push(member("m2", &k2, &root));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let (m1, m2) = (MachineName::try_from("m1".to_string()).unwrap(), MachineName::try_from("m2".to_string()).unwrap());
    let ask = |store: &crate::store::Store, machine: &MachineName, key: &SecretKey, request: Request| {
        serve::ask(&assets, &store.indexed(), &String::new, machine, &key.public(), request)
    };
    let conv = format!("c/{}/m1/conv-1", assets.id);
    let conv = conv.as_str();
    let readers = Readers::Conversation { openers: vec![root.clone()], born_on: Some(m1.clone()) };
    let describe = Request::Describe { scope: conv.into(), readers, classes: fastctx_types::assets::conversation_classes(), born: Default::default(), begun: None };
    let Reply::Described { descriptor } = ask(&store, &m1, &k1, describe).unwrap() else { panic!("described") };
    let (seq0, head0) = heard_word(&assets.public(), &descriptor.fact.0, &descriptor.word.0).unwrap();
    let genesis = fastctx_types::assets::Seen { seq: seq0, head: head0.into(), statement: descriptor.fact.0.clone().into(), word: descriptor.word.0.clone().into() };
    for (machine, key) in [(&m1, &k1), (&m2, &k2)] {
        let open = Request::Open { version: 1, seen: Some(genesis.clone()), reported: vec![], pinned: None, lineage: vec![] };
        assert!(matches!(ask(&store, machine, key, open).unwrap(), Reply::Ready { .. }), "ready");
    }

    // The conversation's one sentinel lives on m2, as the hub orders its record into the scope.
    let kept = scopes::scope(&assets, conv).unwrap().unwrap();
    let library = scopes::library(&assets, &kept).unwrap();
    let sentinel = serde_json::json!({
        "metadata": { "kind": "sentinel", "key": "s-1", "version": 1, "by": "m1", "at": "2026-11-13T00:00:00Z", "deleted": false },
        "body": { "scope": "root", "active": true, "definition": { "id": "s-1", "home": "m2", "owner": { "resident": "conv-1", "machine": "m1" } } },
    });
    let record = KindRecord {
        kind: ConversationSentinel::KIND,
        schema: 1,
        record: Record { item: b"s-1".to_vec(), order: b"s-1".to_vec(), state: State::Is { body: serde_json::to_vec(&sentinel).unwrap(), objects: vec![] } },
    };
    mirror::apply(&assets, &kept, &library, ConversationSentinel::STREAM, vec![record]).unwrap();
    // Kept as a home, as the hub keeps each home a sentinel record names; then the sentinel moves on to m9.
    sentinel_homes::record(&assets, conv, "m2").unwrap();
    let moved = serde_json::json!({
        "metadata": { "kind": "sentinel", "key": "s-1", "version": 2, "by": "m1", "at": "2026-11-13T00:00:01Z", "deleted": false },
        "body": { "scope": "root", "active": true, "definition": { "id": "s-1", "home": "m9", "owner": { "resident": "conv-1", "machine": "m1" } } },
    });
    let moved = KindRecord {
        kind: ConversationSentinel::KIND,
        schema: 1,
        record: Record { item: b"s-1".to_vec(), order: b"s-1".to_vec(), state: State::Is { body: serde_json::to_vec(&moved).unwrap(), objects: vec![] } },
    };
    mirror::apply(&assets, &kept, &library, ConversationSentinel::STREAM, vec![moved]).unwrap();

    // A device's first frame of a wakes' stream, its one wake.
    let frame = |name: &str, key: &SecretKey, stream: &str| {
        let at = base.join(name);
        fastctx_platform::disk::create_dir_all(&at).unwrap();
        let (producer, _) = Library::open(&at).unwrap();
        producer.install(conv, descriptor.fact.0.clone(), 1).unwrap();
        let state = producer.put(&mut &b"{\"sequence\":1}"[..], "drill", std::time::Duration::from_secs(600)).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({ "sentinel": "s-1", "generation": 1, "sequence": 1 })).unwrap();
        let item = fastctx_content::v2::item_of("s-1/1/00000000000000000001");
        let record = KindRecord { kind: 9, schema: 1, record: Record { item: item.clone(), order: item, state: State::Is { body, objects: vec![state.object] } } };
        producer.raw_commit(Commit { scope: conv.into(), stream: stream.into(), records: vec![record], leases: vec![state], durable: true, op: None, holding: producer.holding(conv).unwrap().unwrap() }, Arc::new(Machine(SecretKey::from_seed(&key.seed())))).unwrap();
        let raw = producer.export(conv, stream, 0, 1).unwrap().remove(0);
        Request::Take { scope: conv.into(), frame: Frame { header: raw.header.clone().into(), signature: raw.signature.clone().into(), body: raw.body.clone().into(), apart: 0 }, proven: vec![], staged: 0 }
    };
    // m1 has no sentinel of the conversation living on it: it begins no wakes' stream, though the conversation lives on it.
    let ones = format!("sentinel-runs/{}", hex8(&k1));
    let refused = ask(&store, &m1, &k1, frame("m1-own", &k1, &ones));
    assert!(refused.is_err(), "no sentinel lives on m1: {refused:?}");
    println!("no sentinel there: ok");
    // m1 cannot write m2's stream.
    let twos = format!("sentinel-runs/{}", hex8(&k2));
    let refused = ask(&store, &m1, &k1, frame("m1-theirs", &k1, &twos));
    assert!(refused.is_err(), "another machine's stream: {refused:?}");
    println!("another's stream: ok");
    // m2, where the sentinel lives, cannot begin m1's either.
    let refused = ask(&store, &m2, &k2, frame("m2-theirs", &k2, &ones));
    assert!(refused.is_err(), "the home writes its own stream alone: {refused:?}");
    println!("the home's own alone: ok");
    // m2, where the sentinel lived before it moved on, begins its own: taken, or asked for the wake's objects first.
    let taken = ask(&store, &m2, &k2, frame("m2-own", &k2, &twos)).unwrap();
    assert!(
        matches!(taken, Reply::Received { received: Received::Applied { .. } | Received::Missing { .. } | Received::Prove { .. } }),
        "the sentinel's home begins it: {taken:?}"
    );
    println!("a former home still begins its own: ok");
    println!("sentinel runs drill: ok");
}
