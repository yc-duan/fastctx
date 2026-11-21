//! A drill that a stream ordered nowhere yet is begun on the machine of the
//! account adopting it (design §29, 2.3, NN14).
use crate::assets::*;
use fastctx_content::v2::{Commit, EpochFact, HubVerifier, KindRecord, Library, Signed, Taken};
use fastctx_content::{Record, State};
use fastctx_net::directory::{Kind, Member};
use fastctx_net::sign::{Domain, PublicKey, SecretKey, heard_word};
use fastctx_types::assets::{Frame, Readers, Received, Reply, Request};
use fastctx_types::{AccountName, MachineName};

struct Machine(SecretKey);
impl fastctx_content::v2::Signer for Machine {
    fn device(&self) -> [u8; 32] { *self.0.public().as_bytes() }
    fn key(&self) -> [u8; 32] { *self.0.public().as_bytes() }
    fn sign(&self, header: &[u8]) -> Vec<u8> { self.0.sign(Domain::SyncSourceBatch, header).to_bytes().to_vec() }
}

struct HubWord(PublicKey);
impl HubVerifier for HubWord {
    fn verify(&self, fact: &[u8], signature: &[u8]) -> bool { heard_word(&self.0, fact, signature).is_some() }
}

fn member(name: &str, key: &SecretKey, account: &AccountName) -> Arc<Member> {
    Arc::new(Member {
        name: MachineName::try_from(name.to_string()).unwrap(), was: Vec::new(), key: key.public(), kind: Kind::Computer,
        logins: [account.clone()].into_iter().collect(), home: Some(account.clone()), managed: false, code: None, group: None,
        direct: Vec::new(), relay: Vec::new(), added: jiff::Timestamp::now(), online: false, since: None, through: None,
        reach: None, paths: None, strain: None, workbench: None, settled: None,
    })
}

fn first_frame(dir: &std::path::Path, scope: &str, descriptor: &[u8], machine: &SecretKey, adopted: Option<Signed>, hub: &PublicKey) -> Frame {
    fastctx_platform::disk::create_dir_all(dir).unwrap();
    let (library, _) = Library::open(dir).unwrap();
    library.install(scope, descriptor.to_vec(), 1).unwrap();
    if let Some(signed) = adopted {
        assert_eq!(library.take_over(scope, signed, &HubWord(*hub)).unwrap(), Taken::Opened, "the hub's first epoch opened here");
    }
    let record = KindRecord { kind: 1, schema: 1, record: Record { item: b"a".to_vec(), order: b"a".to_vec(), state: State::Is { body: b"hello".to_vec(), objects: vec![] } } };
    let holding = library.holding(scope).unwrap().unwrap();
    library
        .raw_commit(Commit { scope: scope.into(), stream: "timeline".into(), records: vec![record], leases: vec![], durable: true, op: None, holding }, Arc::new(Machine(SecretKey::from_seed(&machine.seed()))))
        .unwrap();
    let raw = library.export(scope, "timeline", 0, 1).unwrap().remove(0);
    Frame { header: raw.header.into(), signature: raw.signature.into(), body: raw.body.into(), apart: 0 }
}

#[test]
fn adopting_a_stream_ordered_nowhere() {
    let base = std::env::temp_dir().join(format!("hub-adopt-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = AccountName::try_from("root".to_string()).unwrap();
    crate::init(&dir, crate::New { name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(), public: vec![address], listen: address, admin: root.clone(), password: "a long enough password 123".into() }).unwrap();
    let mut store = crate::store::Store::open_logged(&dir, None).unwrap();
    let assets = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    let (k1, k3) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
    store.book.directory.members.push(member("m1", &k1, &root));
    store.book.directory.members.push(member("m3", &k3, &root));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let (m1, m3) = (MachineName::try_from("m1".to_string()).unwrap(), MachineName::try_from("m3".to_string()).unwrap());
    let ask = |store: &crate::store::Store, machine: &MachineName, key: &SecretKey, request: Request| {
        serve::ask(&assets, &store.indexed(), &String::new, machine, &key.public(), request)
    };
    // Described on m1, where it was born; nothing of it ordered anywhere yet.
    let scope = format!("c/{}/m1/conv-9", assets.id);
    let readers = Readers::Conversation { openers: vec![root.clone()], born_on: Some(m1.clone()) };
    let describe = Request::Describe { scope: scope.clone(), readers, classes: fastctx_types::assets::conversation_classes(), born: Default::default(), begun: None };
    let Reply::Described { descriptor } = ask(&store, &m1, &k1, describe).unwrap() else { panic!("described") };
    // root adopts it on m3: the hub begins the timeline there, its first epoch m3's.
    let adopt = Request::Takeover { scope: scope.clone(), stream: "timeline".into(), release: None, adopt: true };
    let Reply::Epoch { descriptor: epoch } = ask(&store, &m3, &k3, adopt.clone()).unwrap() else { panic!("an epoch for the adopter") };
    let fact = EpochFact::decode(&epoch.fact.0).unwrap();
    assert_eq!((fact.epoch, fact.device), (0, *k3.public().as_bytes()), "the first epoch, m3's");
    let Reply::Epoch { descriptor: again } = ask(&store, &m3, &k3, adopt).unwrap() else { panic!("asked again") };
    assert_eq!(again.fact, epoch.fact, "asked again, the same epoch");
    // m1's own first frame, begun where it was born before it heard: not taken into a stream begun on m3.
    let theirs = first_frame(&base.join("m1"), &scope, &descriptor.fact.0, &k1, None, &assets.public());
    let answer = ask(&store, &m1, &k1, Request::Take { scope: scope.clone(), frame: theirs, proven: vec![], staged: 0 });
    assert!(!matches!(answer, Ok(Reply::Received { received: Received::Applied { .. } })), "m1's frame refused: {answer:?}");
    // m3's first frame under the hub's epoch: taken.
    let signed = Signed { fact: epoch.fact.0.clone(), signature: epoch.word.0.clone() };
    let mine = first_frame(&base.join("m3"), &scope, &descriptor.fact.0, &k3, Some(signed), &assets.public());
    let answer = ask(&store, &m3, &k3, Request::Take { scope: scope.clone(), frame: mine, proven: vec![], staged: 0 }).unwrap();
    assert!(matches!(answer, Reply::Received { received: Received::Applied { .. } }), "m3's frame taken: {answer:?}");
    // codex-5c30d5e9 A02: bob opens a conversation on m1, shared whole with him; bob is deleted and an account made
    // again under his name, given m1 whole too, is a member of its body now and reads it — but did not open it, and
    // adopts none of its streams from a machine of his.
    store.bodies = Some(assets.bodies.clone());
    let bob = AccountName::try_from("bob".to_string()).unwrap();
    let account = |opened: jiff::Timestamp| {
        Arc::new(fastctx_net::directory::Account {
            name: bob.clone(), level: fastctx_net::directory::Level::Member, locked: false, opened, collab: None, history: false,
            profile: None, promotion: false,
        })
    };
    let share = || Arc::new(fastctx_net::directory::Share { machine: m1.clone(), account: bob.clone(), jobs: None, pending: None, basis: None });
    let k4 = SecretKey::generate().unwrap();
    let m4 = MachineName::try_from("m4".to_string()).unwrap();
    let first = jiff::Timestamp::now();
    store.book.directory.accounts.push(account(first));
    store.book.directory.members.push(member("m4", &k4, &bob));
    store.book.directory.shares.push(share());
    // bob logged in on m1, shared whole with him: the machine acts for him.
    let at_m1 = store.book.directory.members.iter().position(|member| member.name == m1).unwrap();
    let mut on_m1 = (*store.book.directory.members[at_m1]).clone();
    on_m1.logins.insert(bob.clone());
    store.book.directory.members[at_m1] = Arc::new(on_m1);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let body = bodies::body_of(&store.book.directory, &m1).unwrap();
    let bobs = format!("b/{body}/conv-20");
    let readers = Readers::Body { body: body.clone(), opener: bob.clone() };
    let describe = Request::Describe { scope: bobs.clone(), readers, classes: fastctx_types::assets::conversation_classes(), born: Default::default(), begun: None };
    let Reply::Described { .. } = ask(&store, &m1, &k1, describe).unwrap() else { panic!("bob's, described on m1") };
    store.book.directory.shares.retain(|share| share.account != bob);
    store.book.directory.accounts.retain(|account| account.name != bob);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    store.book.directory.accounts.push(account(first.checked_add(jiff::SignedDuration::from_secs(60)).unwrap()));
    store.book.directory.shares.push(share());
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(matches!(ask(&store, &m4, &k4, Request::Streams { scope: bobs.clone() }).unwrap(), Reply::Streams { .. }), "the new bob, a member now, reads it");
    let take_back = Request::Takeover { scope: bobs.clone(), stream: "timeline".into(), release: None, adopt: true };
    let refused = ask(&store, &m4, &k4, take_back);
    assert!(!matches!(refused, Ok(Reply::Epoch { .. })), "an account made again under its name adopts nothing: {refused:?}");
    // Refused by the opener's identity, not before it by the read gate: the case this guards.
    assert!(format!("{refused:?}").contains("opened by no account logged in"), "refused where the opener is judged: {refused:?}");
    // codex-5c30d5e9 B07: the machine it was born on gone from the network, another machine's first frame does not
    // begin it; the account that opened it adopts it there, and then it does.
    let gone_scope = format!("c/{}/m1/conv-11", assets.id);
    let readers = Readers::Conversation { openers: vec![root.clone()], born_on: Some(m1.clone()) };
    let describe = Request::Describe { scope: gone_scope.clone(), readers, classes: fastctx_types::assets::conversation_classes(), born: Default::default(), begun: None };
    let Reply::Described { descriptor: gone_descriptor } = ask(&store, &m1, &k1, describe).unwrap() else { panic!("described") };
    store.book.directory.members.retain(|member| member.name != m1);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let unasked = first_frame(&base.join("m3-unasked"), &gone_scope, &gone_descriptor.fact.0, &k3, None, &assets.public());
    let answer = ask(&store, &m3, &k3, Request::Take { scope: gone_scope.clone(), frame: unasked, proven: vec![], staged: 0 });
    assert!(!matches!(answer, Ok(Reply::Received { received: Received::Applied { .. } })), "a first frame with its machine gone, unadopted: {answer:?}");
    let adopt = Request::Takeover { scope: gone_scope.clone(), stream: "timeline".into(), release: None, adopt: true };
    let Reply::Epoch { descriptor: epoch } = ask(&store, &m3, &k3, adopt).unwrap() else { panic!("adopted with its machine gone") };
    let signed = Signed { fact: epoch.fact.0.clone(), signature: epoch.word.0.clone() };
    let adopted = first_frame(&base.join("m3-adopted"), &gone_scope, &gone_descriptor.fact.0, &k3, Some(signed), &assets.public());
    let answer = ask(&store, &m3, &k3, Request::Take { scope: gone_scope.clone(), frame: adopted, proven: vec![], staged: 0 }).unwrap();
    assert!(matches!(answer, Reply::Received { received: Received::Applied { .. } }), "taken once adopted: {answer:?}");
    drop((store, assets));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    println!("adopt drill: ok");
}
