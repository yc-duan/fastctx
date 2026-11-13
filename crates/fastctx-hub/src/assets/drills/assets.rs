//! A drill of the hub's chain, witness and seal.
use crate::assets::*;
use fastctx_content::v2::{Authorizer, Commit, Entry, Header, KindRecord, Library, Received, Taking, Verifier};
use fastctx_content::{ObjectRef, Record, State};

struct Device;
impl fastctx_content::v2::Signer for Device {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
impl Verifier for Device {
    fn verify(&self, _: &Header, bytes: &[u8], signature: &[u8]) -> bool { bytes == signature }
}
struct Anyone;
impl Authorizer for Anyone {
    fn authorize(&self, _: &Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> { Ok(()) }
}

fn copy(from: &Path, to: &Path) {
    fastctx_platform::disk::create_dir_all(to).unwrap();
    for entry in fastctx_platform::disk::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        // A database's side files go when its last handle closes, which a
        // handle let go does a moment later: one gone meanwhile is not copied.
        if entry.file_type().unwrap().is_dir() { copy(&entry.path(), &target) } else if let Err(error) = fastctx_platform::disk::copy(entry.path(), &target) {
            assert!(error.kind() == std::io::ErrorKind::NotFound, "copying {}: {error}", entry.path().display());
        }
    }
}

fn open(dir: &Path) -> (crate::store::Store, Assets) {
    // A handle just let go closes on its writer's thread a moment later.
    let mut tries = 0;
    let mut store = loop {
        match crate::store::Store::open_logged(dir, None) {
            Ok(store) => break store,
            Err(error) if tries < 100 => { tries += 1; let _ = error; std::thread::sleep(std::time::Duration::from_millis(50)); }
            Err(error) => panic!("{error}"),
        }
    };
    let assets = Assets::open(dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    (store, assets)
}

/// Does `op` once the files it touches are let go.
/// What `fastctx hub reopen` does to a seal for a recovery: it is gone (AK01).
fn clear_seal(dir: &Path) {
    let (store, assets) = open(dir);
    store.shelf.0.write_now(64, |c| { c.execute("DELETE FROM asset_seal", [])?; Ok(()) }).unwrap();
    drop((store, assets));
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn retry(mut op: impl FnMut() -> std::io::Result<()>) {
    for _ in 0..200 {
        if op().is_ok() { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    op().unwrap();
}

/// One change to the directory, saved.
fn change(store: &mut crate::store::Store) {
    let account = Arc::make_mut(&mut store.book.directory.accounts[0]);
    account.history = !account.history;
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
}

#[test]
fn chain_witness_and_seal() {
    let base = std::env::temp_dir().join(format!("hub-assets-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    crate::init(&dir, crate::New {
        name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(),
        public: vec![address],
        listen: address,
        admin: fastctx_types::AccountName::try_from("root".to_string()).unwrap(),
        password: "a long enough password 123".into(),
    })
    .unwrap();

    // A control fact reaches the chain and the witness before it is published.
    let (mut store, assets) = open(&dir);
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    let before = assets.witness.kept().seq;
    change(&mut store);
    let kept = assets.witness.kept();
    assert_eq!(kept.seq, before + 1, "one control fact, one link");
    let latest = store.shelf.0.read_now(chain::latest).unwrap().unwrap();
    assert_eq!((latest.0, witness::hex(&latest.1)), (kept.seq, kept.head.clone()));
    println!("witness at {}", assets.witness.path().display());

    // A checkpoint of an owner library takes the heads of the scopes that changed.
    let owner = Owner::Account("root".into(), 1);
    let library = assets.library(&owner).unwrap();
    library.install("s", b"d".to_vec(), 1).unwrap();
    let producer_dir = base.join("producer");
    fastctx_platform::disk::create_dir_all(&producer_dir).unwrap();
    let (producer, _) = Library::open(&producer_dir).unwrap();
    producer.install("s", b"d".to_vec(), 1).unwrap();
    let record = |item: &str| KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: b"x".to_vec(), objects: vec![] } } };
    let commit = |item: &str| producer.raw_commit(Commit { scope: "s".into(), stream: "t".into(), records: vec![record(item)], leases: vec![], durable: true, op: None, holding: producer.holding("s").unwrap().unwrap() }, Arc::new(Device)).unwrap();
    commit("a");
    let anyone = |_: &ObjectRef| true;
    let taking = Taking { period: library.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let frame = producer.export("s", "t", 0, 1).unwrap().remove(0);
    let Received::Applied { signature: word, .. } = library.take("s", frame, &Device, &taking, Arc::new(Anyone), assets.signer()).unwrap() else { panic!("applied") };
    let heard = fastctx_net::sign::heard_word(&assets.public(), b"", &word);
    assert!(heard.is_none(), "a word verifies only over its own statement");
    assets.changed(owner.clone(), "s");
    let (seq, _) = assets.checkpoint().unwrap().expect("a checkpoint");
    let kept = assets.witness.kept();
    assert_eq!(kept.seq, seq);
    let rows: i64 = store.shelf.0.read_now(|c| Ok(c.query_row("SELECT COUNT(*) FROM hub_checkpoints WHERE owner = 'account:root:1'", [], |r| r.get(0))?)).unwrap();
    assert_eq!(rows, 1);
    // A head the witness has not (a crash after the commit) reaches it when the hub opens.
    let (lost_seq, lost_head) = store.shelf.0.write_now(64, |c| chain::append(c, "drill", &[9; 32])).unwrap();
    assert!(assets.witness.kept().seq < lost_seq);
    assert!(assets.checkpoint().unwrap().is_none(), "nothing changed since");
    drop((store, assets, library));

    // The hub as it is now opens unsealed, its witness caught up.
    let (store, assets) = open(&dir);
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    assert_eq!(assets.witness.current(), (lost_seq, lost_head));
    assert_eq!(store.shelf.0.read_now(|c| chain::head_at(c, 0)).unwrap(), Some([0; 32]), "the chain's start");
    drop((store, assets));

    // A copy of it, then a further control fact; the copy put back is found out.
    let old = base.join("old");
    copy(&dir, &old);
    let (mut store, assets) = open(&dir);
    change(&mut store);
    drop((store, assets));
    let witness_file = base.join("hub-witness");
    let newer = fastctx_platform::disk::read(fastctx_platform::disk::read_dir(&witness_file).unwrap().next().unwrap().unwrap().path()).unwrap();
    retry(|| fastctx_platform::disk::rename(&dir, base.join("newer")));
    retry(|| fastctx_platform::disk::rename(&old, &dir));
    let (_, assets) = open(&dir);
    let why = assets.sealed().expect("sealed");
    assert!(why.contains("older than what it last wrote outside it"), "{why}");
    assert!(assets.checkpoint().unwrap().is_none(), "nothing is checkpointed while sealed");
    drop(assets);
    let after = fastctx_platform::disk::read(fastctx_platform::disk::read_dir(&witness_file).unwrap().next().unwrap().unwrap().path()).unwrap();
    // The chain head it holds is left as the evidence of the lost history; the count of recoveries in it goes up
    // with the seal (§29 2.2, block 54).
    let (newer, after): (serde_json::Value, serde_json::Value) = (serde_json::from_slice(&newer).unwrap(), serde_json::from_slice(&after).unwrap());
    assert_eq!((&after["seq"], &after["head"]), (&newer["seq"], &newer["head"]), "the witness's head is left as the evidence of the lost history");
    assert!(after["recoveries"].as_u64().unwrap_or(0) > newer["recoveries"].as_u64().unwrap_or(0), "the recovery counted outside the directory too");

    // An owner library put back alone is found out too.
    retry(|| fastctx_platform::disk::remove_dir_all(&dir));
    retry(|| fastctx_platform::disk::rename(base.join("newer"), &dir));
    let (store, assets) = open(&dir);
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    let library_dir = dir.join("assets").join("account").join("root.1");
    let library_old = base.join("library-old");
    let library = assets.library(&owner).unwrap();
    drop(library);
    assets.owners.sweep();
    drop((store, assets));
    copy(&library_dir, &library_old);
    let (store, assets) = open(&dir);
    let library = assets.library(&owner).unwrap();
    commit("b");
    let taking = Taking { period: library.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
    let frame = producer.export("s", "t", 1, 1).unwrap().remove(0);
    assert!(matches!(library.take("s", frame, &Device, &taking, Arc::new(Anyone), assets.signer()).unwrap(), Received::Applied { change: 2, .. }));
    assets.changed(owner.clone(), "s");
    assets.checkpoint().unwrap().expect("a checkpoint");
    drop((store, assets, library));
    std::thread::sleep(std::time::Duration::from_millis(300));
    copy(&library_dir, &base.join("library-newer"));
    retry(|| fastctx_platform::disk::remove_dir_all(&library_dir));
    copy(&library_old, &library_dir);
    let (_, assets) = open(&dir);
    assert!(assets.sealed().is_none(), "checked as it opens");
    assert!(assets.library(&owner).is_err());
    let why = assets.sealed().expect("sealed");
    assert!(why.contains("older than the hub's own record"), "{why}");
    drop(assets);
    // Started again as ever, it is still sealed for its recovery (AK01).
    std::thread::sleep(std::time::Duration::from_millis(300));
    let (_, assets) = open(&dir);
    assert!(assets.sealed().is_some_and(|why| why.contains("older than the hub's own record")), "kept across a restart");
    drop(assets);
    clear_seal(&dir);
    // Without the witness, the hub's own record still finds the library older (a backup copied in part).
    let witness_files: Vec<_> = fastctx_platform::disk::read_dir(&witness_file).unwrap().map(|e| e.unwrap().path()).collect();
    for file in &witness_files { retry(|| fastctx_platform::disk::remove_file(file)); }
    let (_, assets) = open(&dir);
    assert!(assets.library(&owner).is_err());
    let why = assets.sealed().expect("sealed");
    assert!(why.contains("older than the hub's own record"), "{why}");
    drop(assets);
    // A library of another hub put among this one's.
    let other = base.join("other");
    crate::init(&other, crate::New {
        name: fastctx_types::HubName::try_from("lab2".to_string()).unwrap(),
        public: vec![address],
        listen: address,
        admin: fastctx_types::AccountName::try_from("root".to_string()).unwrap(),
        password: "a long enough password 123".into(),
    })
    .unwrap();
    let (other_store, other_assets) = open(&other);
    other_assets.library(&Owner::Account("eve".into(), 1)).unwrap();
    drop((other_store, other_assets));
    std::thread::sleep(std::time::Duration::from_millis(300));
    copy(&other.join("assets").join("account").join("eve.1"), &dir.join("assets").join("account").join("eve.1"));
    retry(|| fastctx_platform::disk::remove_dir_all(&library_dir));
    copy(&base.join("library-newer"), &library_dir);
    clear_seal(&dir);
    let (_, assets) = open(&dir);
    assert!(assets.library(&owner).is_ok(), "its own library is as new as its record");
    assert!(assets.library(&Owner::Account("eve".into(), 1)).is_err());
    let why = assets.sealed().expect("sealed");
    assert!(why.contains("another hub's"), "{why}");

    // The backup marker seals by itself.
    drop(assets);
    retry(|| fastctx_platform::disk::remove_dir_all(dir.join("assets").join("account").join("eve.1")));
    std::thread::sleep(std::time::Duration::from_millis(300));
    clear_seal(&dir);
    fastctx_platform::disk::write(dir.join(BACKUP_MARKER), b"").unwrap();
    let (_, assets) = open(&dir);
    assert!(assets.sealed().expect("sealed").contains("from a backup"));
    drop(assets);
    fastctx_platform::disk::remove_file(dir.join(BACKUP_MARKER)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    // The recovery seal is kept until the hub is reopened (§29, 2.2).
    clear_seal(&dir);

    // A backup made by the tool, put back after the hub went on: sealed; reported; reopened with the history after it set aside.
    let backup_dir = base.join("bk");
    let marker = recovery::backup(&dir, &backup_dir).unwrap();
    assert!(fastctx_platform::disk::is_there(backup_dir.join(BACKUP_MARKER)));
    let (mut store, assets) = open(&dir);
    for _ in 0..3 { change(&mut store); }
    let gone = assets.witness.kept();
    assert!(gone.seq > marker.seq);
    drop((store, assets));
    std::thread::sleep(std::time::Duration::from_millis(300));
    retry(|| fastctx_platform::disk::rename(&dir, base.join("before-restore")));
    retry(|| fastctx_platform::disk::rename(&backup_dir, &dir));
    let (_, assets) = open(&dir);
    assert!(assets.sealed().is_some());
    drop(assets);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let lines = recovery::report(&dir).unwrap().expect("a report");
    assert!(lines.iter().any(|line| line.contains("a backup made")), "{lines:?}");
    std::thread::sleep(std::time::Duration::from_millis(300));
    recovery::reopen(&dir).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let (store, assets) = open(&dir);
    assert!(assets.sealed().is_none(), "{:?}", assets.sealed());
    let latest = store.shelf.0.read_now(chain::latest).unwrap().unwrap();
    assert_eq!(assets.witness.current(), latest, "the witness holds the chain as it is now");
    let gone_head = witness::unhex(&gone.head).unwrap();
    assert!(assets.closed_head(gone.seq, &gone_head).unwrap(), "the head outside the directory is of the history set aside");
    assert!(!assets.closed_head(gone.seq, &[7; 32]).unwrap(), "another head at that number is not: only what was shown");
    drop((store, assets));
    std::thread::sleep(std::time::Duration::from_millis(300));

    // A key rotation: exported first, then in use; a word under the retired key still counts.
    let (store, assets) = open(&dir);
    let old_word = assets.signer().sign(b"said before");
    let old_key = *assets.public().as_bytes();
    drop((store, assets));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let export = base.join("keyring");
    let next = recovery::rotate_prepare(&dir, &export).unwrap();
    assert!(fastctx_platform::disk::is_there(export.join("next.key")) && fastctx_platform::disk::is_there(export.join("current.key")));
    let (old, new) = recovery::rotate_finish(&dir).unwrap();
    assert_eq!(new, next);
    assert_eq!(old, witness::hex(&old_key));
    let (_, assets) = open(&dir);
    assert_ne!(*assets.public().as_bytes(), old_key);
    assert!(assets.keys().holds(&old_key));
    assert!(assets.keys().heard(b"said before", &old_word).is_some(), "a retired key's word counts");
    assert_eq!(assets.keys().rotations().len(), 1);
    drop(assets);
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    println!("hub assets drill: ok");
}
