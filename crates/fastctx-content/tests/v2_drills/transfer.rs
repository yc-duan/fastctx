//! A drill of v2 resumable transfers.
use fastctx_content::ObjectRef;
use fastctx_content::v2::{CHUNK_BYTES, LEASE, Library};
use sha2::{Digest, Sha256};

pub(crate) fn run() {
    let dir = std::env::temp_dir().join(format!("v2-transfer-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bytes: Vec<u8> = (0..(5 * CHUNK_BYTES + 12345)).map(|i| (i * 7 % 253) as u8).collect();
    let object = ObjectRef { hash: Sha256::digest(&bytes).into(), size: bytes.len() as u64 };
    let digests: Vec<[u8; 32]> = bytes.chunks(CHUNK_BYTES as usize).map(|c| Sha256::digest(c).into()).collect();
    let chunk = |i: u64| &bytes[(i * CHUNK_BYTES) as usize..(((i + 1) * CHUNK_BYTES) as usize).min(bytes.len())];
    {
        let (library, _) = Library::open(&dir).unwrap();
        let mut transfer = library.receive(object, digests.clone(), Some("hub".into())).unwrap().unwrap();
        assert_eq!(transfer.missing(), vec![0, 1, 2, 3, 4, 5]);
        assert!(transfer.chunk(1, chunk(2)).is_err(), "a chunk not matching its digest is refused");
        for i in [0, 1, 2] {
            transfer.chunk(i, chunk(i)).unwrap();
        }
        transfer.settle().unwrap();
        transfer.chunk(4, chunk(4)).unwrap();
        // Stopped before chunk 4 was settled.
    }
    {
        let (library, started) = Library::open(&dir).unwrap();
        assert_eq!(started.transfers, 0);
        let transfer = library.receive(object, digests.clone(), None).unwrap().unwrap();
        assert_eq!(transfer.missing(), vec![3, 4, 5], "unsettled chunk 4 is asked again");
    }
    // The power took the last recorded chunk's bytes: the part file is cut short.
    let part = std::fs::read_dir(dir.join("asset-objects").join("incoming")).unwrap().next().unwrap().unwrap().path();
    let file = std::fs::OpenOptions::new().write(true).open(&part).unwrap();
    file.set_len(2 * CHUNK_BYTES + 10).unwrap();
    drop(file);
    let (library, started) = Library::open(&dir).unwrap();
    assert_eq!(started.transfers, 1, "{started:?}");
    let mut transfer = library.receive(object, digests.clone(), None).unwrap().unwrap();
    assert_eq!(transfer.missing(), vec![2, 3, 4, 5]);
    for i in transfer.missing() {
        transfer.chunk(i, chunk(i)).unwrap();
    }
    let lease = transfer.finish("drill", LEASE).unwrap();
    assert_eq!(lease.object, object);
    assert_eq!(library.size(&object.hash).unwrap(), Some(object.size));
    let mut back = Vec::new();
    let mut offset = 0;
    while offset < object.size {
        let piece = library.read(&object.hash, offset, 1024 * 1024).unwrap();
        offset += piece.len() as u64;
        back.extend(piece);
    }
    assert_eq!(back, bytes);
    assert!(library.receive(object, digests, None).unwrap().is_none(), "present: nothing to receive");
    assert_eq!(std::fs::read_dir(dir.join("asset-objects").join("incoming")).unwrap().count(), 0);
    println!("v2 transfer drill: ok");
    let _ = std::fs::remove_dir_all(&dir);
}
