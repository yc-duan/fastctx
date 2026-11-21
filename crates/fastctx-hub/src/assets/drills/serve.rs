//! A drill of the hub's asset requests behind the read gate.
use crate::assets::*;
use fastctx_content::v2::{Commit, FrameKey, Header, KindRecord, Library, applied_statement};
use fastctx_content::{Record, State};
use fastctx_net::directory::{Account, Kind, Level, Member};
use fastctx_net::sign::{Domain, SecretKey, heard_word};
use fastctx_types::assets::{Cursor, Frame, Readers, Received, Reply, Reported, Request, Said};
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
        settled: None,
    })
}

#[test]
fn asks_behind_the_gate() {
    let base = std::env::temp_dir().join(format!("hub-serve-drill-{}", std::process::id()));
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
    let accounts = &mut store.book.directory.accounts;
    accounts.push(Arc::new(Account {
        name: bob.clone(), level: Level::Member, locked: false, opened: jiff::Timestamp::now(), collab: None, history: false, profile: None, promotion: false,
    }));
    store.book.directory.members.push(member("m1", &k1, &root));
    store.book.directory.members.push(member("m2", &k2, &bob));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let (m1, m2) = (MachineName::try_from("m1".to_string()).unwrap(), MachineName::try_from("m2".to_string()).unwrap());
    let ask = |store: &crate::store::Store, machine: &MachineName, key: &SecretKey, request: Request| {
        serve::ask(&assets, &store.indexed(), &String::new, machine, &key.public(), request)
    };
    let refused = |answer: Result<Reply, String>| answer.is_err() || matches!(answer, Ok(Reply::Revoked { .. }));
    // An account's scopes are named under the hub's id (BB11).
    let own = |account: &str, name: &str| format!("a/{}/{account}/{name}", assets.id);
    // A conversation's scope is named under the hub (§29, 2.8).
    let conv1 = format!("c/{}/m1/conv-1", assets.id);
    let conv1 = conv1.as_str();

    // A conversation born on m1, opened by root: m1 describes it; m2, bob's, cannot.
    let readers = Readers::Conversation { openers: vec![root.clone()], born_on: Some(m1.clone()) };
    let classes = fastctx_types::assets::conversation_classes();
    let describe = Request::Describe { scope: conv1.into(), readers: readers.clone(), classes: classes.clone(), born: Default::default() };
    let Reply::Described { descriptor } = ask(&store, &m1, &k1, describe.clone()).unwrap() else { panic!("described") };
    assert!(heard_word(&assets.public(), &descriptor.fact.0, &descriptor.word.0).is_some(), "the hub's word");
    let Reply::Described { descriptor: again } = ask(&store, &m1, &k1, describe).unwrap() else { panic!() };
    assert_eq!(again, descriptor, "the same description again is the same descriptor");
    let bobs = Request::Describe { scope: conv1.into(), readers: Readers::Conversation { openers: vec![bob.clone()], born_on: None }, classes: classes.clone(), born: Default::default() };
    assert!(ask(&store, &m2, &k2, bobs).is_err(), "a key under another's prefix");
    let theirs = Request::Describe { scope: conv1.into(), readers: readers.clone(), classes: classes.clone(), born: Default::default() };
    assert!(ask(&store, &m2, &k2, theirs).is_err(), "only the machine it was born on describes it");
    let unprefixed = Request::Describe { scope: "conv-2".into(), readers: readers.clone(), classes: classes.clone(), born: Default::default() };
    assert!(ask(&store, &m1, &k1, unprefixed).is_err(), "a key under its readers' prefix");
    let read: fastctx_types::assets::Descriptor = serde_json::from_slice(&descriptor.fact.0).unwrap();
    assert_eq!(read.hub, store.hub.id.as_str());
    assert_eq!(read.born.len(), 2, "{:?}", read.born);
    // Begun for an earlier account of root's name: never described for the
    // one there now; begun for that one, it is.
    let root_now = read.born["account:root"].clone();
    let conv3 = format!("c/{}/m1/conv-3", assets.id);
    let begun = |opened: &str| Request::Describe {
        scope: conv3.clone(),
        readers: readers.clone(),
        classes: classes.clone(),
        born: [("account:root".to_string(), opened.to_string())].into_iter().collect(),
    };
    let earlier = ask(&store, &m1, &k1, begun("1")).unwrap_err();
    assert!(earlier.contains(fastctx_types::assets::BEGUN_FOR_ANOTHER), "{earlier}");
    assert!(matches!(ask(&store, &m1, &k1, begun(&root_now)).unwrap(), Reply::Described { .. }), "begun for the one there now");
    // An account's own scope: the cached answer passes the read gate too.
    let roots = Request::Describe { scope: own("root", "settings"), readers: Readers::Account { account: root.clone() }, classes: fastctx_types::assets::records_classes(), born: Default::default() };
    assert!(matches!(ask(&store, &m1, &k1, roots.clone()).unwrap(), Reply::Described { .. }));
    assert!(ask(&store, &m2, &k2, roots).is_err(), "bob's machine is not given root's descriptor");
    // A first word bound to the chain's start opens a session; it seals nothing.
    let (seq0, head0) = heard_word(&assets.public(), &descriptor.fact.0, &descriptor.word.0).unwrap();
    let genesis = fastctx_types::assets::Seen { seq: seq0, head: head0.into(), statement: descriptor.fact.0.clone().into(), word: descriptor.word.0.clone().into() };
    let Reply::Ready { .. } = ask(&store, &m1, &k1, Request::Open { version: 1, seen: Some(genesis), reported: vec![], pinned: None, lineage: vec![] }).unwrap() else { panic!("ready at the start") };

    // m1 commits a record and the hub takes it; bob's machine cannot take into the scope.
    let producer_dir = base.join("m1-assets");
    fastctx_platform::disk::create_dir_all(&producer_dir).unwrap();
    let (producer, _) = Library::open(&producer_dir).unwrap();
    producer.install(conv1, descriptor.fact.0.clone(), 1).unwrap();
    let record = KindRecord { kind: 1, schema: 1, record: Record { item: b"a".to_vec(), order: b"a".to_vec(), state: State::Is { body: b"hello".to_vec(), objects: vec![] } } };
    producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records: vec![record], leases: vec![], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, Arc::new(Machine(SecretKey::from_seed(&k1.seed())))).unwrap();
    let raw = producer.export(conv1, "timeline", 0, 1).unwrap().remove(0);
    let frame = Frame { header: raw.header.clone().into(), signature: raw.signature.clone().into(), body: raw.body.clone().into(), apart: 0 };
    let take = Request::Take { scope: conv1.into(), frame: frame.clone(), proven: vec![], staged: 0 };
    assert!(refused(ask(&store, &m2, &k2, take.clone())), "bob reads not the scope");
    let Reply::Received { received: Received::Applied { change, prefix, word, .. } } = ask(&store, &m1, &k1, take.clone()).unwrap() else { panic!("applied") };
    assert_eq!(change, 1);
    let mut forged = frame.clone();
    forged.signature = vec![0u8; 64].into();
    assert!(ask(&store, &m1, &k1, Request::Take { scope: conv1.into(), frame: forged, proven: vec![], staged: 0 }).is_err(), "only the device's own signature");

    // The session opens: the receipt checks; once a checkpoint covers it, the hub says so with its word.
    let header = Header::decode(&raw.header).unwrap();
    let reported = Reported { scope: conv1.into(), stream: "timeline".into(), epoch: 0, first: 0, digest: header.digest.into(), said: Said::Applied { change, prefix: prefix.clone() }, word: word.clone() };
    let open = |reported: Vec<Reported>| Request::Open { version: 1, seen: None, reported, pinned: None, lineage: vec![] };
    let Reply::Ready { checkpointed, .. } = ask(&store, &m1, &k1, open(vec![reported.clone()])).unwrap() else { panic!("ready") };
    assert!(checkpointed.is_empty(), "no checkpoint yet");
    assets.checkpoint().unwrap().expect("a checkpoint");
    let Reply::Ready { checkpointed, seq, .. } = ask(&store, &m1, &k1, open(vec![reported.clone()])).unwrap() else { panic!("ready") };
    assert_eq!(checkpointed.len(), 1);
    let covered = &checkpointed[0];
    let head = covered.head.array().unwrap();
    assert!(heard_word(&assets.public(), &fastctx_types::assets::checkpoint_statement(conv1, covered.change, covered.parked, covered.seq, &head), &covered.word.0).is_some());
    assert!(covered.seq <= seq);

    // Frames, cursors and succession go to a device that holds the stream.
    let Reply::Frames { frames, .. } = ask(&store, &m1, &k1, Request::Frames { scope: conv1.into(), stream: "timeline".into(), from: 0, limit: 10 }).unwrap() else { panic!() };
    assert_eq!(frames, vec![frame.clone()]);
    assert!(refused(ask(&store, &m2, &k2, Request::Frames { scope: conv1.into(), stream: "timeline".into(), from: 0, limit: 10 })));
    let cursor = Cursor { stream: "timeline".into(), sequence: 0, head: header.chain().into() };
    let Reply::Standing { streams } = ask(&store, &m1, &k1, Request::Cursors { scope: conv1.into(), holding: 1, streams: vec![cursor] }).unwrap() else { panic!() };
    assert!(streams[0].agrees);
    let Reply::Succession { epochs, gaps } = ask(&store, &m1, &k1, Request::Succession { scope: conv1.into(), stream: "timeline".into() }).unwrap() else { panic!() };
    assert_eq!((epochs.len(), gaps.len()), (1, 0));

    // Sharing m1 whole to bob makes the conversation bob's to read too, from his machine.
    store.book.directory.shares.push(Arc::new(fastctx_net::directory::Share { machine: m1.clone(), account: bob.clone(), jobs: None, pending: None, basis: None }));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let Reply::Frames { frames, .. } = ask(&store, &m2, &k2, Request::Frames { scope: conv1.into(), stream: "timeline".into(), from: 0, limit: 10 }).unwrap() else { panic!() };
    assert_eq!(frames.len(), 1, "read as the directory stands now");
    // Pushes: a frame taken from m1 is told to m2, which reads the scope; a checkpoint to m1, given a receipt.
    let _ = assets.take_pushes(assets.drain_pushes(), &store.indexed());
    let record2 = KindRecord { kind: 1, schema: 1, record: Record { item: b"b".to_vec(), order: b"b".to_vec(), state: State::Is { body: b"again".to_vec(), objects: vec![] } } };
    producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records: vec![record2], leases: vec![], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, Arc::new(Machine(SecretKey::from_seed(&k1.seed())))).unwrap();
    let raw2 = producer.export(conv1, "timeline", 1, 1).unwrap().remove(0);
    let frame2 = Frame { header: raw2.header.into(), signature: raw2.signature.into(), body: raw2.body.into(), apart: 0 };
    assert!(matches!(ask(&store, &m1, &k1, Request::Take { scope: conv1.into(), frame: frame2, proven: vec![], staged: 0 }).unwrap(), Reply::Received { received: Received::Applied { change: 2, .. } }));
    let pushes = assets.take_pushes(assets.drain_pushes(), &store.indexed());
    assert!(pushes.iter().any(|(machine, push)| machine == &m2 && matches!(push, fastctx_types::assets::Push::Moved { next: 2, .. })), "{pushes:?}");
    assert!(!pushes.iter().any(|(machine, _)| machine == &m1), "not to the machine that sent it");
    assets.checkpoint().unwrap().expect("a checkpoint");
    let pushes = assets.take_pushes(assets.drain_pushes(), &store.indexed());
    assert!(pushes.iter().any(|(machine, push)| machine == &m1 && matches!(push, fastctx_types::assets::Push::Checkpointed { checkpoint } if checkpoint.change == 2)), "{pushes:?}");
    store.book.directory.shares.clear();
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(matches!(ask(&store, &m2, &k2, Request::Frames { scope: conv1.into(), stream: "timeline".into(), from: 0, limit: 10 }).unwrap(), Reply::Revoked { .. }), "and no more once unshared: told so");

    // Objects: a frame referring to one the hub has not wants it first; a put names every chunk under a ticket.
    let big: Vec<u8> = (0..(2 * 1024 * 1024 + 4321)).map(|i: u32| (i % 251) as u8).collect();
    let lease = producer.put(&mut big.as_slice(), "drill", std::time::Duration::from_secs(60)).unwrap();
    let object = lease.object;
    let record3 = KindRecord { kind: 1, schema: 1, record: Record { item: b"c".to_vec(), order: b"c".to_vec(), state: State::Is { body: b"file".to_vec(), objects: vec![object] } } };
    producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records: vec![record3], leases: vec![lease], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, Arc::new(Machine(SecretKey::from_seed(&k1.seed())))).unwrap();
    let raw3 = producer.export(conv1, "timeline", 2, 1).unwrap().remove(0);
    let frame3 = Frame { header: raw3.header.into(), signature: raw3.signature.into(), body: raw3.body.into(), apart: 0 };
    let wire_object = fastctx_types::assets::ObjectRef { hash: object.hash.into(), size: object.size };
    let take3 = |proven: Vec<fastctx_types::assets::Bytes>| Request::Take { scope: conv1.into(), frame: frame3.clone(), proven, staged: 0 };
    let Reply::Received { received: Received::Missing { objects } } = ask(&store, &m1, &k1, take3(vec![])).unwrap() else { panic!("missing") };
    assert_eq!(objects, vec![wire_object.clone()]);
    let manifest: Vec<fastctx_types::assets::Bytes> = producer.chunks(&object.hash).unwrap().into_iter().map(Into::into).collect();
    assert_eq!(manifest.len(), 3);
    let put = Request::Put { scope: conv1.into(), object: wire_object.clone(), manifest: vec![], prove: false };
    let Reply::Put { ticket, missing } = ask(&store, &m1, &k1, put.clone()).unwrap() else { panic!("put") };
    assert!(ticket != 0);
    assert!(missing.is_empty(), "a large object's digests and the chunks wanted go on the data connection");
    assert!(refused(ask(&store, &m2, &k2, put.clone())), "bob's machine reads not the scope");
    let get = Request::Get { scope: conv1.into(), object: wire_object.clone(), from: 0 };
    assert!(ask(&store, &m1, &k1, get.clone()).is_err(), "not held, not read");
    // What the data connection does: two chunks come, then the put resumes from the third.
    let scope = scopes::scope(&assets, conv1).unwrap().unwrap();
    let hub_library = scopes::library(&assets, &scope).unwrap();
    let digests = producer.chunks(&object.hash).unwrap();
    let mut transfer = hub_library.receive(object, digests.clone(), None).unwrap().unwrap();
    for index in 0..2u64 {
        let start = (index * 1024 * 1024) as usize;
        transfer.chunk(index, &big[start..start + 1024 * 1024]).unwrap();
    }
    transfer.settle().unwrap();
    drop(transfer);
    let Reply::Put { ticket, .. } = ask(&store, &m1, &k1, put.clone()).unwrap() else { panic!("put") };
    assert!(ticket != 0, "under way, a chunk short");
    let mut transfer = hub_library.receive(object, digests, None).unwrap().unwrap();
    transfer.chunk(2, &big[2 * 1024 * 1024..]).unwrap();
    transfer.settle().unwrap();
    drop(transfer);
    assert!(hub_library.size(&object.hash).unwrap().is_none(), "every chunk in, not placed yet (a stop before it was)");
    let Reply::Put { ticket, missing } = ask(&store, &m1, &k1, put.clone()).unwrap() else { panic!("put") };
    assert_eq!((ticket, missing), (0, vec![]), "placed now, nothing to send (W05)");
    assert_eq!(hub_library.size(&object.hash).unwrap(), Some(object.size));
    // Held, but not read by m1 and not proved: proved first, a claim alone counts for nothing.
    let Reply::Received { received: Received::Prove { objects } } = ask(&store, &m1, &k1, take3(vec![object.hash.into()])).unwrap() else { panic!("prove") };
    assert_eq!(objects, vec![wire_object.clone()]);
    let prove = Request::Put { scope: conv1.into(), object: wire_object.clone(), manifest: vec![], prove: true };
    let Reply::Put { ticket, missing } = ask(&store, &m1, &k1, prove).unwrap() else { panic!("prove") };
    assert!(ticket != 0);
    assert!(missing.is_empty(), "a proof is every chunk in order: none listed on the control connection");
    // Or by a challenge (§28.45): asked of the device the hub asked to prove it, answered with every byte, worked
    // out apart and asked after; the proof then stands as one the data connection records.
    let challenge = || Request::Challenge { object: wire_object.clone() };
    assert!(ask(&store, &m1, &k1, challenge()).is_err(), "a session opened in asset sync 1 has no challenges");
    ask(&store, &m1, &k1, Request::Open { version: 2, seen: None, reported: vec![], pinned: None, lineage: vec![] }).unwrap();
    assert!(ask(&store, &m2, &k2, challenge()).is_err(), "only the device asked to prove it");
    let answer = |nonce: &[u8]| -> fastctx_types::assets::Bytes {
        use sha2::Digest as _;
        let mut digest = sha2::Sha256::new();
        digest.update(fastctx_types::assets::PROOF_TAG);
        digest.update(nonce);
        digest.update(&big);
        fastctx_types::assets::Bytes(digest.finalize().to_vec())
    };
    let settled = |nonce: &fastctx_types::assets::Bytes, given: &fastctx_types::assets::Bytes| {
        for _ in 0..400 {
            match ask(&store, &m1, &k1, Request::Proof { nonce: nonce.clone(), answer: given.clone() }).unwrap() {
                Reply::Proof { proof: fastctx_types::assets::ProofState::Working } => std::thread::sleep(std::time::Duration::from_millis(25)),
                Reply::Proof { proof } => return proof,
                other => panic!("a proof answered {other:?}"),
            }
        }
        panic!("a proof never settled")
    };
    let Reply::Challenge { nonce, .. } = ask(&store, &m1, &k1, challenge()).unwrap() else { panic!("challenge") };
    let keep = Request::Proof { nonce: nonce.clone(), answer: fastctx_types::assets::Bytes(vec![]) };
    assert!(
        matches!(ask(&store, &m1, &k1, keep).unwrap(), Reply::Proof { proof: fastctx_types::assets::ProofState::Working }),
        "asked with no answer: kept, and still to be answered"
    );
    let right = answer(&nonce.0);
    assert!(ask(&store, &m2, &k2, Request::Proof { nonce: nonce.clone(), answer: right.clone() }).is_err(), "another device's challenge");
    assert_eq!(settled(&nonce, &right), fastctx_types::assets::ProofState::Proven);
    assert!(ask(&store, &m1, &k1, Request::Proof { nonce: nonce.clone(), answer: answer(b"other") }).is_err(), "answered once");
    assert!(assets.objects.proven(k1.public().as_bytes(), &object.hash));
    // Two challenges not settled at most; a wrong answer bars the object to the device, a right one already
    // asked for still proves.
    let Reply::Challenge { nonce: second, .. } = ask(&store, &m1, &k1, challenge()).unwrap() else { panic!("second") };
    let Reply::Challenge { nonce: third, .. } = ask(&store, &m1, &k1, challenge()).unwrap() else { panic!("third") };
    assert!(ask(&store, &m1, &k1, challenge()).is_err(), "two not settled at most");
    assert_eq!(settled(&second, &answer(b"not the bytes")), fastctx_types::assets::ProofState::Wrong);
    assert!(ask(&store, &m1, &k1, challenge()).is_err(), "answered wrong: barred");
    assert_eq!(settled(&third, &answer(&third.0)), fastctx_types::assets::ProofState::Proven);
    assert!(ask(&store, &m1, &k1, Request::Proof { nonce: fastctx_types::assets::Bytes(vec![7; 32]), answer: right }).is_err(), "unknown");
    println!("challenges: ok");
    // The frame is taken; then the object is read and given.
    assert!(!assets.objects.proven(k2.public().as_bytes(), &object.hash), "a proof is its device's key's alone");
    let Reply::Received { received: Received::Applied { .. } } = ask(&store, &m1, &k1, take3(vec![object.hash.into()])).unwrap() else { panic!("applied") };
    let Reply::Get { ticket, manifest: given } = ask(&store, &m1, &k1, get.clone()).unwrap() else { panic!("get") };
    assert!(ticket != 0);
    assert!(given.is_empty(), "the digests come on the data connection");
    let _ = manifest;
    println!("objects: ok");

    // A frame body over 64 KiB travels apart: refused inline, staged under a ticket, then taken; given back apart too.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut noise = |n: usize| -> Vec<u8> {
        (0..n).map(|_| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed as u8 }).collect()
    };
    let records: Vec<KindRecord> = (0..3u8)
        .map(|i| KindRecord { kind: 1, schema: 1, record: Record { item: vec![b'd', i], order: vec![b'd', i], state: State::Is { body: noise(40 * 1024), objects: vec![] } } })
        .collect();
    producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records, leases: vec![], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, Arc::new(Machine(SecretKey::from_seed(&k1.seed())))).unwrap();
    let raw4 = producer.export(conv1, "timeline", 3, 1).unwrap().remove(0);
    assert!(raw4.body.len() > 64 * 1024, "{}", raw4.body.len());
    let whole4 = Frame::whole(raw4.header.clone(), raw4.signature.clone(), raw4.body.clone());
    assert!(ask(&store, &m1, &k1, Request::Take { scope: conv1.into(), frame: whole4, proven: vec![], staged: 0 }).is_err(), "too large inline");
    assert!(ask(&store, &m1, &k1, Request::Stage { scope: conv1.into(), size: 1000 }).is_err(), "only a large body is staged");
    assert!(ask(&store, &m1, &k1, Request::Stage { scope: conv1.into(), size: 3 << 20 }).is_err(), "at most 2 MiB");
    let Reply::Staged { ticket } = ask(&store, &m1, &k1, Request::Stage { scope: conv1.into(), size: raw4.body.len() as u64 }).unwrap() else { panic!("staged") };
    let apart4 = Frame { header: raw4.header.clone().into(), signature: raw4.signature.clone().into(), body: Default::default(), apart: raw4.body.len() as u64 };
    let take4 = |staged| Request::Take { scope: conv1.into(), frame: apart4.clone(), proven: vec![], staged };
    assert!(ask(&store, &m1, &k1, take4(ticket)).is_err(), "nothing staged yet");
    assets.objects.apart.staged(ticket, k1.public().as_bytes(), raw4.body.clone()).unwrap();
    assert!(ask(&store, &m1, &k2, take4(ticket)).is_err(), "a staged body is its device's key's alone");
    let mut wrong = apart4.clone();
    wrong.apart -= 1;
    assert!(ask(&store, &m1, &k1, Request::Take { scope: conv1.into(), frame: wrong, proven: vec![], staged: ticket }).is_err(), "the staged length is the frame's");
    let Reply::Received { received: Received::Applied { .. } } = ask(&store, &m1, &k1, take4(ticket)).unwrap() else { panic!("applied apart") };
    // Its body let go once the frame was settled, the frame asked again hears what the hub said of it.
    assert!(matches!(ask(&store, &m1, &k1, take4(ticket)).unwrap(), Reply::Received { received: Received::Applied { .. } }), "asked again, told again");
    let Reply::Frames { frames, ticket } = ask(&store, &m1, &k1, Request::Frames { scope: conv1.into(), stream: "timeline".into(), from: 0, limit: 10 }).unwrap() else { panic!() };
    assert_eq!(frames.len(), 4);
    assert!(frames[..3].iter().all(|frame| frame.apart == 0 && !frame.body.0.is_empty()), "small ones inline");
    assert_eq!((frames[3].apart, frames[3].body.0.len()), (raw4.body.len() as u64, 0));
    assert!(ticket != 0);
    assert!(assets.objects.apart.going(ticket, k2.public().as_bytes()).is_none(), "under its device's key");
    assert_eq!(assets.objects.apart.going(ticket, k1.public().as_bytes()).unwrap(), vec![raw4.body.clone()]);
    assert!(assets.objects.apart.going(ticket, k1.public().as_bytes()).is_none(), "sent once");
    println!("frames apart: ok");

    // Several frames of a stream in one take (§18 E2), from asset sync 3: each answered as taken alone, in order,
    // up to the first not applied; one wanting objects ends it, and frames of one stream, inline, at most 16.
    let signer = || Arc::new(Machine(SecretKey::from_seed(&k1.seed())));
    for n in 0..5u8 {
        let record = KindRecord { kind: 1, schema: 1, record: Record { item: vec![b'm', n], order: vec![b'm', n], state: State::Is { body: vec![n; 64], objects: vec![] } } };
        producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records: vec![record], leases: vec![], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, signer()).unwrap();
    }
    let many: Vec<Frame> = producer.export(conv1, "timeline", 6, 10).unwrap().into_iter().map(|raw| Frame::whole(raw.header, raw.signature, raw.body)).collect();
    assert_eq!(many.len(), 5);
    let take_many = |frames: &[Frame]| Request::TakeMany { scope: conv1.into(), frames: frames.to_vec() };
    assert!(ask(&store, &m1, &k1, take_many(&many[..3])).is_err(), "a session opened in asset sync 2 takes frames one at a time");
    ask(&store, &m1, &k1, Request::Open { version: 3, seen: None, reported: vec![], pinned: None, lineage: vec![] }).unwrap();
    let words = |reply: Reply| -> Vec<Received> {
        let Reply::ReceivedMany { received } = reply else { panic!("words on several: {reply:?}") };
        received
    };
    let first = words(ask(&store, &m1, &k1, take_many(&many[..3])).unwrap());
    let changes: Vec<u64> = first.iter().map(|word| match word { Received::Applied { change, .. } => *change, other => panic!("applied: {other:?}") }).collect();
    assert!(changes.windows(2).all(|pair| pair[1] == pair[0] + 1), "{changes:?}");
    // Asked again — a receipt lost — each is told as before, nothing applied twice.
    assert_eq!(words(ask(&store, &m1, &k1, take_many(&many[..2])).unwrap()), first[..2].to_vec(), "asked again, told again");
    assert!(matches!(words(ask(&store, &m1, &k1, take_many(&many[4..])).unwrap())[..], [Received::Gap { from: 9, to: 9 }]), "past a gap: told where it is");
    let rest = words(ask(&store, &m1, &k1, take_many(&many[3..])).unwrap());
    assert!(matches!(rest[..], [Received::Applied { change: a, .. }, Received::Applied { change: b, .. }] if a == changes[2] + 1 && b == a + 1), "{rest:?}");
    // One wanting an object ends the take: answered as alone, the frame after it not taken.
    let wanted: Vec<u8> = (0..70_000u32).map(|i| (i % 241) as u8).collect();
    let lease = producer.put(&mut wanted.as_slice(), "drill", std::time::Duration::from_secs(60)).unwrap();
    let wanting = KindRecord { kind: 1, schema: 1, record: Record { item: b"w".to_vec(), order: b"w".to_vec(), state: State::Is { body: b"wants".to_vec(), objects: vec![lease.object] } } };
    producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records: vec![wanting], leases: vec![lease], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, signer()).unwrap();
    let after = KindRecord { kind: 1, schema: 1, record: Record { item: b"x".to_vec(), order: b"x".to_vec(), state: State::Is { body: b"after".to_vec(), objects: vec![] } } };
    producer.raw_commit(Commit { scope: conv1.into(), stream: "timeline".into(), records: vec![after], leases: vec![], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, signer()).unwrap();
    let two: Vec<Frame> = producer.export(conv1, "timeline", 11, 10).unwrap().into_iter().map(|raw| Frame::whole(raw.header, raw.signature, raw.body)).collect();
    assert_eq!(two.len(), 2);
    assert!(matches!(words(ask(&store, &m1, &k1, take_many(&two)).unwrap())[..], [Received::Missing { .. }]), "wants the object, the frame after it left");
    // What one take carries: frames of one stream, none apart, 1 to 16 of them.
    let other = KindRecord { kind: 1, schema: 1, record: Record { item: b"o".to_vec(), order: b"o".to_vec(), state: State::Is { body: b"other".to_vec(), objects: vec![] } } };
    producer.raw_commit(Commit { scope: conv1.into(), stream: "notes".into(), records: vec![other], leases: vec![], durable: true, op: None, holding: producer.holding(conv1).unwrap().unwrap() }, signer()).unwrap();
    let note = producer.export(conv1, "notes", 0, 1).unwrap().into_iter().map(|raw| Frame::whole(raw.header, raw.signature, raw.body)).next().unwrap();
    // One of another stream ends the take before it, as one that does not pass the gate does.
    assert_eq!(words(ask(&store, &m1, &k1, take_many(&[two[1].clone(), note.clone()])).unwrap()).len(), 1, "frames of one stream");
    assert!(ask(&store, &m1, &k1, take_many(std::slice::from_ref(&note))).is_ok(), "taken first, it is taken");
    assert!(ask(&store, &m1, &k1, take_many(&[])).is_err(), "at least one");
    assert!(ask(&store, &m1, &k1, take_many(&vec![two[1].clone(); 17])).is_err(), "at most 16");
    assert!(ask(&store, &m1, &k1, take_many(std::slice::from_ref(&apart4))).is_err(), "none apart");
    println!("several frames in one take: ok");

    // Bob's account made again under his name reads nothing of what the old one did.
    let bobs_own = Request::Describe { scope: own("bob", "notes"), readers: Readers::Account { account: bob.clone() }, classes: fastctx_types::assets::records_classes(), born: Default::default() };
    assert!(matches!(ask(&store, &m2, &k2, bobs_own.clone()).unwrap(), Reply::Described { .. }));
    let streams = Request::Streams { scope: own("bob", "notes") };
    assert!(ask(&store, &m2, &k2, streams.clone()).is_ok());
    let again = store.book.directory.accounts.iter().position(|account| account.name == bob).unwrap();
    Arc::make_mut(&mut store.book.directory.accounts[again]).opened = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(5);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(refused(ask(&store, &m2, &k2, streams)), "the new bob reads nothing of the old one's");
    println!("incarnations: ok");

    // m1 given whole to bob, the share waiting for m1 to clear: m1 keeps what bob will read through holding it, and is told to drop the rest.
    store.book.directory.shares.push(Arc::new(fastctx_net::directory::Share { machine: m1.clone(), account: bob.clone(), jobs: None, pending: Some(5), basis: None }));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(!store.indexed().holds(&bob, &m1), "a waiting share gives nothing yet");
    // The revision moves on only when what decides what m1 may hold does, and past every earlier one.
    let waiting_at = |store: &crate::store::Store| store.book.directory.shares.iter().find(|share| share.pending.is_some()).and_then(|share| share.pending).unwrap();
    let r1 = waiting_at(&store);
    assert_ne!(r1, 5, "given a revision of the save that wrote it");
    let at2 = store.book.directory.members.iter().position(|member| member.name == m2).unwrap();
    Arc::make_mut(&mut store.book.directory.members[at2]).online = true;
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert_eq!(waiting_at(&store), r1, "a machine coming online changes nothing m1 may hold");
    let at1 = store.book.directory.members.iter().position(|member| member.name == m1).unwrap();
    Arc::make_mut(&mut store.book.directory.members[at1]).logins.insert(bob.clone());
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let r2 = waiting_at(&store);
    assert!(r2 > r1, "a login on m1 does: {r1} then {r2}");
    Arc::make_mut(&mut store.book.directory.members[at1]).logins.remove(&bob);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(waiting_at(&store) > r2);
    let frames = |key: &str| Request::Frames { scope: key.into(), stream: "timeline".into(), from: 0, limit: 10 };
    assert!(matches!(ask(&store, &m1, &k1, frames(conv1)).unwrap(), Reply::Frames { .. }), "born on m1: bob's once the share is in force");
    assert!(matches!(ask(&store, &m1, &k1, Request::Streams { scope: own("root", "settings") }).unwrap(), Reply::Revoked { .. }), "root's own: m1 drops it");
    assert!(matches!(ask(&store, &m2, &k2, frames(conv1)).unwrap(), Reply::Revoked { .. }), "bob reads nothing through m1 yet");
    let keeps = fastctx_net::record_ownership::keeps(&store.book.directory, store.book.directory.member(&m1).unwrap(), std::slice::from_ref(&root), Some(&m1));
    assert!(keeps, "records born on m1 stay");
    let keeps_other = fastctx_net::record_ownership::keeps(&store.book.directory, store.book.directory.member(&m1).unwrap(), std::slice::from_ref(&root), None);
    assert!(!keeps_other, "root's records born elsewhere go");
    let at = store.book.directory.shares.iter().position(|share| share.pending.is_some()).unwrap();
    Arc::make_mut(&mut store.book.directory.shares[at]).pending = None;
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(store.indexed().holds(&bob, &m1), "in force");
    assert!(matches!(ask(&store, &m2, &k2, frames(conv1)).unwrap(), Reply::Frames { .. }), "bob reads through m1 now");
    println!("waiting shares: ok");

    // §17 (b): sealed as it starts, by its setting, until the super administrator opens them; nothing else opens it.
    assets.seal_at_start();
    let Reply::Sealed { why, .. } = ask(&store, &m1, &k1, open(vec![])).unwrap() else { panic!("sealed at start") };
    assert!(why.contains("each time it starts"), "{why}");
    {
        let mut held = assets.seal_held();
        crate::assets::Assets::openable(&held).unwrap();
        *held = None;
    }
    assert!(matches!(ask(&store, &m1, &k1, open(vec![])).unwrap(), Reply::Ready { .. }), "open once opened");
    assert!(crate::assets::Assets::openable(&assets.seal_held()).is_err(), "nothing to open");
    println!("sealed at start: ok");

    // A forged receipt is passed over; a receipt the hub's word stands behind that its history does not hold seals it.
    let mut fake = reported.clone();
    fake.said = Said::Applied { change: 99, prefix: vec![7u8; 32].into() };
    let Reply::Ready { .. } = ask(&store, &m1, &k1, open(vec![fake.clone()])).unwrap() else { panic!("a forged word is no evidence") };
    let digest = header.digest;
    let key = FrameKey { scope: conv1, stream: "timeline", epoch: 0, first: 0, digest: &digest };
    fake.word = assets.signer().sign(&applied_statement(&key, 99, &[7u8; 32])).into();
    let Reply::Sealed { why, .. } = ask(&store, &m1, &k1, open(vec![fake.clone()])).unwrap() else { panic!("sealed") };
    assert!(why.contains("no longer holds"), "{why}");
    assert!(matches!(ask(&store, &m1, &k1, take).unwrap(), Reply::Sealed { .. }), "nothing of assets once sealed");
    // A hub sealed only as it started that is shown it was put back seals
    // for its recovery: the super administrator no longer opens it (AI04).
    *assets.seal_held() = None;
    assets.seal_at_start();
    assert!(assets.sealed().is_some_and(|why| crate::assets::Assets::is_start_seal(&why)));
    let Reply::Sealed { why, .. } = ask(&store, &m1, &k1, open(vec![fake])).unwrap() else { panic!("sealed") };
    assert!(why.contains("no longer holds"), "escalated: {why}");
    assert!(!assets.sealed().is_some_and(|why| crate::assets::Assets::is_start_seal(&why)), "no longer only the start seal");
    assert!(crate::assets::Assets::openable(&assets.seal_held()).is_err(), "not opened as an ordinary start");
    // AK01: started again as ever, the hub is still sealed for its recovery, not only as it starts.
    let again = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    let why = again.sealed().expect("still sealed after a restart");
    assert!(!crate::assets::Assets::is_start_seal(&why) && why.contains("no longer holds"), "{why}");
    again.seal_at_start();
    assert!(crate::assets::Assets::openable(&again.seal_held()).is_err(), "a start seal does not stand in for it");
    drop(again);
    // AL01: stopped after what was brought was kept but before the seal was, it seals as it starts.
    store.shelf.0.write_now(64, |c| { c.execute("DELETE FROM asset_seal", [])?; Ok(()) }).unwrap();
    let again = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    let why = again.sealed().expect("sealed from what was brought");
    assert!(!crate::assets::Assets::is_start_seal(&why) && why.contains("no longer holds"), "{why}");
    drop(again);
    println!("start seal gives way to a recovery: ok");
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    println!("hub serve drill: ok");
}

#[test]
fn bodies_outlive_their_machine() {
    let base = std::env::temp_dir().join(format!("hub-bodies-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = AccountName::try_from("root".to_string()).unwrap();
    let bob = AccountName::try_from("bob".to_string()).unwrap();
    crate::init(&dir, crate::New { name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(), public: vec![address], listen: address, admin: root.clone(), password: "a long enough password 123".into() }).unwrap();
    let mut store = crate::store::Store::open_logged(&dir, None).unwrap();
    let assets = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    store.bodies = Some(assets.bodies.clone());
    let (k1, k2) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
    store.book.directory.accounts.push(Arc::new(Account { name: bob.clone(), level: Level::Member, locked: false, opened: jiff::Timestamp::now(), collab: None, history: false, profile: None, promotion: false }));
    store.book.directory.members.push(member("m1", &k1, &root));
    store.book.directory.members.push(member("m2", &k2, &bob));
    let (m1, m2) = (MachineName::try_from("m1".to_string()).unwrap(), MachineName::try_from("m2".to_string()).unwrap());
    store.book.directory.shares.push(Arc::new(fastctx_net::directory::Share { machine: m1.clone(), account: bob.clone(), jobs: None, pending: None, basis: None }));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let ask = |store: &crate::store::Store, machine: &MachineName, key: &SecretKey, request: Request| serve::ask(&assets, &store.indexed(), &String::new, machine, &key.public(), request);
    let body = bodies::body_of(&store.book.directory, &m1).unwrap();
    let readers = Readers::Body { body: body.clone(), opener: root.clone() };
    let classes = fastctx_types::assets::conversation_classes();
    let key = format!("b/{body}/talk");
    let dave = AccountName::try_from("dave".to_string()).unwrap();
    store.book.directory.accounts.push(Arc::new(Account { name: dave.clone(), level: Level::Member, locked: false, opened: jiff::Timestamp::now(), collab: None, history: false, profile: None, promotion: false }));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let theirs = Readers::Body { body: body.clone(), opener: dave.clone() };
    assert!(ask(&store, &m1, &k1, Request::Describe { scope: key.clone(), readers: theirs, classes: classes.clone(), born: Default::default() }).is_err(), "an opener the machine does not act for");
    assert!(ask(&store, &m2, &k2, Request::Describe { scope: key.clone(), readers: readers.clone(), classes: classes.clone(), born: Default::default() }).is_err(), "described by the body's machine alone");
    let Reply::Described { .. } = ask(&store, &m1, &k1, Request::Describe { scope: key.clone(), readers, classes, born: Default::default() }).unwrap() else { panic!("described") };
    let streams = Request::Streams { scope: key.clone() };
    assert!(matches!(ask(&store, &m2, &k2, streams.clone()).unwrap(), Reply::Streams { .. }), "bob, a member, reads it");
    // m1 leaves the directory, then root is deleted: bob, frozen in, reads on.
    store.book.directory.shares.clear();
    store.book.directory.members.retain(|member| member.name != m1);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(matches!(ask(&store, &m2, &k2, streams.clone()).unwrap(), Reply::Streams { .. }), "frozen in when the machine left");
    store.book.directory.accounts.retain(|account| account.name != root);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert!(matches!(ask(&store, &m2, &k2, streams.clone()).unwrap(), Reply::Streams { .. }), "the owner gone, bob reads on");
    assert_eq!(assets.sweep_owners(&store.indexed()).unwrap(), 0);
    // Bob deleted too: no one reads it; it goes once no one has for long enough.
    store.book.directory.accounts.retain(|account| account.name != bob);
    store.book.directory.accounts.push(Arc::new(Account { name: AccountName::try_from("carol".to_string()).unwrap(), level: Level::Member, locked: false, opened: jiff::Timestamp::now(), collab: None, history: false, profile: None, promotion: false }));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    assert_eq!(assets.sweep_owners(&store.indexed()).unwrap(), 0, "not before the grace");
    store.shelf.0.write_now(64, |c| { c.execute("UPDATE asset_unread SET since = 0", [])?; Ok(()) }).unwrap();
    assert_eq!(assets.sweep_owners(&store.indexed()).unwrap(), 1);
    assert!(scopes::scope(&assets, &key).unwrap().is_none());
    for _ in 0..100 {
        if !fastctx_platform::disk::is_there(dir.join("assets").join("body").join(&body)) { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assets.sweep_owners(&store.indexed()).unwrap();
    }
    assert!(!fastctx_platform::disk::is_there(dir.join("assets").join("body").join(&body)), "its files go, once closed");
    // A pinned key the keyring lacks: a key a machine made, signing for itself, shows nothing; one the keyring's own rotation leads to seals.
    let made = SecretKey::generate().unwrap();
    let k3 = SecretKey::generate().unwrap();
    store.book.directory.members.push(member("m3", &k3, &dave));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let m3 = MachineName::try_from("m3".to_string()).unwrap();
    let pinned = |lineage| Request::Open { version: 1, seen: None, reported: vec![], pinned: Some(made.public().as_bytes().to_vec().into()), lineage };
    let selfmade = fastctx_types::assets::Rotation {
        old: made.public().as_bytes().to_vec().into(),
        new: made.public().as_bytes().to_vec().into(),
        word: made.sign(Domain::SyncHubKeyRotation, &keys::rotation_statement(made.public().as_bytes(), made.public().as_bytes())).to_bytes().to_vec().into(),
    };
    assert!(ask(&store, &m3, &k3, pinned(vec![selfmade])).is_err(), "no rotation of the keyring leads to it: refused, not sealed");
    assert!(assets.sealed().is_none());
    let current = SecretKey::load(&dir.join("keys").join("current.key")).unwrap();
    let (old, new) = (*current.public().as_bytes(), *made.public().as_bytes());
    let legit = fastctx_types::assets::Rotation { old: old.to_vec().into(), new: new.to_vec().into(), word: current.sign(Domain::SyncHubKeyRotation, &keys::rotation_statement(&old, &new)).to_bytes().to_vec().into() };
    // Sealed only as it started, the same key seals for the recovery (AJ02).
    assets.seal_at_start();
    let Reply::Sealed { why, .. } = ask(&store, &m3, &k3, pinned(vec![legit.clone()])).unwrap() else { panic!("sealed") };
    assert!(why.contains("keyring lacks"), "escalated: {why}");
    assert!(crate::assets::Assets::openable(&assets.seal_held()).is_err(), "not opened as an ordinary start");
    *assets.seal_held() = None;
    assert!(matches!(ask(&store, &m3, &k3, pinned(vec![legit])).unwrap(), Reply::Sealed { .. }), "the keyring's own successor: put back to before a rotation");
    // What a retired key's write cut short left beside it does not keep the keyring from opening.
    let retired = dir.join("keys").join("retired");
    fastctx_platform::disk::create_dir_all(&retired).unwrap();
    fastctx_platform::disk::write(retired.join(".fastctx-1-1.tmp"), b"half").unwrap();
    assert!(keys::Keys::open(&dir.join("keys")).is_ok());
    // A copy never lands inside what it copies through a step back up.
    assert!(keys::copy_tree(&dir, &base.join("missing").join("..").join("hub").join("backup")).is_err());
    drop((store, assets));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    println!("bodies drill: ok");
}
