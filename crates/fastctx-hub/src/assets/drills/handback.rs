//! A drill of §28.38: a hub put back from a backup is handed back what it signed after, and takes the longest chain back as it reopens.
//! Each phase runs in a process of its own: a hub's databases are let go only as its process ends.
use crate::assets::*;
use fastctx_content::v2::{ConversationPlan, Kind as _, KindRecord, RawFrame, Signer};
use fastctx_content::{Record as Item, State};
use fastctx_types::AccountName;
use fastctx_types::account_records::{Metadata, Record};
use fastctx_types::assets::{self as wire, Readers, Reply, Request};
use std::path::{Path, PathBuf};

fn open_hub(dir: &Path) -> (crate::store::Store, Assets) {
    let mut store = crate::store::Store::open_logged(dir, None).unwrap();
    let assets = Assets::open(dir, store.hub.id.as_str(), store.shelf.0.clone()).unwrap();
    store.witness = Some(assets.witness.clone());
    (store, assets)
}

fn plan(version: u64, text: &str) -> Record {
    Record {
        metadata: Metadata { kind: "plan".into(), key: "conv-1".into(), version, by: Some(machine()), at: jiff::Timestamp::now(), deleted: false },
        body: serde_json::json!({ "text": text }),
    }
}

fn machine() -> fastctx_types::MachineName {
    fastctx_types::MachineName::try_from("m1".to_string()).unwrap()
}

fn root() -> AccountName {
    AccountName::try_from("root".to_string()).unwrap()
}

struct Service<'a>(&'a keys::Keys);
impl Signer for Service<'_> {
    fn device(&self) -> [u8; 32] {
        self.0.frame_key()
    }
    fn key(&self) -> [u8; 32] {
        self.0.frame_key()
    }
    fn sign(&self, header: &[u8]) -> Vec<u8> {
        self.0.sign_frame(header)
    }
}

fn wired(frame: &RawFrame) -> wire::Frame {
    wire::Frame::whole(frame.header.clone(), frame.signature.clone(), frame.body.clone())
}

/// A frame as the phases keep it: header, signature, body and objects.
type FrameKept = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<[u8; 32]>);

/// What the phases share, kept in the drill's directory.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Shared {
    key: Vec<u8>,
    conv: String,
    opened_ns: i128,
    frames: Vec<FrameKept>,
    objects: Vec<Vec<(Vec<u8>, Vec<u8>)>>,
    live_next: u64,
    live_head: [u8; 32],
}

fn shared(base: &Path) -> Shared {
    serde_json::from_slice(&fastctx_platform::disk::read(base.join("shared.json")).unwrap()).unwrap()
}

fn keep(base: &Path, shared: &Shared) {
    fastctx_platform::disk::write(base.join("shared.json"), serde_json::to_vec(shared).unwrap()).unwrap();
}

fn plan_path(slot: &Path) -> PathBuf {
    let digest = <sha2::Sha256 as sha2::Digest>::digest("plan\0conv-1".as_bytes());
    slot.join(format!("{digest:x}.json"))
}

#[test]
fn a_put_back_hub_takes_back_what_devices_hand_back() {
    let base = std::env::temp_dir().join(format!("hub-handback-drill-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&base);
    fastctx_platform::disk::create_dir_all(&base).unwrap();
    for phase in 1..=8 {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["assets::drills::handback::phase", "--exact", "--nocapture", "--test-threads", "1"])
            .env("HANDBACK_PHASE", phase.to_string())
            .env("HANDBACK_BASE", &base)
            .status()
            .unwrap();
        assert!(status.success(), "phase {phase} failed");
    }
    println!("hub hand-back drill: ok");
    let _ = fastctx_platform::disk::remove_dir_all(&base);
}

#[test]
fn phase() {
    let (Ok(phase), Ok(base)) = (std::env::var("HANDBACK_PHASE"), std::env::var("HANDBACK_BASE")) else { return };
    let base = PathBuf::from(base);
    let dir = base.join("live").join("hub");
    let restored = base.join("restored").join("hub");
    match phase.as_str() {
        "1" => first(&base, &dir),
        "2" => {
            recovery::backup(&dir, &restored).unwrap();
            println!("phase 2: backup taken");
        }
        "3" => live_goes_on(&base, &dir),
        "4" => hand_back_while_sealed(&base, &restored),
        "5" => {
            let lines = recovery::report(&restored).unwrap().unwrap();
            for line in &lines {
                println!("report: {line}");
            }
            assert!(lines.iter().any(|line| line.starts_with("handed back: 3 frame(s) of plan")), "the report says what was handed back");
            assert!(lines.iter().any(|line| line.starts_with("not kept:")), "and what was not kept");
        }
        "6" => {
            // Taken back, then cut short before it noted how far and before
            // the seal came off: the reopening after goes on from the library.
            let (store, assets) = open_hub(&restored);
            let lines = handback::take_back(&restored, &store.book.directory, &assets, assets.restores() + 1).unwrap();
            assert!(lines.iter().any(|line| line.contains("took back 2 frame(s)")), "two frames taken back: {lines:?}");
            assets
                .control
                .write_now(256, |c| {
                    c.execute("UPDATE recovery_results SET applied_next = tail_next, applied_head = tail_head, frames = 0, repaired = 0", [])?;
                    Ok(())
                })
                .unwrap();
            assert!(assets.sealed().is_some(), "still sealed");
            println!("phase 6: taken back, the note of it lost");
        }
        "7" => {
            let reopened = recovery::reopen_confirmed(&restored, |_| Ok(true)).unwrap().unwrap().1.unwrap();
            for line in &reopened.taken_back {
                println!("taken back: {line}");
            }
            assert!(reopened.taken_back.iter().any(|line| line.contains("took back 0 frame(s)")), "nothing taken twice");
            assert!(reopened.taken_back.iter().any(|line| line.contains("wrote back 0")), "nothing written back twice");
        }
        "8" => after_reopening(&base, &restored),
        other => panic!("no phase {other}"),
    }
}

fn first(base: &Path, dir: &Path) {
    let address: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    crate::init(dir, crate::New {
        name: fastctx_types::HubName::try_from("lab".to_string()).unwrap(),
        public: vec![address], listen: address, admin: root(), password: "a long enough password 123".into(),
    })
    .unwrap();
    let key = fastctx_net::sign::SecretKey::generate().unwrap();
    let (mut store, assets) = open_hub(dir);
    let member = Arc::new(fastctx_net::directory::Member {
        name: machine(),
        was: Vec::new(), key: key.public(), kind: fastctx_net::directory::Kind::Computer, logins: [root()].into_iter().collect(),
        home: Some(root()), managed: false, code: None, group: None, direct: Vec::new(), relay: Vec::new(),
        added: jiff::Timestamp::now(), online: false, since: None, through: None, reach: None, paths: None, strain: None,
        workbench: None, settled: None,
    });
    store.book.directory.members.push(member);
    store.book_edit = crate::store::BookEdit::Full;
    store.save_book().unwrap();
    let opened = store.book.directory.account(&root()).unwrap().opened;
    let conv = format!("c/{}/m1/conv-1", assets.id);
    let readers = Readers::Conversation { openers: vec![root()], born_on: Some(machine()) };
    let describe = Request::Describe { scope: conv.clone(), readers, classes: fastctx_types::assets::conversation_classes() };
    let Reply::Described { .. } = serve::ask(&assets, &store.indexed(), &String::new, &machine(), &key.public(), describe).unwrap() else { panic!("described") };
    // The plan's first version, kept where the hub keeps account records
    // and written into the conversation's scope.
    let slot = dir.join("account-records").join("root").join(opened.as_nanosecond().to_string());
    fastctx_platform::disk::create_dir_all(&slot).unwrap();
    let first = plan(1, "the first plan");
    fastctx_platform::disk::write(plan_path(&slot), serde_json::to_vec(&first).unwrap()).unwrap();
    mirror::document(&assets, &root(), opened, &first).unwrap();
    keep(base, &Shared { key: key.public().as_bytes().to_vec(), conv, opened_ns: opened.as_nanosecond(), ..Default::default() });
    println!("phase 1: hub made, the plan's first version written");
}

fn live_goes_on(base: &Path, dir: &Path) {
    let mut kept = shared(base);
    let opened = jiff::Timestamp::from_nanosecond(kept.opened_ns).unwrap();
    let (_store, assets) = open_hub(dir);
    mirror::document(&assets, &root(), opened, &plan(2, "the second plan")).unwrap();
    // Too large for a record's body: carried in an object.
    mirror::document(&assets, &root(), opened, &plan(3, &"x".repeat(80 * 1024))).unwrap();
    let scope = scopes::scope(&assets, &kept.conv).unwrap().unwrap();
    let library = scopes::library(&assets, &scope).unwrap();
    let frames = library.export(&kept.conv, ConversationPlan::STREAM, 0, 100).unwrap();
    assert_eq!(frames.len(), 3, "three frames on the live hub");
    for frame in &frames {
        let mut bytes = Vec::new();
        for hash in &frame.objects {
            let size = library.size(hash).unwrap().unwrap();
            let mut read = Vec::new();
            while (read.len() as u64) < size {
                read.extend(library.read(hash, read.len() as u64, fastctx_content::v2::READ_BYTES).unwrap());
            }
            bytes.push((hash.to_vec(), read));
        }
        kept.objects.push(bytes);
        kept.frames.push((frame.header.clone(), frame.signature.clone(), frame.body.clone(), frame.objects.clone()));
    }
    assert_eq!(kept.objects[2].len(), 1, "the third frame refers to its object");
    let tail = library.tail(&kept.conv, ConversationPlan::STREAM).unwrap().unwrap();
    (kept.live_next, kept.live_head) = (tail.next, tail.head);
    keep(base, &kept);
    println!("phase 3: the live hub went on to {}", tail.next);
}

fn hand_back_while_sealed(base: &Path, restored: &Path) {
    let kept = shared(base);
    let key = fastctx_net::sign::PublicKey::from_bytes(kept.key.as_slice().try_into().unwrap());
    let frames: Vec<RawFrame> = kept
        .frames
        .iter()
        .map(|(header, signature, body, objects)| RawFrame { header: header.clone(), signature: signature.clone(), body: body.clone(), objects: objects.clone() })
        .collect();
    let objects = |index: usize| -> Vec<(wire::Bytes, wire::Bytes)> {
        kept.objects[index].iter().map(|(hash, bytes)| (hash.clone().into(), bytes.clone().into())).collect()
    };
    let conv = kept.conv.clone();
    let (store, assets) = open_hub(restored);
    let why = assets.sealed().expect("sealed");
    assert!(!Assets::is_start_seal(&why), "sealed for its recovery: {why}");
    let recovery = assets.restores() + 1;
    let indexed = store.indexed();
    let m1 = machine();
    let ask = |request| serve::ask(&assets, &indexed, &String::new, &m1, &key, request).unwrap();
    // Opening, a device is told the recovery it hands back for.
    let open = Request::Open { version: wire::VERSION, seen: None, reported: Vec::new(), pinned: None, lineage: Vec::new() };
    match ask(open) {
        Reply::Sealed { recovery: Some(said), .. } => assert_eq!(said, recovery),
        other => panic!("not sealed for a recovery: {other:?}"),
    }
    // Where the hub stands, against where the device does.
    let last = fastctx_content::v2::Header::decode(&frames[2].header).unwrap();
    let cursor = wire::Cursor { stream: "plan".into(), sequence: last.first + u64::from(last.count) - 1, head: last.chain().into() };
    let standing = ask(Request::Cursors { scope: conv.clone(), holding: 1, streams: vec![cursor] });
    let Reply::Standing { streams } = &standing else { panic!("no standing: {standing:?}") };
    assert!(!streams[0].agrees && streams[0].ends.len() == 1 && streams[0].ends[0].0 == 0, "the hub ends at the first frame: {streams:?}");
    let rechecked = serve::recheck(&assets, &indexed, &m1, &conv, None, standing.clone()).unwrap();
    assert!(matches!(rechecked, Reply::Standing { .. }), "where it stands passes the seal");
    // A scope the hub put back does not hold: nothing to hand back of it,
    // and a frame handed back of it is not kept, its name not counted.
    let unknown = format!("c/{}/m1/claude-5a1ce0ff", assets.id);
    let cursor = wire::Cursor { stream: "plan".into(), sequence: 0, head: [0u8; 32].into() };
    let standing = ask(Request::Cursors { scope: unknown.clone(), holding: 1, streams: vec![cursor] });
    assert!(matches!(&standing, Reply::Standing { streams } if streams.is_empty()), "nothing of a scope it does not hold: {standing:?}");
    let reply = ask(Request::HandBack { scope: unknown.clone(), stream: "plan".into(), frame: wired(&frames[1]), objects: Vec::new() });
    assert!(matches!(reply, Reply::HandedBack { kept: false, .. }), "not kept of a scope it does not hold");
    let counted: i64 = assets
        .control
        .read_now(|c| Ok(c.query_row("SELECT COUNT(*) FROM recovery_refusals WHERE scope LIKE '%5a1ce0ff'", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(counted, 0, "a scope it does not hold is not counted");
    // An object the frame does not refer to is refused before anything is
    // kept of it.
    let stray = vec![(wire::Bytes::from(vec![0u8; 32]), wire::Bytes::from(b"not referred to".to_vec()))];
    let reply = ask(Request::HandBack { scope: conv.clone(), stream: "plan".into(), frame: wired(&frames[1]), objects: stray });
    assert!(matches!(reply, Reply::HandedBack { kept: false, .. }), "a stray object is refused");
    // Asked anything else, it is sealed.
    let other = ask(Request::Streams { scope: conv.clone() });
    assert!(matches!(other, Reply::Sealed { .. }), "nothing else is served: {other:?}");
    let hand = |index: usize, objects: Vec<(wire::Bytes, wire::Bytes)>| {
        ask(Request::HandBack { scope: conv.clone(), stream: "plan".into(), frame: wired(&frames[index]), objects })
    };
    // The frame the hub holds already: before its tail.
    assert!(matches!(hand(0, Vec::new()), Reply::HandedBack { kept: false, .. }), "a frame before the tail is not kept");
    // Not signed by a key of the hub's keyring.
    let mut forged = frames[1].clone();
    forged.signature[0] ^= 1;
    let reply = ask(Request::HandBack { scope: conv.clone(), stream: "plan".into(), frame: wired(&forged), objects: Vec::new() });
    assert!(matches!(reply, Reply::HandedBack { kept: false, .. }), "a forged frame is not kept: {reply:?}");
    // The third before the second: it follows nothing kept yet.
    assert!(matches!(hand(2, objects(2)), Reply::HandedBack { kept: false, .. }), "a frame past a gap is not kept");
    let reply = hand(1, Vec::new());
    assert!(matches!(reply, Reply::HandedBack { kept: true, .. }), "the second is kept: {reply:?}");
    let rechecked = serve::recheck(&assets, &indexed, &m1, &conv, Some("plan"), reply).unwrap();
    assert!(matches!(rechecked, Reply::HandedBack { kept: true, .. }), "what the hub made of it passes the seal");
    // Without its object the third is not kept; with it, it is.
    assert!(matches!(hand(2, Vec::new()), Reply::HandedBack { kept: false, .. }), "a frame without its object is not kept");
    let reply = hand(2, objects(2));
    assert!(matches!(reply, Reply::HandedBack { kept: true, .. }), "the third with its object is kept: {reply:?}");
    assert!(matches!(hand(2, objects(2)), Reply::HandedBack { kept: true, .. }), "handed back again, it stands kept");
    // Another device's branch from the same tail, one frame long, signed by
    // the hub's key: kept, not the longest.
    let scope = scopes::scope(&assets, &conv).unwrap().unwrap();
    let library = handback::library_as_is(&assets, &scope).unwrap();
    let other = plan(2, "another second plan");
    let records = vec![KindRecord {
        kind: ConversationPlan::KIND,
        schema: 1,
        record: Item { item: b"plan".to_vec(), order: b"plan".to_vec(), state: State::Is { body: serde_json::to_vec(&other).unwrap(), objects: Vec::new() } },
    }];
    let branch = library.hub_frames(&conv, "plan", records, &Service(assets.keys())).unwrap();
    let reply = ask(Request::HandBack { scope: conv.clone(), stream: "plan".into(), frame: wired(&branch[0]), objects: Vec::new() });
    assert!(matches!(reply, Reply::HandedBack { kept: true, .. }), "the other branch is kept: {reply:?}");
    assert_eq!(library.tail(&conv, "plan").unwrap().unwrap().next, 1, "nothing taken while sealed");
    println!("phase 4: handed back while sealed (recovery {recovery})");
}

fn after_reopening(base: &Path, restored: &Path) {
    let kept = shared(base);
    let conv = kept.conv.clone();
    let (store, assets) = open_hub(restored);
    assert!(assets.sealed().is_none(), "reopened");
    let scope = scopes::scope(&assets, &conv).unwrap().unwrap();
    let library = scopes::library(&assets, &scope).unwrap();
    let tail = library.tail(&conv, "plan").unwrap().unwrap();
    assert_eq!((tail.next, tail.head), (kept.live_next, kept.live_head), "the longest chain taken back: the live hub's");
    let item = library.item(&conv, "plan", b"plan").unwrap().unwrap();
    let said: serde_json::Value = serde_json::from_slice(&item.body).unwrap();
    assert_eq!(said["metadata"]["version"], 3, "the plan reads at its third version");
    let row: (i64, i64, i64, Option<String>) = assets
        .control
        .read_now(|c| {
            Ok(c.query_row("SELECT frames, branches, repaired, stopped FROM recovery_results WHERE stream = 'plan'", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?)
        })
        .unwrap();
    assert_eq!(row, (2, 1, 1, None), "two frames taken back, one branch left, the source written back");
    let left: i64 = assets.control.read_now(|c| Ok(c.query_row("SELECT COUNT(*) FROM recovery_frames", [], |row| row.get(0))?)).unwrap();
    assert_eq!(left, 0, "what was handed back is cleared as the hub reopens");
    // Open now, a frame handed back is answered not kept.
    let key = fastctx_net::sign::PublicKey::from_bytes(kept.key.as_slice().try_into().unwrap());
    let (header, signature, body, _) = &kept.frames[1];
    let request = Request::HandBack {
        scope: conv.clone(),
        stream: "plan".into(),
        frame: wire::Frame::whole(header.clone(), signature.clone(), body.clone()),
        objects: Vec::new(),
    };
    let reply = serve::ask(&assets, &store.indexed(), &String::new, &machine(), &key, request).unwrap();
    assert!(matches!(reply, Reply::HandedBack { kept: false, .. }), "an open hub keeps nothing handed back: {reply:?}");
    // The account record written back, its earlier versions kept as history.
    let opened = jiff::Timestamp::from_nanosecond(kept.opened_ns).unwrap();
    let slot = restored.join("account-records").join("root").join(opened.as_nanosecond().to_string());
    let path = plan_path(&slot);
    let current: Record = serde_json::from_slice(&fastctx_platform::disk::read(&path).unwrap()).unwrap();
    assert_eq!(current.metadata.version, 3, "the plan's record is at its third version");
    assert_eq!(current.body["text"].as_str().map(str::len), Some(80 * 1024), "read whole from its object");
    let versions = path.with_extension("versions");
    assert!(fastctx_platform::disk::is_there(versions.join("1.json")) && fastctx_platform::disk::is_there(versions.join("2.json")), "the first and second kept as history");
    println!("phase 6: taken back and written back");
}
