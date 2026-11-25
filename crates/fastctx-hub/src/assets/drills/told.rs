//! A drill of what the hub tells an account (`told::audit`): a look at its
//! scope that finds it as the machine log has it writes nothing at all — no
//! place kept, none let go — while one that puts a record right keeps where
//! it was before it does, and forgets it after.
use crate::assets::*;
use fastctx_net::directory::{Kind, Member};
use fastctx_net::sign::SecretKey;
use fastctx_storage::rusqlite::params;
use fastctx_types::assets::{Readers, Reply, Request};
use fastctx_types::machine_log::{Action, Actor, Outcome};
use fastctx_types::{AccountName, MachineName};

#[test]
fn a_look_that_finds_nothing_to_put_right_writes_nothing() {
    let base = std::env::temp_dir().join(format!("hub-told-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    let dir = base.join("hub");
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = AccountName::try_from("root".to_string()).unwrap();
    crate::init(
        &dir,
        crate::New {
            name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(),
            public: vec![address],
            listen: address,
            admin: root.clone(),
            password: "a long enough password 123".into(),
        },
    )
    .unwrap();
    let mut store = crate::store::Store::open_logged(&dir, None).unwrap();
    let assets = Assets::open(&dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());

    // root's machine, and a few records of the machine log root reads.
    let key = SecretKey::generate().unwrap();
    let m1 = MachineName::try_from("m1".to_string()).unwrap();
    store.book.directory.members.push(Arc::new(Member {
        name: m1.clone(),
        was: Vec::new(),
        key: key.public(),
        kind: Kind::Computer,
        logins: [root.clone()].into_iter().collect(),
        home: Some(root.clone()),
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
    }));
    let directory = store.book.directory.clone();
    for _ in 0..3 {
        let actor = Actor { account: Some(root.clone()), authenticated: true, member: None, peer: None };
        store
            .book
            .machine_log
            .push(&directory, &[], actor, Action::PasswordFinish { target: root.clone() }, Outcome::Done)
            .unwrap();
    }
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();

    // root's records, described by its machine.
    let opened = store.book.directory.account(&root).unwrap().opened;
    let scope = fastctx_types::assets::records_key(assets.id.as_str(), &root, opened.as_millisecond());
    let describe = Request::Describe {
        scope: scope.clone(),
        readers: Readers::Account { account: root.clone() },
        classes: fastctx_types::assets::records_classes(),
        born: Default::default(),
        begun: None,
    };
    let Reply::Described { .. } =
        serve::ask(&assets, &store.indexed(), &String::new, &m1, &key.public(), describe).unwrap()
    else {
        panic!("described")
    };

    let database = store.shelf.0.clone();
    let library = assets.library(&Owner::Account("root".into(), opened.as_millisecond())).unwrap();
    let again = format!("audit/again/{scope}");
    let look = || told::audit(&assets, &database, &root, opened).unwrap();
    let kept = || library.checkpoint_in_epoch(&again).unwrap();
    let latest = || library.latest_change().unwrap().map(|(change, _)| change);

    // The first look writes what the log holds, with nothing to put right:
    // it keeps no place, and lets none go.
    look();
    let first = latest();
    assert!(first.is_some(), "the log's records written");
    assert_eq!(kept(), (None, 0), "a look with nothing to put right keeps nothing");
    // Again, the scope as the log has it: nothing is written at all.
    look();
    assert_eq!(latest(), first, "a look with nothing to put right writes nothing");
    assert_eq!(kept(), (None, 0));

    // A record root no longer reads: put right, its place kept while it
    // was, and let go after.
    let reader = crate::store::log_store::account(&root, opened);
    let gone = database
        .write_now(256, move |c| {
            Ok(c.execute(
                "DELETE FROM machine_log_readers WHERE reader = ?1
                 AND id = (SELECT id FROM machine_log_readers WHERE reader = ?1 ORDER BY id LIMIT 1 OFFSET 1)",
                params![reader],
            )?)
        })
        .unwrap();
    assert_eq!(gone, 1);
    look();
    assert!(latest() > first, "the record put right");
    assert_eq!(kept(), (None, 1), "kept while it was put right, let go after");

    // A look stopped before: the next goes on from there, and forgets it.
    library.keep_checkpoint(&again, b"{\"latest\":true}".to_vec()).unwrap();
    let before = latest();
    look();
    assert_eq!(latest(), before, "nothing more to put right");
    assert_eq!(kept(), (None, 2), "a place kept before is let go at the end");

    drop((library, store, assets));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
}
