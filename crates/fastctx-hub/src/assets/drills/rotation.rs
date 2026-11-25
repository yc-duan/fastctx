//! A drill that the hub writes the streams it orders on after its key rotation.
use crate::assets::*;
use fastctx_content::v2::{ConversationSentinel, Kind as _, KindRecord};
use fastctx_content::{Record, State};
use fastctx_types::AccountName;
use fastctx_types::assets::{Readers, Reply, Request};

fn open_hub(dir: &std::path::Path) -> (crate::store::Store, Assets) {
    let mut store = crate::store::Store::open_logged(dir, None).unwrap();
    let assets = Assets::open(dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    (store, assets)
}

fn sentinel(version: u64) -> KindRecord {
    let body = serde_json::json!({
        "metadata": { "kind": "sentinel", "key": "s-1", "version": version, "by": "m1", "at": "2026-11-13T00:00:00Z", "deleted": false },
        "body": { "scope": "root", "active": true, "definition": { "id": "s-1", "home": "m1", "version": version } },
    });
    KindRecord {
        kind: ConversationSentinel::KIND,
        schema: 1,
        record: Record { item: b"s-1".to_vec(), order: b"s-1".to_vec(), state: State::Is { body: serde_json::to_vec(&body).unwrap(), objects: vec![] } },
    }
}

#[test]
fn hub_streams_go_on_after_a_key_rotation() {
    let base = std::env::temp_dir().join(format!("hub-rotation-drill-{}", std::process::id()));
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
    let (store, mut assets) = open_hub(&dir);
    let key = fastctx_net::sign::SecretKey::generate().unwrap();
    let member = Arc::new(fastctx_net::directory::Member {
        name: fastctx_types::MachineName::try_from("m1".to_string()).unwrap(),
        was: Vec::new(), key: key.public(), kind: fastctx_net::directory::Kind::Computer, logins: [root.clone()].into_iter().collect(),
        home: Some(root.clone()), managed: false, code: None, group: None, direct: Vec::new(), relay: Vec::new(),
        added: jiff::Timestamp::now(), online: false, since: None, through: None, reach: None, paths: None, strain: None,
        workbench: None, settled: None,
        logins_number: None,
    });
    let mut store = store;
    store.book.directory.members.push(member);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let conv = format!("c/{}/m1/conv-1", assets.id);
    let readers = Readers::Conversation { openers: vec![root.clone()], born_on: Some(fastctx_types::MachineName::try_from("m1".to_string()).unwrap()) };
    let describe = Request::Describe { scope: conv.clone(), readers, classes: fastctx_types::assets::conversation_classes(), born: Default::default(), begun: None };
    let m1 = fastctx_types::MachineName::try_from("m1".to_string()).unwrap();
    let Reply::Described { .. } = serve::ask(&assets, &store.indexed(), &String::new, &m1, &key.public(), describe).unwrap() else { panic!("described") };
    {
        let kept = scopes::scope(&assets, &conv).unwrap().unwrap();
        let library = scopes::library(&assets, &kept).unwrap();
        mirror::apply(&assets, &kept, &library, ConversationSentinel::STREAM, vec![sentinel(1)]).unwrap();
        let (epoch, orderer) = library.last_epoch(&conv, ConversationSentinel::STREAM).unwrap().unwrap();
        assert_eq!((epoch, orderer), (0, assets.keys().frame_key()), "first ordered under the key in use");
    }
    println!("before the rotation: ok");
    // The key rotated as `fastctx hub rotate-key` does, the keyring taken up
    // again as the hub opened anew takes it.
    let keyring = dir.join("keys");
    let old = assets.keys().frame_key();
    keys::prepare(&keyring, &base.join("keyring-export")).unwrap();
    keys::finish(&keyring).unwrap();
    assets.keys = Arc::new(keys::Keys::open(&keyring).unwrap());
    assert_ne!(old, assets.keys().frame_key(), "a new key in use");
    assert!(assets.keys().holds(&old), "the old one kept, retired");
    let kept = scopes::scope(&assets, &conv).unwrap().unwrap();
    let library = scopes::library(&assets, &kept).unwrap();
    mirror::apply(&assets, &kept, &library, ConversationSentinel::STREAM, vec![sentinel(2)]).unwrap();
    let (epoch, orderer) = library.last_epoch(&conv, ConversationSentinel::STREAM).unwrap().unwrap();
    assert_eq!((epoch, orderer), (1, assets.keys().frame_key()), "handed over to the key in use, a new epoch");
    let (epochs, _) = library.succession(&conv, ConversationSentinel::STREAM).unwrap();
    assert_eq!(epochs.len(), 2, "both epochs kept for the devices to follow");
    // And on again under it, no further epoch.
    mirror::apply(&assets, &kept, &library, ConversationSentinel::STREAM, vec![sentinel(3)]).unwrap();
    assert_eq!(library.last_epoch(&conv, ConversationSentinel::STREAM).unwrap().unwrap().0, 1);
    println!("hub rotation drill: ok");
}
