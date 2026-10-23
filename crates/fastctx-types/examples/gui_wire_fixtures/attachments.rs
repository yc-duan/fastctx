use fastctx_types::attachments::{
    Attachment, AttachmentFile, AttachmentFileSource, AttachmentSource, HostImage, UploadFile, UploadTarget,
};
use fastctx_types::file_read::FileRange;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::{Happened, SessionEvent, Speaker, attachments::AttachedMessage};
use fastctx_types::{Address, Call, Reply};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    replies: Vec<Answer>,
    remote_calls: Vec<RemoteCall>,
    remote_replies: Vec<RemoteReply>,
    events: Vec<SessionEvent>,
    host_text: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Answer {
    Reply(Reply),
    Frame(fastctx_types::ipc::NodeFrame),
}

pub fn fixtures() -> Fixtures {
    let owner = Address { resident: "codex-ab12".parse().unwrap(), machine: "laptop".parse().unwrap() };
    let town = Some("world".parse().unwrap());
    let file =
        UploadFile { name: "截图.png".into(), media_type: "image/png".into(), size: 3145728, sha256: "a".repeat(64) };
    let attachment = Attachment {
        id: "attachment-ab12".parse().unwrap(),
        owner: owner.clone(),
        conversation: Some(Box::new(Address { machine: "desktop".parse().unwrap(), resident: owner.resident.clone() })),
        town: town.clone(),
        path: "C:/world/towns/world/residents/codex-ab12/attachments/attachment-ab12/content.png".into(),
        file: file.clone(),
    };
    let attachments = vec![attachment.clone()];
    let text = "看这张图。\nKeep my words verbatim.".to_string();
    let envelope = AttachedMessage {
        delivery: "talk-ab12".parse().unwrap(),
        from: "phone".parse().unwrap(),
        attachments: attachments.clone(),
    };
    let target = UploadTarget::Resident { resident: owner.resident.clone() };
    Fixtures {
        calls: vec![
            Call::ReadAttachment { attachment: attachment.clone().into(), range: FileRange::All },
            Call::ReadAttachment {
                attachment: attachment.clone().into(),
                range: FileRange::Range { offset: 1_048_576, bytes: 1024 },
            },
            Call::AttachmentThumbnail { attachment: attachment.clone().into(), max: 96 },
            Call::ReadAttachment {
                range: FileRange::All,
                attachment: AttachmentSource::HostImage {
                    owner: owner.clone(),
                    host_image: HostImage::Codex { turn: "turn-1".into(), message: 0, image: 1 },
                },
            },
            Call::AttachmentThumbnail {
                attachment: AttachmentSource::HostImage {
                    owner: Address { resident: "claude-ab12".parse().unwrap(), ..owner.clone() },
                    host_image: HostImage::Claude { message: "message-uuid".into(), image: 0 },
                },
                max: 96,
            },
            Call::Upload {
                target: UploadTarget::Sent {
                    account: None,
                    files_on: None,
                    machine: "desktop".parse().unwrap(),
                    target: Box::new(target.clone()),
                    operation: "upload-1".into(),
                },
                node: Some("desktop".parse().unwrap()),
                file: file.clone(),
                composer: None,
            },
            Call::Upload {
                target: target.clone(),
                node: Some(owner.machine.clone().into()),
                file: file.clone(),
                composer: None,
            },
            Call::Upload {
                target: UploadTarget::New { agent: "codex".parse().unwrap(), town: town.clone() },
                node: Some(owner.machine.clone().into()),
                file: file.clone(),
                composer: None,
            },
            Call::Upload {
                target: UploadTarget::Draft { resident: owner.resident.clone(), town: town.clone() },
                node: Some(owner.machine.clone().into()),
                file: file.clone(),
                composer: None,
            },
            Call::Talk {
                to: owner.resident.clone(),
                machine: Some(owner.machine.clone().into()),
                text: text.clone(),
                attachments: attachments.clone(),
                files_on: Some("laptop".parse().unwrap()),
                id: Some("words-1".into()),
            },
            Call::Open {
                agent: "codex".parse().unwrap(),
                town: town.clone(),
                node: Some(owner.machine.clone().into()),
                text: text.clone(),
                model: None,
                reasoning_effort: None,
                attachments: attachments.clone(),
                choices: Default::default(),
            },
        ],
        replies: vec![
            Answer::Reply(Reply::Uploaded(attachment.clone())),
            Answer::Reply(Reply::Notices {
                notices: vec![Delivery {
                    id: envelope.delivery.clone(),
                    at: "2026-10-04T12:00:00Z".parse().unwrap(),
                    notice: Notice::Attached { message: envelope.clone(), text: text.clone() },
                }],
                notes: Vec::new(),
                leading_notes: 0,
            }),
            Answer::Frame(fastctx_types::ipc::NodeFrame::Reply {
                id: 17,
                reply: Err(fastctx_types::ipc::Failure::original_gone()),
            }),
            Answer::Frame(fastctx_types::ipc::NodeFrame::FileRead {
                id: 17,
                part: fastctx_types::file_read::FileReadPart::Opened {
                    info: fastctx_types::file_read::FileReadInfo {
                        machine: owner.machine.clone(),
                        path: "/private/cache/image.jpg".into(),
                        offset: 0,
                        length: 2048,
                        file: fastctx_types::file_read::FileMetadata {
                            size: 2048,
                            modified: "2026-10-04T12:00:00Z".parse().unwrap(),
                        },
                        attachment: Some(AttachmentFile {
                            name: "截图.jpg".into(),
                            media_type: "image/jpeg".into(),
                            source: AttachmentFileSource::HostRecord,
                        }),
                        version: None,
                    },
                },
            }),
        ],
        remote_calls: vec![
            RemoteCall::ReadAttachment { attachment: attachment.clone().into(), read: 17, range: FileRange::All },
            RemoteCall::ReadAttachment {
                attachment: attachment.clone().into(),
                read: 18,
                range: FileRange::Range { offset: 2048, bytes: 4096 },
            },
            RemoteCall::AttachmentThumbnail {
                attachment: AttachmentSource::HostImage {
                    owner: owner.clone(),
                    host_image: HostImage::Codex { turn: "turn-1".into(), message: 0, image: 1 },
                },
                max: 96,
            },
            RemoteCall::ReserveAttachment {
                target: target.clone(),
                file: file.clone(),
                operation: "upload-1".into(),
                composer: None,
            },
            RemoteCall::CommitAttachment { attachment: attachment.clone(), preview: None },
            RemoteCall::Upload { target, file, composer: None, resumes: false },
            RemoteCall::Talk {
                to: owner.resident.clone(),
                text: text.clone(),
                attachments: attachments.clone(),
                files_on: None,
                id: Some("words-1".into()),
            },
            RemoteCall::Open {
                mcp_off: Default::default(),
                agent: "codex".parse().unwrap(),
                town,
                text: text.clone(),
                model: None,
                reasoning_effort: None,
                attachments,
                focus: Default::default(),
                home: None,
                files_on: None,
            },
        ],
        remote_replies: vec![RemoteReply::Uploaded(attachment)],
        events: vec![SessionEvent {
            at: "2026-10-04T12:00:00Z".parse().unwrap(),
            happened: Happened::Heard {
                delivery: Some(envelope.delivery.clone()),
                turn: Some("turn-1".into()),
                from: Some(Speaker::User { machine: envelope.from.clone() }),
                text: text.clone(),
                message: Some(Box::new(envelope.message(&text))),
                notice: None,
            },
        }],
        host_text: envelope.render(&text),
    }
}
