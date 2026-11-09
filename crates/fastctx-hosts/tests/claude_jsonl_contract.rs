//! One frozen framing contract, including complete-row publication and 64 KiB
//! boundaries. Private index bytes and checkpoint serialization are not frozen.
use fastctx_hosts::claude::ingest::{
    body::{BodyCursor, BodyFuture, BodyKey, BodyPart, BodySink},
    document::{
        self, Address, Completion, Value,
        read::{BodyRead, Reader},
    },
    image::ImageSource,
    jsonl::{Jsonl, Row, SealReason, StableSeal},
    lexical::WINDOW_BYTES,
};
use fastctx_types::{
    file_identity::{FileChangeStamp, FileIdentity},
    sync::{
        cursor::{ByteCount, Count},
        identity::{Digest, HostGeneration, HubId, ScopeId, ScopeKey, SourceId},
        manifest::{BlobId, ContentRef, DigestAlgorithm},
    },
};
use serde::Deserialize;
use std::{collections::BTreeMap, io};

#[derive(Default)]
struct Memory(BTreeMap<Digest, Vec<u8>>);
impl BodySink for Memory {
    fn append<'a>(
        &'a mut self,
        _: &'a BodyKey,
        before: Option<&'a BodyCursor>,
        bytes: Vec<u8>,
    ) -> BodyFuture<'a, BodyCursor> {
        Box::pin(async move {
            let mut value = before.map(|b| self.0[&b.content.id().digest].clone()).unwrap_or_default();
            value.extend_from_slice(&bytes);
            let digest = Digest::of(&value);
            let block = BlobId {
                domain: ScopeKey { hub: HubId::from_bytes([0; 8]), scope: ScopeId::from_bytes([0; 32]) },
                algorithm: DigestAlgorithm::Sha256,
                digest: digest.clone(),
            };
            let cursor = BodyCursor {
                content: ContentRef::Single { block, decoded_bytes: value.len() as u32 },
                bytes: ByteCount(value.len() as u64),
            };
            self.0.insert(digest, value);
            Ok(cursor)
        })
    }
}
impl BodyRead for Memory {
    fn read<'a>(&'a self, body: &'a BodyCursor, offset: u64, bytes: usize) -> BodyFuture<'a, Vec<u8>> {
        Box::pin(
            async move { Ok(self.0[&body.content.id().digest][offset as usize..offset as usize + bytes].to_vec()) },
        )
    }
}
#[derive(Deserialize)]
struct Contract {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    expected: serde_json::Value,
    valid_while_following: usize,
    valid_after_stable_seal: usize,
    invalid_rows: usize,
    raw_sha256: String,
    align_tokens: Vec<String>,
    image_sha256: Option<String>,
}
#[tokio::test]
async fn claude_jsonl_v1() -> io::Result<()> {
    let contract: Contract = serde_json::from_str(include_str!("../contracts/claude-jsonl-v1.json"))?;
    for case in contract.cases {
        assert_eq!(
            Digest::of(case.input.strip_suffix('\n').unwrap_or(&case.input).as_bytes()).as_str(),
            case.raw_sha256,
            "{} fixture digest",
            case.name
        );
        let padding = case
            .align_tokens
            .iter()
            .map(|token| WINDOW_BYTES - 1 - case.input.find(token.as_str()).expect("alignment token is in fixture"));
        for pad in std::iter::once(0).chain(padding) {
            let input = format!("{}{input}", " ".repeat(pad), input = case.input);
            for window in [1, 7, WINDOW_BYTES] {
                replay(&case, input.as_bytes(), window).await?;
            }
        }
    }
    Ok(())
}
async fn replay(case: &Case, input: &[u8], window: usize) -> io::Result<()> {
    let mut sink = Memory::default();
    let key = BodyKey {
        source: SourceId::from_bytes([1; 32]),
        generation: HostGeneration::from_bytes([2; 32]),
        line_start: ByteCount(0),
        part: BodyPart::Raw,
    };
    let mut parser = Jsonl::new(key, ByteCount(0));
    let mut rows = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let end = (at + window).min(input.len());
        let step = parser.push(&input[at..end], &mut sink).await?;
        assert!(step.used > 0);
        at += step.used;
        if let Some(row) = step.row {
            rows.push(row);
        }
        if at == 1 || at == 7 || at % WINDOW_BYTES == 0 || at == input.len() {
            let bytes = serde_json::to_vec(&parser.checkpoint(&mut sink).await?)?;
            parser = Jsonl::resume(serde_json::from_slice(&bytes)?)?;
        }
    }
    assert_eq!(valid(&rows), case.valid_while_following, "{} following", case.name);
    let identity = FileIdentity::Unix { device: Count(1), inode: Count(2) };
    let stamp = FileChangeStamp {
        size: ByteCount(input.len() as u64),
        modified: "2026-01-01T00:00:00Z".parse().unwrap(),
        indicators: ["0".into(), "0".into()],
    };
    let seal = StableSeal {
        reason: SealReason::HostCompleted,
        identity: identity.clone(),
        before: stamp.clone(),
        after: stamp,
    };
    if let Some(row) = parser.seal(&identity, &seal, &mut sink).await? {
        rows.push(row);
    }
    assert_eq!(valid(&rows), case.valid_after_stable_seal, "{} seal", case.name);
    assert_eq!(rows.len() - valid(&rows), case.invalid_rows);
    for row in rows {
        let bodies = match &row.content {
            Completion::Valid(doc) => &doc.bodies,
            Completion::Invalid { bodies, .. } => bodies,
        };
        assert!(bodies.nodes.bytes.0 <= document::index_limit(bodies.raw.bytes.0));
        let raw = &sink.0[&bodies.raw.content.id().digest];
        assert_eq!(raw, &input[row.start.0 as usize..(row.end.0 as usize - usize::from(!row.sealed))]);
        if let Completion::Valid(doc) = &row.content {
            let reader = Reader { document: doc, input: &sink };
            assert_eq!(walk(&reader, doc.root).await?, case.expected);
            if let Some(expected) = &case.image_sha256 {
                let message = reader.field(doc.root, "message").await?.unwrap().0;
                let content = reader.field(message, "content").await?.unwrap().0;
                let Value::Array { last: Some(block), .. } = reader.node(content).await?.value else {
                    panic!("image fixture shape");
                };
                let image = ImageSource::from_block(&reader, block).await?.expect("image classification");
                let mut cursor = image.start();
                let mut bytes = Vec::new();
                while !image.complete(&cursor) {
                    bytes.extend(image.read_window(&reader, &mut cursor, window).await?);
                    cursor = serde_json::from_slice(&serde_json::to_vec(&cursor)?)?;
                }
                assert_eq!(Digest::of(&bytes).as_str(), expected);
            }
        }
    }
    Ok(())
}
fn valid(rows: &[Row]) -> usize {
    rows.iter().filter(|r| matches!(r.content, Completion::Valid(_))).count()
}
fn walk<'a>(reader: &'a Reader<'_>, address: Address) -> BodyFuture<'a, serde_json::Value> {
    Box::pin(async move {
        Ok(match reader.node(address).await?.value {
            Value::Object { last, .. } | Value::Array { last, .. } => {
                let object = matches!(reader.node(address).await?.value, Value::Object { .. });
                let mut children = Vec::new();
                let mut next = last;
                while let Some(a) = next {
                    let node = reader.node(a).await?;
                    next = node.previous;
                    children.push((node, a));
                }
                children.reverse();
                let mut map = serde_json::Map::new();
                let mut array = Vec::new();
                for (node, a) in children {
                    let value = walk(reader, a).await?;
                    if object {
                        map.insert(reader.text(node.name.unwrap(), WINDOW_BYTES).await?.unwrap(), value);
                    } else {
                        array.push(value);
                    }
                }
                if object { serde_json::Value::Object(map) } else { serde_json::Value::Array(array) }
            }
            Value::String { text } => reader.text(text, WINDOW_BYTES).await?.unwrap().into(),
            Value::Number { text } => serde_json::from_str(&reader.text(text, WINDOW_BYTES).await?.unwrap())?,
            Value::Bool { value } => value.into(),
            Value::Null => serde_json::Value::Null,
            Value::Opaque { .. } => return Err(io::Error::other("contract expected selected fields")),
        })
    })
}
