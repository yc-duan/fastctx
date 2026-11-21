//! A drill of §17's delay in finding a hub was put back (design §28.40
//! seven): an image of the hub's directory and of the witness outside it,
//! made after a checkpoint, put back after the hub took more — a frame
//! applied and a frame parked, whose receipts no checkpoint covers yet.
//! Neither the image nor the witness tells; the hub opens unsealed and
//! answers a device that saw nothing newer. Only a device bringing one of
//! those receipts tells, an applied one or a parked one, and the hub seals
//! as it opens that device's session. The producer, never told a checkpoint
//! covered its frame, still holds it: taken again once the hub reopens.
use crate::assets::*;
use fastctx_content::v2::{Commit, Cutoff, EpochKind, Header, KindRecord, Library, Successor};
use fastctx_content::{Record, State};
use fastctx_net::directory::{Kind, Member};
use fastctx_net::sign::{Domain, SecretKey};
use fastctx_types::assets::{Frame, Readers, Received, Reply, Reported, Request, Said};
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

fn open(dir: &Path) -> (crate::store::Store, Assets) {
    // A handle just let go closes on its writer's thread a moment later.
    let mut tries = 0;
    let mut store = loop {
        match crate::store::Store::open_logged(dir, None) {
            Ok(store) => break store,
            Err(_) if tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => panic!("{error}"),
        }
    };
    let assets = Assets::open(dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    (store, assets)
}

fn copy(from: &Path, to: &Path) {
    fastctx_platform::disk::create_dir_all(to).unwrap();
    for entry in fastctx_platform::disk::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        // A database's index of its log is made again as it opens, and may be
        // held locked by a handle let go a moment ago: no copy needs it.
        if entry.file_name().to_string_lossy().ends_with("-shm") {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else if let Err(error) = fastctx_platform::disk::copy(entry.path(), &target) {
            assert!(error.kind() == std::io::ErrorKind::NotFound, "copying {}: {error}", entry.path().display());
        }
    }
}

fn retry(mut op: impl FnMut() -> std::io::Result<()>) {
    for _ in 0..200 {
        if op().is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    op().unwrap();
}

/// Puts the image back: the hub's directory and the witness, as an image of
/// the whole disk holds both.
fn put_back(base: &Path) {
    std::thread::sleep(std::time::Duration::from_millis(300));
    for (live, image) in [("hub", "image-hub"), ("hub-witness", "image-witness")] {
        retry(|| fastctx_platform::disk::remove_dir_all(base.join(live)));
        copy(&base.join(image), &base.join(live));
    }
}

#[test]
fn found_out_only_by_receipts() {
    let base = std::env::temp_dir().join(format!("hub-identify-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = AccountName::try_from("root".to_string()).unwrap();
    crate::init(&dir, crate::New {
        name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(),
        public: vec![address],
        listen: address,
        admin: root.clone(),
        password: "a long enough password 123".into(),
    })
    .unwrap();
    let k1 = SecretKey::generate().unwrap();
    let m1 = MachineName::try_from("m1".to_string()).unwrap();
    let ask = |store: &crate::store::Store, assets: &Assets, request: Request| {
        serve::ask(assets, &store.indexed(), &String::new, &m1, &k1.public(), request)
    };
    let (mut store, assets) = open(&dir);
    store.book.directory.members.push(member("m1", &k1, &root));
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let conv = format!("c/{}/m1/conv-1", assets.id);
    let readers = Readers::Conversation { openers: vec![root.clone()], born_on: Some(m1.clone()) };
    let describe = Request::Describe { scope: conv.clone(), readers, classes: fastctx_types::assets::conversation_classes(), born: Default::default() };
    let Reply::Described { descriptor } = ask(&store, &assets, describe).unwrap() else { panic!("described") };

    // The producer, m1: frame 0 taken and checkpointed.
    let producer_dir = base.join("m1-assets");
    fastctx_platform::disk::create_dir_all(&producer_dir).unwrap();
    let (producer, _) = Library::open(&producer_dir).unwrap();
    producer.install(&conv, descriptor.fact.0.clone(), 1).unwrap();
    let signer = Arc::new(Machine(SecretKey::from_seed(&k1.seed())));
    let commit = |item: &str| {
        let record = KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: item.as_bytes().to_vec(), objects: vec![] } } };
        producer.raw_commit(Commit { scope: conv.clone(), stream: "timeline".into(), records: vec![record], leases: vec![], durable: true, op: None, holding: producer.holding(&conv).unwrap().unwrap() }, signer.clone()).unwrap();
    };
    let wired = |first: u64| {
        let raw = producer.export(&conv, "timeline", first, 1).unwrap().remove(0);
        (Header::decode(&raw.header).unwrap(), Frame { header: raw.header.into(), signature: raw.signature.into(), body: raw.body.into(), apart: 0 })
    };
    let take = |store: &crate::store::Store, assets: &Assets, frame: Frame| match ask(store, assets, Request::Take { scope: conv.clone(), frame, proven: vec![], staged: 0 }).unwrap() {
        Reply::Received { received } => received,
        other => panic!("taken: {other:?}"),
    };
    commit("a");
    assert!(matches!(take(&store, &assets, wired(0).1), Received::Applied { change: 1, .. }));
    assets.checkpoint().unwrap().expect("a checkpoint");
    drop((store, assets));

    // The image: the directory and the witness, at one moment.
    std::thread::sleep(std::time::Duration::from_millis(300));
    copy(&dir, &base.join("image-hub"));
    copy(&base.join("hub-witness"), &base.join("image-witness"));

    // Past the image: frame 1 applied; then, the stream handed over to a
    // new epoch whose predecessor's tail (frame 2) is not in, frame 3 parked.
    // No checkpoint covers either.
    let (store, assets) = open(&dir);
    commit("b");
    let (header1, frame1) = wired(1);
    let Received::Applied { change, prefix, word, .. } = take(&store, &assets, frame1.clone()) else { panic!("applied") };
    assert_eq!(change, 2);
    let applied = Reported { scope: conv.clone(), stream: "timeline".into(), epoch: 0, first: 1, digest: header1.digest.into(), said: Said::Applied { change, prefix }, word };
    commit("c");
    let (tail, _) = wired(2);
    producer.new_epoch(&conv, "timeline", 1, b"e1".to_vec()).unwrap();
    commit("d");
    let scope = scopes::scope(&assets, &conv).unwrap().unwrap();
    let library = scopes::library(&assets, &scope).unwrap();
    let successor = Successor { device: *k1.public().as_bytes(), key: *k1.public().as_bytes(), kind: EpochKind::Source };
    library.open_epoch(&conv, "timeline", successor, assets.signer()).unwrap();
    let cut = Cutoff { next: 3, head: tail.chain() };
    library.handover(&conv, "timeline", successor, cut, serve::kinds(), assets.signer()).unwrap();
    let (header3, frame3) = wired(3);
    let Received::Parked { sequence, accumulated, word } = take(&store, &assets, frame3) else { panic!("parked") };
    let parked = Reported { scope: conv.clone(), stream: "timeline".into(), epoch: 1, first: 3, digest: header3.digest.into(), said: Said::Parked { sequence, accumulated }, word };
    drop((library, store, assets));

    for (receipt, said) in [(applied, "applied"), (parked, "parked")] {
        put_back(&base);
        let (store, assets) = open(&dir);
        assert!(assets.sealed().is_none(), "{said}: the image tells nothing by itself: {:?}", assets.sealed());
        // A device that saw nothing newer is answered as the image says.
        let open_with = |reported: Vec<Reported>| Request::Open { version: 1, seen: None, reported, pinned: None, lineage: vec![] };
        assert!(matches!(ask(&store, &assets, open_with(vec![])).unwrap(), Reply::Ready { .. }), "{said}: an honest stale device");
        // The one with the receipt seals it as its session opens.
        let Reply::Sealed { why, .. } = ask(&store, &assets, open_with(vec![receipt])).unwrap() else { panic!("{said}: sealed") };
        assert!(why.contains("receipt"), "{said}: {why}");
        assert!(assets.sealed().is_some(), "{said}: and stays so");
        drop((store, assets));
    }

    // Reopened, the producer's frame past the image, which no checkpoint
    // confirmed to it, is still there to go again, and goes.
    std::thread::sleep(std::time::Duration::from_millis(300));
    recovery::reopen(&dir).unwrap();
    let (store, assets) = open(&dir);
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    let (_, again) = wired(1);
    assert_eq!(again, frame1, "the producer still holds the frame no checkpoint covered");
    assert!(matches!(take(&store, &assets, again), Received::Applied { .. }), "taken again");
    drop((store, assets));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    println!("hub identify drill: ok");
}
