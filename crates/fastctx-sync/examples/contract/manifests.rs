use fastctx_sync::{
    blocks::{codec, manifest},
    cursor::{ByteCount, HoldingPeriod},
    identity::{Digest, ObjectId},
    manifest::{
        BlobId, ContentRef, Descriptor, DigestAlgorithm, DirectoryEntry, Encoding, FileEntry, ManifestPage,
        ManifestReply, ManifestRoot, PathName, RelativePath,
    },
    transfer::Message,
};

pub fn sample(holding: &HoldingPeriod) -> Result<Vec<Message>, Box<dyn std::error::Error>> {
    let scope = &holding.scope;
    let small = codec::encode(scope.clone(), b"{}".to_vec(), Encoding::Raw)?;
    let raw = codec::encode(scope.clone(), vec![b'x'; fastctx_types::sync::BLOCK_BYTES as usize], Encoding::Raw)?;
    let compressed =
        codec::encode(scope.clone(), vec![b'x'; fastctx_types::sync::BLOCK_BYTES as usize], Encoding::Zstd)?;
    let page = |value: ManifestPage| -> Result<ManifestReply, Box<dyn std::error::Error>> {
        let page_json = serde_json::to_string(&value)?;
        let reply = ManifestReply {
            root: ManifestRoot(BlobId {
                domain: scope.clone(),
                algorithm: DigestAlgorithm::Sha256,
                digest: Digest::of(page_json.as_bytes()),
            }),
            page_json,
        };
        manifest::validate(scope, reply.clone())?;
        Ok(reply)
    };
    let file = page(ManifestPage::Blocks {
        offset: ByteCount(0),
        blocks: vec![raw.representation.content.clone(), small.representation.content.clone()],
    })?;
    let directory = page(ManifestPage::Directory {
        entries: vec![
            DirectoryEntry::File {
                file: FileEntry {
                    object: ObjectId::from_bytes([40; 32]),
                    path: RelativePath { components: vec![PathName::Utf8("small.json".into())] },
                    bytes: ByteCount(2),
                    executable: false,
                    content: ContentRef::Single { block: small.representation.content.id.clone(), decoded_bytes: 2 },
                },
            },
            DirectoryEntry::File {
                file: FileEntry {
                    object: ObjectId::from_bytes([41; 32]),
                    path: RelativePath { components: vec![PathName::Utf8("large.bin".into())] },
                    bytes: ByteCount(u64::from(fastctx_types::sync::BLOCK_BYTES) + 2),
                    executable: false,
                    content: ContentRef::Manifest { root: file.root.clone() },
                },
            },
        ],
    })?;
    Ok(vec![
        Message::Manifest { holding: holding.clone(), reply: directory },
        Message::Manifest { holding: holding.clone(), reply: file },
        Message::Descriptor {
            holding: holding.clone(),
            descriptor: Descriptor {
                content: raw.representation.content.clone(),
                representations: vec![raw.representation, compressed.representation],
            },
        },
    ])
}
