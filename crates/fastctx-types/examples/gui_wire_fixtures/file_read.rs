use fastctx_types::file_read::*;
use fastctx_types::ipc::NodeFrame;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Call, Reply};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    replies: Vec<Reply>,
    remote_calls: Vec<RemoteCall>,
    remote_replies: Vec<RemoteReply>,
    frames: Vec<NodeFrame>,
    /// Payload of the binary IPC frame following the sample chunk header;
    /// the ordinary four-byte little-endian frame length is not included.
    binary_payload: Vec<u8>,
}

pub fn fixtures() -> Fixtures {
    let machine: fastctx_types::MachineName = "desktop".parse().unwrap();
    let file = FileMetadata { size: 5_368_709_120, modified: "2026-10-04T12:06:07.123456789Z".parse().unwrap() };
    let ranges = [
        FileRange::All,
        FileRange::Tail { bytes: 4096 },
        FileRange::Range { offset: 4_294_967_296, bytes: 1_048_576 },
        FileRange::Range { offset: 0, bytes: 0 },
    ];
    let mut calls = Vec::new();
    let mut remote_calls = Vec::new();
    for range in ranges {
        let request = ReadFileRequest { path: "C:/work/中文 项目/output.bin".into(), range };
        calls.push(Call::ReadFile { machine: Some(machine.clone().into()), request: request.clone() });
        calls.push(Call::ReadFile { machine: None, request: request.clone() });
        remote_calls.push(RemoteCall::ReadFile { request, read: 17 });
    }
    let mut results = vec![Ok(FileReadComplete { bytes: 4, file: file.clone() })];
    for kind in [
        FileReadErrorKind::NotFound,
        FileReadErrorKind::PermissionDenied,
        FileReadErrorKind::IsDirectory,
        FileReadErrorKind::NotRegularFile,
        FileReadErrorKind::InvalidInput,
        FileReadErrorKind::Changed,
        FileReadErrorKind::Io,
    ] {
        let changed = kind == FileReadErrorKind::Changed;
        results.push(Err(FileReadError {
            kind,
            message: "The source machine's exact error 中文".into(),
            bytes: if changed { 4 } else { 0 },
            observed: changed.then(|| FileMetadata { size: file.size + 8, modified: file.modified }),
        }));
    }
    let replies = results.iter().cloned().map(|result| Reply::FileRead { result }).collect();
    let remote_replies = results.iter().cloned().map(|result| RemoteReply::FileRead { result }).collect();
    let mut frames = vec![
        NodeFrame::FileRead {
            id: 1,
            part: FileReadPart::Opened {
                info: FileReadInfo {
                    attachment: None,
                    machine,
                    path: "/srv/中文 项目/output.bin".into(),
                    file,
                    offset: 4_294_967_296,
                    length: 4,
                    version: Some("3f9a0c1d5e7b2a4c6d8e0f1a2b3c4d5e".into()),
                },
            },
        },
        NodeFrame::FileRead { id: 1, part: FileReadPart::Chunk { offset: 4_294_967_296, bytes: 4 } },
    ];
    frames.extend(results.into_iter().map(|result| NodeFrame::Reply { id: 1, reply: Ok(Reply::FileRead { result }) }));
    Fixtures { calls, replies, remote_calls, remote_replies, frames, binary_payload: vec![0, 255, 13, 10] }
}
