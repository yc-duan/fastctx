//! A drill of v2 reads, the local gate and ops.
use fastctx_content::v2::{Commit, Delivery, KindRecord, LEASE, Library, Op, Signer};
use fastctx_content::{Record, State};
use std::sync::Arc;

struct Fake;
impl Signer for Fake {
    fn device(&self) -> [u8; 32] { [1; 32] }
    fn key(&self) -> [u8; 32] { [2; 32] }
    fn sign(&self, header: &[u8]) -> Vec<u8> { header.to_vec() }
}
fn rec(item: &str, order: &str, state: State) -> KindRecord {
    KindRecord { kind: 3, schema: 2, record: Record { item: item.as_bytes().to_vec(), order: order.as_bytes().to_vec(), state } }
}

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-reads-ops-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (library, _) = Library::open(&dir).unwrap();
    let signer: Arc<dyn Signer> = Arc::new(Fake);
    library.install("s", b"d".to_vec(), 1).unwrap();
    let lease = library.put(&mut vec![4u8; 70 * 1024].as_slice(), "drill", LEASE).unwrap();
    let words = Op { id: "dev-1:1".into(), digest: [9; 32], kind: "talk".into() };
    let commit = |records: Vec<KindRecord>, leases, op: Option<(Op, bool)>| Commit { scope: "s".into(), stream: "t".into(), records, leases, durable: true, op, holding: library.holding("s").unwrap().unwrap_or(0) };
    let first = library.raw_commit(commit(vec![
        rec("a", "0001", State::Is { body: b"A".to_vec(), objects: vec![lease.object] }),
        rec("b", "0002", State::Is { body: b"B".to_vec(), objects: vec![] }),
        rec("c", "0003", State::Is { body: b"C".to_vec(), objects: vec![] }),
    ], vec![lease], Some((words.clone(), true))), signer.clone()).unwrap();
    assert!(!first.duplicate);
    // The same operation again writes nothing; another request under its number is refused.
    let again = library.raw_commit(commit(vec![rec("a", "0001", State::Gone)], vec![], Some((words.clone(), true))), signer.clone()).unwrap();
    assert!(again.duplicate && again.count == 0);
    let mut other = words.clone();
    other.digest = [8; 32];
    assert!(library.raw_commit(commit(vec![rec("z", "9", State::Gone)], vec![], Some((other, true))), signer.clone()).is_err());
    assert_eq!(library.durability(vec!["dev-1:1".into(), "dev-1:2".into()]).unwrap(), vec![("dev-1:1".into(), true), ("dev-1:2".into(), false)]);
    assert_eq!(library.delivery("dev-1:1").unwrap(), Some(Delivery::Kept));
    library.deliver("dev-1:1", Delivery::Delivering).unwrap();
    library.deliver("dev-1:1", Delivery::Delivered).unwrap();
    assert_eq!(library.delivery("dev-1:1").unwrap(), Some(Delivery::Delivered));
    // Pages and items of the current period; a deleted item is gone from both.
    library.raw_commit(commit(vec![rec("b", "0002", State::Gone)], vec![], None), signer.clone()).unwrap();
    let page = library.page("s", "t", None, 10).unwrap();
    assert_eq!(page.iter().map(|e| e.item.clone()).collect::<Vec<_>>(), vec![b"a".to_vec(), b"c".to_vec()]);
    assert_eq!((page[0].kind, page[0].schema, page[0].objects.len()), (3, 2, 1));
    let after = library.page("s", "t", Some((b"0001".to_vec(), b"a".to_vec())), 10).unwrap();
    assert_eq!(after.len(), 1);
    assert!(library.item("s", "t", b"b").unwrap().is_none());
    assert_eq!(library.item("s", "t", b"c").unwrap().unwrap().body, b"C");
    // The gate: readable while referred to in the current period; nothing after revocation.
    assert!(library.readable("s", &lease.object.hash).unwrap());
    assert_eq!(library.read_in("s", &lease.object.hash, 0, 4).unwrap(), vec![4u8; 4]);
    assert!(!library.readable("other", &lease.object.hash).unwrap());
    library.revoke("s").unwrap();
    assert!(!library.readable("s", &lease.object.hash).unwrap());
    assert!(library.page("s", "t", None, 10).unwrap().is_empty());
    println!("v2 reads and ops drill: ok");
    let _ = std::fs::remove_dir_all(&dir);
}
