//! A drill of letting a copy go: one of another device's frames goes, its
//! purge taking its frames; one this device wrote to stays.
use fastctx_content::v2::{Commit, Dropped, KindRecord, Library, Signer};
use fastctx_content::{Record, State};
use std::sync::Arc;

struct Device([u8; 32]);
impl Signer for Device {
    fn device(&self) -> [u8; 32] { self.0 }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
fn rec(item: &str) -> KindRecord {
    KindRecord { kind: 1, schema: 1, record: Record { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state: State::Is { body: vec![7; 4096], objects: vec![] } } }
}

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-copies-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (library, _) = Library::open(&dir).unwrap();
    let elsewhere: Arc<dyn Signer> = Arc::new(Device([1; 32]));
    for scope in ["c/h/m/theirs", "c/h/m/mine"] {
        library.install(scope, b"d".to_vec(), 1).unwrap();
        let holding = library.holding(scope).unwrap().unwrap_or(0);
        for item in ["a", "b", "c"] {
            let commit = Commit { scope: scope.into(), stream: "timeline".into(), records: vec![rec(item)], leases: vec![], durable: true, op: None, holding };
            library.raw_commit(commit, elsewhere.clone()).unwrap();
        }
    }
    let here = [9; 32];
    let size = library.copy_size("c/h/m/theirs").unwrap();
    assert!(size.frames > 0, "its frames are counted: {size:?}");
    let holding = library.holding("c/h/m/theirs").unwrap().unwrap();
    assert_eq!(library.drop_copy("c/h/m/theirs", holding + 1, &here).unwrap(), Dropped::Kept("it was held anew since it was looked at".into()));
    assert_eq!(library.drop_copy("c/h/m/theirs", holding, &here).unwrap(), Dropped::Gone);
    assert!(library.holding("c/h/m/theirs").unwrap().is_none(), "held no more");
    while library.purging().unwrap() {
        library.purge().unwrap();
    }
    assert_eq!(library.copy_size("c/h/m/theirs").unwrap(), Default::default(), "its frames went with the purge");
    // Written to by this device: kept.
    let mine = library.holding("c/h/m/mine").unwrap().unwrap();
    // Ordered here, its frames signed here: kept, whichever is found first.
    assert!(matches!(library.drop_copy("c/h/m/mine", mine, &[1; 32]).unwrap(), Dropped::Kept(_)), "kept");
    assert!(library.holding("c/h/m/mine").unwrap().is_some(), "still held");
    println!("v2 copies drill: ok");
    let _ = std::fs::remove_dir_all(&dir);
}
