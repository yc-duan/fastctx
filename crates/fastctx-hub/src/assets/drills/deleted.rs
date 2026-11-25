//! A drill of a conversation's scope deleted with the conversation: its
//! holders told, what is asked of it answered as of one read no more, the
//! owner library let go of it, and the hub opened again knowing it deleted.
use crate::assets::*;
use fastctx_content::v2::{Commit, KindRecord, Library};
use fastctx_content::{Record, State};
use fastctx_net::directory::{Account, Kind, Level, Member};
use fastctx_net::sign::{Domain, SecretKey};
use fastctx_types::assets::{Frame, Push, Readers, Received, Reply, Request};
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
        was: Vec::new(),
        key: key.public(),
        kind: Kind::Computer,
        logins: [account.clone()].into_iter().collect(),
        home: Some(account.clone()),
        managed: false,
        code: None,
        group: None,
        direct: Vec::new(),
        relay: Vec::new(),
        added: jiff::Timestamp::now(),
        online: false,
        since: None,
        through: None,
        reach: None,
        paths: None,
        strain: None,
        workbench: None,
        logins_number: None,
        settled: None,
    })
}

#[test]
fn a_scope_goes_with_its_conversation() {
    let base = std::env::temp_dir().join(format!("hub-deleted-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = AccountName::try_from("root".to_string()).unwrap();
    let bob = AccountName::try_from("bob".to_string()).unwrap();
    crate::init(&dir, crate::New {
        name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(),
        public: vec![address],
        listen: address,
        admin: root.clone(),
        password: "a long enough password 123".into(),
    })
    .unwrap();
    let mut store = crate::store::Store::open_logged(&dir, None).unwrap();
    let assets = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    let (k1, k2) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
    store.book.directory.accounts.push(Arc::new(Account {
        name: bob.clone(), level: Level::Member, locked: false, opened: jiff::Timestamp::now(), collab: None, history: false, profile: None, promotion: false,
    }));
    store.book.directory.members.push(member("m1", &k1, &root));
    store.book.directory.members.push(member("m2", &k2, &bob));
    // m1 shared whole to bob: his machine reads, and holds, what is born there.
    let (m1, m2) = (MachineName::try_from("m1".to_string()).unwrap(), MachineName::try_from("m2".to_string()).unwrap());
    store.book.directory.shares.push(Arc::new(fastctx_net::directory::Share { machine: m1.clone(), account: bob.clone(), jobs: None, pending: None, basis: None }));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let ask = |assets: &Assets, store: &crate::store::Store, machine: &MachineName, key: &SecretKey, request: Request| {
        serve::ask(assets, &store.indexed(), &String::new, machine, &key.public(), request)
    };
    let classes = fastctx_types::assets::conversation_classes();
    // Two conversations of one id, each born on its own machine.
    let conv1 = format!("c/{}/m1/claude-1a2b", assets.id);
    let conv2 = format!("c/{}/m2/claude-1a2b", assets.id);
    let describe = |key: &str, opener: &AccountName, born: &MachineName| Request::Describe {
        scope: key.into(),
        readers: Readers::Conversation { openers: vec![opener.clone()], born_on: Some(born.clone()) },
        classes: classes.clone(),
        born: Default::default(),
        begun: None,
    };
    let Reply::Described { descriptor } = ask(&assets, &store, &m1, &k1, describe(&conv1, &root, &m1)).unwrap() else { panic!("described") };
    assert!(matches!(ask(&assets, &store, &m2, &k2, describe(&conv2, &bob, &m2)).unwrap(), Reply::Described { .. }));

    // m1 commits a record, the hub takes it and checkpoints it.
    let producer_dir = base.join("m1-assets");
    fastctx_platform::disk::create_dir_all(&producer_dir).unwrap();
    let (producer, _) = Library::open(&producer_dir).unwrap();
    producer.install(&conv1, descriptor.fact.0.clone(), 1).unwrap();
    let record = KindRecord { kind: 1, schema: 1, record: Record { item: b"a".to_vec(), order: b"a".to_vec(), state: State::Is { body: b"hello".to_vec(), objects: vec![] } } };
    producer.raw_commit(Commit { scope: conv1.clone(), stream: "timeline".into(), records: vec![record], leases: vec![], durable: true, op: None, holding: producer.holding(&conv1).unwrap().unwrap() }, Arc::new(Machine(SecretKey::from_seed(&k1.seed())))).unwrap();
    let raw = producer.export(&conv1, "timeline", 0, 1).unwrap().remove(0);
    let frame = Frame { header: raw.header.clone().into(), signature: raw.signature.clone().into(), body: raw.body.clone().into(), apart: 0 };
    let take = Request::Take { scope: conv1.clone(), frame: frame.clone(), proven: vec![], staged: 0 };
    assert!(matches!(ask(&assets, &store, &m1, &k1, take.clone()).unwrap(), Reply::Received { received: Received::Applied { change: 1, .. } }));
    assets.checkpoint().unwrap().expect("a checkpoint");
    let frames = |store: &crate::store::Store, machine: &MachineName, key: &SecretKey, scope: &str| {
        ask(&assets, store, machine, key, Request::Frames { scope: scope.into(), stream: "timeline".into(), from: 0, limit: 10 })
    };
    // m2 lists what it may hold: the hub knows it as a holder of the scope.
    let Reply::Holdings { scopes: listed, .. } = ask(&assets, &store, &m2, &k2, Request::Holdings { after: None }).unwrap() else { panic!("holdings") };
    assert!(listed.iter().any(|held| held.scope == conv1), "m2 may hold it");
    assert!(matches!(frames(&store, &m2, &k2, &conv1).unwrap(), Reply::Frames { .. }));
    let _ = assets.take_pushes(assets.drain_pushes(), &store.indexed());

    // The conversation on m1 is deleted: its scope alone is found, and goes.
    let conversation: fastctx_types::Address = serde_json::from_value(serde_json::json!({ "machine": "m1", "resident": "claude-1a2b" })).unwrap();
    let found = deleted::of_conversation(&assets, &store.indexed(), &conversation).unwrap();
    assert_eq!(found, vec![conv1.clone()], "never the other machine's of the same id");
    let owner = scopes::scope(&assets, &conv1).unwrap().unwrap().owner;
    assert!(assets.delete_scope(&conv1).unwrap());
    assert!(!assets.delete_scope(&conv1).unwrap(), "deleted once");
    let pushes = assets.take_pushes(assets.drain_pushes(), &store.indexed());
    assert!(pushes.iter().any(|(machine, push)| machine == &m2 && matches!(push, Push::Revoked { scope } if *scope == conv1)), "{pushes:?}");
    assert!(scopes::scope(&assets, &conv1).unwrap().is_none());
    assert!(assets.was_deleted(&conv1) && !assets.was_deleted(&conv2));
    // Whatever is asked of it, it is read no more; described or taken into again, it does not come back.
    assert!(matches!(frames(&store, &m1, &k1, &conv1).unwrap(), Reply::Revoked { .. }));
    assert!(matches!(frames(&store, &m2, &k2, &conv1).unwrap(), Reply::Revoked { .. }));
    assert!(matches!(ask(&assets, &store, &m1, &k1, describe(&conv1, &root, &m1)).unwrap(), Reply::Revoked { .. }));
    assert!(matches!(ask(&assets, &store, &m1, &k1, take).unwrap(), Reply::Revoked { .. }));
    let Reply::Holdings { scopes: listed, .. } = ask(&assets, &store, &m2, &k2, Request::Holdings { after: None }).unwrap() else { panic!("holdings") };
    assert!(!listed.iter().any(|held| held.scope == conv1), "listed to none");
    // The owner library lets go of what it kept of it.
    for _ in 0..8 {
        assets.let_go();
    }
    let library = assets.library(&owner).unwrap();
    assert!(!library.purging().unwrap(), "all of it let go");
    assert!(library.holding(&conv1).unwrap().is_none());
    assert!(library.export(&conv1, "timeline", 0, 10).map(|frames| frames.is_empty()).unwrap_or(true), "no frame of it left");
    // The other conversation of the same id is as it was.
    assert!(scopes::scope(&assets, &conv2).unwrap().is_some());
    // Checkpoints go on, and the hub opened again finds its library whole and the scope deleted.
    assets.checkpoint().unwrap();
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    drop(library);
    let id = assets.id.clone();
    drop(assets);
    let assets = Assets::open(&dir, id.as_str(), store.shelf.0.clone()).unwrap();
    assets.library(&owner).unwrap();
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    assert!(assets.was_deleted(&conv1), "kept by the chain");
    assert!(matches!(ask(&assets, &store, &m1, &k1, Request::Frames { scope: conv1.clone(), stream: "timeline".into(), from: 0, limit: 10 }).unwrap(), Reply::Revoked { .. }));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
}
