//! A drill of a stream's window and what fills in below it (design §9, §28.2): producer -> hub ->
// a full device that installs the window and fills in, and a light one that pages down; not kept.
use fastctx_content::v2::{Authorizer, Commit, Entry, Header, Holder, HubSigner, Imported, Installed, KindRecord, Library, OlderTaken, Signer, Taking, Verifier, ViewChange};
use fastctx_content::{ObjectRef, Record, State};
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
impl Verifier for Fake {
    fn verify(&self, _: &Header, bytes: &[u8], signature: &[u8]) -> bool { bytes == signature }
}
impl HubSigner for Fake {
    fn sign(&self, statement: &[u8]) -> Vec<u8> { statement[..8].to_vec() }
}
struct All;
impl Authorizer for All {
    fn authorize(&self, _: &Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> { Ok(()) }
    fn knows(&self, _: u16, _: u16) -> bool { true }
}
struct AnyHub;
impl fastctx_content::v2::HubVerifier for AnyHub { fn verify(&self, _: &[u8], _: &[u8]) -> bool { true } }
fn epoch0() -> fastctx_content::v2::Signed {
    let fact = fastctx_content::v2::EpochFact { scope: "s".into(), stream: "t".into(), epoch: 0, device: [1; 32], key: [2; 32], start: 0, kind: fastctx_content::v2::EpochKind::Source, predecessor: None, follows: [0; 32] };
    fastctx_content::v2::Signed { fact: fact.encode(), signature: vec![] }
}
fn is(item: &str, order: &str, body: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.into(), order: order.into(), state: State::Is { body: body.into(), objects: vec![] } } }
}
fn gone(item: &str, order: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.into(), order: order.into(), state: State::Gone } }
}
/// Every live item as (identity, order, body).
fn all(lib: &Library) -> Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let (mut out, mut after) = (Vec::new(), None);
    loop {
        let page = lib.page("s", "t", after.clone(), 500).unwrap();
        let Some(last) = page.last() else { break };
        after = Some((last.order.clone(), last.item.clone()));
        let full = page.len() == 500;
        out.extend(page.into_iter().map(|e| (e.item, e.order, e.body)));
        if !full { break; }
    }
    out
}

pub(crate) fn run() {
    let base = std::env::temp_dir().join(format!("v2-snapshot-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["p", "h", "d", "l", "b"] { std::fs::create_dir_all(base.join(d)).unwrap(); }
    let (p, _) = Library::open(&base.join("p")).unwrap();
    let (h, _) = Library::open_hub(&base.join("h")).unwrap();
    let (d, _) = Library::open(&base.join("d")).unwrap();
    let (l, _) = Library::open(&base.join("l")).unwrap();
    for lib in [&p, &h, &d, &l] { lib.install("s", b"d".to_vec(), 1).unwrap(); }
    for lib in [&d, &l] { lib.accept_epoch("s", epoch0(), &AnyHub).unwrap(); }
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    let hub_signer: Arc<dyn HubSigner> = Arc::new(Fake);
    let anyone = |_: &ObjectRef| true;
    let commit = |records: Vec<KindRecord>| {
        let holding = p.holding("s").unwrap().unwrap();
        p.raw_commit(Commit { scope: "s".into(), stream: "t".into(), records, leases: vec![], durable: true, op: None, holding }, signer.clone()).unwrap();
    };
    let ship = || {
        let from = h.streams("s").unwrap().into_iter().find(|(name, _)| name == "t").map_or(0, |(_, next)| next);
        for frame in p.export("s", "t", from, 1000).unwrap() {
            let taking = Taking { period: h.holding("s").unwrap().unwrap(), readable: &anyone, proven: &|_: &[u8; 32]| false };
            h.take("s", frame, &Fake, &taking, Arc::new(All), hub_signer.clone()).unwrap();
        }
    };
    let import = |lib: &Library, from: u64, frames: usize| {
        let holding = lib.holding("s").unwrap().unwrap();
        for frame in h.export("s", "t", from, frames).unwrap() {
            let done = lib.import("s", frame, &Fake, Holder::Replica, holding).unwrap();
            assert!(matches!(done, Imported::Applied { .. }), "{done:?}");
        }
    };
    // 300 items, ten a commit.
    for c in 0..30 {
        commit((0..10).map(|i| { let k = format!("k{:03}", c * 10 + i); is(&k, &k, &format!("v1 {k}")) }).collect());
    }
    ship();
    // The device had the first 27 frames: k000..k269.
    import(&d, 0, 27);
    let c_seq = d.streams("s").unwrap().into_iter().find(|(n, _)| n == "t").unwrap().1;
    assert_eq!(c_seq, 270);
    // Then, beyond it: one changed below where the window will be, one deleted there; in its keys one changed,
    // one deleted, one moved below it, one moved into it; and new ones.
    commit(vec![is("k010", "k010", "v2 k010"), gone("k020", "k020"), is("k230", "k230", "v2 k230"), gone("k240", "k240"),
                is("k250", "a250", "v2 k250 moved down"), is("k030", "z030", "v2 k030 moved up")]);
    commit((300..310).map(|i| { let k = format!("k{i:03}"); is(&k, &k, &format!("v1 {k}")) }).collect());
    ship();
    let window = h.window("s", "t", 100, 1 << 20).unwrap().unwrap();
    assert_eq!(window.entries.len(), 100);
    assert_eq!(window.entries[0].order, b"z030".to_vec(), "newest key first");
    let low = window.low.clone().expect("the hub holds more below");
    let statement = window.statement();
    let mut tampered = window.clone();
    tampered.entries[3].body = b"forged".to_vec();
    assert_ne!(tampered.statement(), statement, "the word covers every entry");
    // A device that held the first 270 and went light: the window, then pages down.
    let holding = d.holding("s").unwrap().unwrap();
    let before_window = d.view_revision("s", "t").unwrap().expect("held");
    assert_eq!(d.install_window(window.clone(), holding, false).unwrap(), Installed::Installed);
    let in_keys = |rows: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>| rows.into_iter().filter(|(item, order, _)| (order.clone(), item.clone()) >= low).collect::<Vec<_>>();
    assert_eq!(in_keys(all(&d)), in_keys(all(&h)), "the window's keys as the hub has them");
    let here = all(&d);
    assert!(!here.iter().any(|(item, _, _)| item == b"k240"), "deleted in its keys: gone");
    assert!(!here.iter().any(|(item, _, _)| item == b"k250"), "moved below it: not at its old place, not yet at its new");
    assert!(here.iter().any(|(item, _, body)| item == b"k020" && body == b"v1 k020"), "below it, as it was until read again");
    // Older records came in, and a reader reads on from where it stood, by the rows written here (PH04).
    let read = d.view_changes("s", "t", before_window, 1000, 1 << 20).unwrap().expect("read on, not anew");
    assert!(read.changes.iter().any(|c| matches!(c, ViewChange::Gone { item } if item == b"k240")), "what the window has not, told gone");
    assert!(read.changes.iter().any(|c| matches!(c, ViewChange::Is(e) if e.item == b"k230")), "what it has, told");
    let mut before = window.low.clone();
    loop {
        let asked = d.page_token().unwrap();
        let page = h.older("s", "t", before.clone(), 40).unwrap().unwrap();
        before = page.entries.last().map(|e| (e.order.clone(), e.item.clone()));
        let more = page.more;
        assert_eq!(d.install_older(page, holding, asked).unwrap(), OlderTaken::Taken);
        if !more { break; }
    }
    assert_eq!(all(&d), all(&h), "paged down: the whole stream as the hub has it, k250 at its new place, k020 gone");
    let now = d.view_revision("s", "t").unwrap().unwrap();
    assert!(now > before_window, "a reader's place only grows");
    let latest = d.view_page("s", "t", &fastctx_content::Anchor::Latest, 5, 1 << 20).unwrap();
    let tail: Vec<_> = all(&d).into_iter().rev().take(5).rev().map(|(item, _, _)| item).collect();
    assert_eq!(latest.entries.iter().map(|e| e.item.clone()).collect::<Vec<_>>(), tail, "the newest five, earliest first");
    assert!(latest.older.is_some() && latest.newer.is_none());
    let more = d.view_page("s", "t", &fastctx_content::Anchor::Before(latest.older.clone().unwrap()), 5, 1 << 20).unwrap();
    assert_eq!(more.entries.len(), 5);
    assert!(more.entries.last().unwrap().order <= latest.entries.first().unwrap().order);
    // The stream goes on from the head as ever.
    let stale = h.window("s", "t", 100, 1 << 20).unwrap().unwrap();
    commit((310..320).map(|i| { let k = format!("k{i:03}"); is(&k, &k, &format!("v1 {k}")) }).collect());
    ship();
    import(&d, window.next, 1000);
    assert_eq!(all(&d), all(&h), "after the head as before");
    // The tail passed the window's floors: they went, and a reader that may not have read them reads anew.
    assert!(d.view_changes("s", "t", now, 100, 1 << 20).unwrap().is_none(), "floors let go: read anew");
    let anew = d.view_revision("s", "t").unwrap().unwrap();
    assert!(anew > now, "a reader's place only grows");
    commit((320..330).map(|i| { let k = format!("k{i:03}"); is(&k, &k, &format!("v1 {k}")) }).collect());
    ship();
    let at = d.streams("s").unwrap().into_iter().find(|(n, _)| n == "t").unwrap().1;
    import(&d, at, 1000);
    let changed = d.view_changes("s", "t", anew, 100, 1 << 20).unwrap().expect("the same era");
    let names: Vec<_> = changed.changes.iter().map(|c| match c { fastctx_content::v2::ViewChange::Is(e) => e.item.clone(), fastctx_content::v2::ViewChange::Gone { item } => item.clone() }).collect();
    assert_eq!(names, (320..330).map(|i| format!("k{i:03}").into_bytes()).collect::<Vec<_>>(), "what the tail brought, in order");
    assert_eq!(changed.through, d.view_revision("s", "t").unwrap().unwrap());
    // E02: a window from before what the device holds now is nothing to it.
    assert_eq!(d.install_window(stale, holding, false).unwrap(), Installed::Behind);
    // G01: one at the head here, the hub's records indexed again, is taken all the same when asked to.
    let same = h.window("s", "t", 100, 1 << 20).unwrap().unwrap();
    assert_eq!(d.install_window(same.clone(), holding, false).unwrap(), Installed::Behind);
    assert_eq!(d.install_window(same, holding, true).unwrap(), Installed::Installed);
    assert_eq!(all(&d), all(&h));
    // Cut back, the stream is read anew from a place no earlier one shares, and only grows (JJ02).
    let here = d.view_revision("s", "t").unwrap().unwrap();
    d.truncate("s", "t", Some(9)).unwrap();
    assert!(d.view_changes("s", "t", here, 10, 1 << 20).unwrap().is_none(), "cut back: read anew");
    assert!(d.view_revision("s", "t").unwrap().unwrap() > here, "a reader's place only grows");
    // A light device from nothing: the window, then pages down to the first.
    let lh = l.holding("s").unwrap().unwrap();
    let first = h.window("s", "t", 50, 1 << 20).unwrap().unwrap();
    let old_asked = l.page_token().unwrap();
    let old_page = h.older("s", "t", first.low.clone(), 40).unwrap().unwrap();
    assert_eq!(l.install_window(first.clone(), lh, false).unwrap(), Installed::Installed);
    let covered = l.coverage("s", "t").unwrap();
    assert_eq!(covered.len(), 1, "the window covers its keys up: {covered:?}");
    assert_eq!((covered[0].high.clone(), covered[0].through), (None, first.next));
    let mut before = first.low.clone();
    loop {
        let asked = l.page_token().unwrap();
        let page = h.older("s", "t", before.clone(), 40).unwrap().unwrap();
        before = page.entries.last().map(|e| (e.order.clone(), e.item.clone()));
        let more = page.more;
        assert_eq!(l.install_older(page, lh, asked).unwrap(), OlderTaken::Taken);
        if !more { break; }
    }
    assert_eq!(all(&l), all(&h), "a light device paged down holds what the hub holds");
    assert!(l.coverage("s", "t").unwrap().iter().any(|c| c.low == (Vec::new(), Vec::new())), "paged down to the first: covered from below everything");
    // E03: a page from before a later window of the light device is dropped, nothing of it brought back.
    commit(vec![gone("k100", "k100")]);
    ship();
    let later = h.window("s", "t", 50, 1 << 20).unwrap().unwrap();
    assert_eq!(l.install_window(later.clone(), lh, false).unwrap(), Installed::Installed);
    assert_eq!(l.install_older(old_page, lh, old_asked).unwrap(), OlderTaken::Before, "a page from before the window began is dropped");
    let asked = l.page_token().unwrap();
    let fresh = h.older("s", "t", None, 500).unwrap().unwrap();
    assert_eq!(l.install_older(fresh, lh, asked).unwrap(), OlderTaken::Taken);
    assert!(!all(&l).iter().any(|(item, _, _)| item == b"k100"));
    assert_eq!(all(&l), all(&h));
    // PH01: a page answered before a later one showed an item gone does not bring it back, whatever its revision;
    // one answered after it came back does.
    let reader = l.view_revision("s", "t").unwrap().unwrap();
    commit(vec![is("k005", "k005", "v3 k005")]);
    ship();
    let late_asked = l.page_token().unwrap();
    let late = h.older("s", "t", None, 500).unwrap().unwrap();
    assert!(late.entries.iter().any(|e| e.item == b"k005" && e.body == b"v3 k005"));
    commit(vec![gone("k005", "k005")]);
    ship();
    let asked = l.page_token().unwrap();
    let showing_gone = h.older("s", "t", None, 500).unwrap().unwrap();
    assert_eq!(l.install_older(showing_gone, lh, asked).unwrap(), OlderTaken::Taken);
    assert_eq!(l.install_older(late, lh, late_asked).unwrap(), OlderTaken::Taken);
    assert!(!all(&l).iter().any(|(item, _, _)| item == b"k005"), "the late page undid nothing");
    let read = l.view_changes("s", "t", reader, 1000, 1 << 20).unwrap().expect("pages taken in: read on");
    assert!(read.changes.iter().any(|c| matches!(c, ViewChange::Gone { item } if item == b"k005")));
    commit(vec![is("k005", "k005", "v4 k005")]);
    ship();
    let asked = l.page_token().unwrap();
    let back = h.older("s", "t", None, 500).unwrap().unwrap();
    assert_eq!(l.install_older(back, lh, asked).unwrap(), OlderTaken::Taken);
    assert!(all(&l).iter().any(|(item, _, body)| item == b"k005" && body == b"v4 k005"), "come back: told as it is");
    assert_eq!(all(&l), all(&h));
    // A page asked for before floors it could undo went, as the tail passed them, is asked for again.
    let stale_asked = l.page_token().unwrap();
    let stale_page = h.older("s", "t", None, 500).unwrap().unwrap();
    commit(vec![gone("k006", "k006")]);
    ship();
    let asked = l.page_token().unwrap();
    let floored = h.older("s", "t", None, 500).unwrap().unwrap();
    assert_eq!(l.install_older(floored, lh, asked).unwrap(), OlderTaken::Taken);
    commit(vec![is("k400", "k400", "v1 k400")]);
    ship();
    import(&l, later.next, 1000);
    assert_eq!(l.install_older(stale_page, lh, stale_asked).unwrap(), OlderTaken::Stale);
    assert_eq!(all(&l), all(&h), "the tail on from the window, as the hub has it");
    // P12: what a window has not, hidden a batch at a time past a thousand rows, then the tail.
    for c in 0..110 {
        commit((0..10).map(|i| { let k = format!("m{:04}", c * 10 + i); is(&k, &k, &format!("v1 {k}")) }).collect());
    }
    ship();
    let to = h.streams("s").unwrap().into_iter().find(|(n, _)| n == "t").unwrap().1;
    let here = l.streams("s").unwrap().into_iter().find(|(n, _)| n == "t").unwrap().1;
    import(&l, here, (to - here) as usize);
    assert_eq!(all(&l), all(&h));
    for c in 0..11 {
        commit((0..100).map(|i| { let k = format!("m{:04}", c * 100 + i); gone(&k, &k) }).collect());
    }
    ship();
    let whole = h.window("s", "t", 5000, 1 << 24).unwrap().unwrap();
    assert!(whole.low.is_none(), "the window is the whole stream");
    // Stopped after its first batch, as a device killed then: its rows are in, the rest left to do; frames wait.
    assert_eq!(l.install_window_stopping(whole.clone(), lh, 1).unwrap(), Installed::Installed);
    let frame = h.export("s", "t", whole.next.saturating_sub(1), 1).unwrap().into_iter().next();
    if let Some(frame) = frame {
        assert!(l.import("s", frame, &Fake, Holder::Replica, lh).is_err(), "no frame while the window is installed");
    }
    assert!(all(&l).len() > all(&h).len(), "what it has not, not all hidden yet");
    // The library opened again, as a device started again after it was killed: the intent it kept goes on from
    // the disk (codex-5c30d5e9 re-review, section 6).
    drop(l);
    let (l, _) = Library::open(&base.join("l")).unwrap();
    assert_eq!(l.holding("s").unwrap(), Some(lh), "the same holding period after opening again");
    assert_eq!(l.resume_installs().unwrap(), 1, "gone on with after the stop");
    assert_eq!(l.resume_installs().unwrap(), 0, "nothing left to do");
    assert_eq!(all(&l), all(&h), "1,100 rows hidden in two batches");
    assert_eq!(l.streams("s").unwrap().into_iter().find(|(n, _)| n == "t").unwrap().1, whole.next, "the tail at its head");
    // PH03: a light device switched to holding whole takes the frames below its window in the same holding
    // period, chained from the first to the window's head, the tail going on meanwhile.
    let (b, _) = Library::open(&base.join("b")).unwrap();
    b.install("s", b"d".to_vec(), 1).unwrap();
    b.accept_epoch("s", epoch0(), &AnyHub).unwrap();
    let bh = b.holding("s").unwrap().unwrap();
    let top = h.window("s", "t", 50, 1 << 20).unwrap().unwrap();
    assert_eq!(b.install_window(top.clone(), bh, false).unwrap(), Installed::Installed);
    let began = b.begin_backfill("s", "t", bh).unwrap().expect("its history is not whole");
    assert_eq!((began.at, began.through), (0, top.next));
    commit(vec![is("k500", "k500", "v1 k500")]);
    ship();
    import(&b, top.next, 1000);
    // Opened again before anything below the window came: the fill-in it began goes on from the disk.
    drop(b);
    let (b, _) = Library::open(&base.join("b")).unwrap();
    assert_eq!(b.holding("s").unwrap(), Some(bh), "the same holding period after opening again");
    let kept = b.backfill("s", "t").unwrap().expect("the fill-in began is kept");
    assert_eq!((kept.at, kept.through), (0, top.next));
    let mut at = 0;
    let mut whole_now = false;
    while !whole_now {
        let frames = h.export("s", "t", at, 20).unwrap();
        assert!(!frames.is_empty(), "frames below the window at {at}");
        for frame in frames {
            match b.import_backfill("s", frame, &Fake, bh).unwrap() {
                fastctx_content::v2::Backfilled::Applied { at: next } => at = next,
                fastctx_content::v2::Backfilled::Whole => { whole_now = true; break; }
                other => panic!("{other:?}"),
            }
        }
    }
    assert!(b.backfill("s", "t").unwrap().is_none(), "done");
    assert!(b.begin_backfill("s", "t", bh).unwrap().is_none(), "its history whole");
    assert_eq!(all(&b), all(&h), "the whole stream as the hub has it");
    let runs = b.coverage("s", "t").unwrap();
    assert!(runs.len() == 1 && runs[0].low == (Vec::new(), Vec::new()) && runs[0].high.is_none(), "{runs:?}");
    // §29 4: a page opened while the device is offline, then a frame taken in the background: reading on from the
    // page tells that frame and nothing anew, and the page read again has it at its place.
    let page = b.view_page("s", "t", &fastctx_content::Anchor::Latest, 5, 1 << 20).unwrap();
    let from = b.streams("s").unwrap().into_iter().find(|(n, _)| n == "t").unwrap().1;
    commit(vec![is("k901", "k901", "v1 k901 while the page was open"), gone("k310", "k310")]);
    ship();
    import(&b, from, 1000);
    let read = b.view_changes("s", "t", page.through, 1000, 1 << 20).unwrap().expect("read on from the page, not anew");
    assert!(read.changes.iter().any(|c| matches!(c, ViewChange::Is(e) if e.item == b"k901")), "the frame taken in the background, told");
    assert!(read.changes.iter().any(|c| matches!(c, ViewChange::Gone { item } if item == b"k310")), "its deletion, told");
    assert_eq!(read.changes.len(), 2, "nothing else: {:?}", read.changes);
    let again = b.view_page("s", "t", &fastctx_content::Anchor::Latest, 5, 1 << 20).unwrap();
    assert!(again.entries.iter().any(|e| e.item == b"k901"), "at its place in the page read again: {:?}", again.entries.iter().map(|e| String::from_utf8_lossy(&e.item).into_owned()).collect::<Vec<_>>());
    assert!(again.entries.windows(2).all(|w| (w[0].order.clone(), w[0].item.clone()) <= (w[1].order.clone(), w[1].item.clone())), "in the stream's order");
    let _ = std::fs::remove_dir_all(&base);
    println!("v2 snapshot drill: ok");
}
