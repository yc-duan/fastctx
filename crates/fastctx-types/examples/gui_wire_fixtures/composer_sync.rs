use fastctx_types::{
    composer::{Change, Mutation, sync::*},
    ipc::{CapacityReason, Failure},
};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    subscription: Subscription,
    inputs: Vec<Input>,
    frames: Vec<Frame>,
    sends: Vec<Mutation>,
}

pub fn fixtures() -> Fixtures {
    let document = Document {
        id: "document-a".into(),
        revision: 3,
        epoch: 0,
        attachments: Vec::new(),
        uploads: Vec::new(),
        files_on: None,
    };
    let update = Update { document: document.id.clone(), revision: 3, update: Bytes(vec![0, 0]) };
    let state = State {
        revision: 7,
        draft: document,
        queue: Vec::new(),
        commands: Vec::new(),
        unobserved: Vec::new(),
        held: None,
        awaiting_turn: None,
        operations: vec![],
    };
    Fixtures {
        subscription: Subscription {
            version: VERSION,
            known: vec![Vector { document: "document-a".into(), vector: Bytes(vec![0]) }],
        },
        inputs: vec![
            Input::Update { number: 1, document: "document-a".into(), update: Bytes(vec![0, 0]) },
            Input::Mutate {
                number: 4,
                mutation: Box::new(Mutation {
                    operation: "files-a".into(),
                    change: Change::FilesOn {
                        target: fastctx_types::composer::Target::Draft,
                        epoch: 0,
                        files_on: None,
                    },
                }),
            },
            Input::Mutate {
                number: 7,
                mutation: Box::new(Mutation {
                    operation: "withdraw-a".into(),
                    change: Change::Withdraw { send: "send-native".into() },
                }),
            },
        ],
        frames: vec![
            Frame::State {
                version: VERSION,
                state: Box::new(state.clone()),
                documents: vec![update.clone()],
                vectors: vec![],
                mutations: false,
            },
            Frame::State {
                version: VERSION,
                state: Box::new(state),
                documents: vec![],
                vectors: vec![Vector { document: "document-a".into(), vector: Bytes(vec![0]) }],
                mutations: true,
            },
            Frame::Result {
                number: 4,
                revision: 9,
                operation: Operation {
                    operation: "send-a".into(),
                    delivery: Some("queued-cd34".parse().unwrap()),
                    accepted: true,
                    landed: None,
                    failure: None,
                },
            },
            // A send the queue holds: it neither went nor failed.
            Frame::Result {
                number: 5,
                revision: 9,
                operation: Operation {
                    operation: "send-queued".into(),
                    delivery: None,
                    accepted: false,
                    landed: None,
                    failure: None,
                },
            },
            Frame::Result {
                number: 6,
                revision: 10,
                operation: Operation {
                    operation: "send-native".into(),
                    delivery: Some("queued-cd34".parse().unwrap()),
                    accepted: false,
                    landed: Some(fastctx_types::residents::Landed::NewTurn { turn: "turn-a".into() }),
                    failure: None,
                },
            },
            // A taking back of words that went.
            Frame::Result {
                number: 7,
                revision: 10,
                operation: Operation {
                    operation: "withdraw-a".into(),
                    delivery: None,
                    accepted: false,
                    landed: None,
                    failure: Some(fastctx_types::composer::ALREADY_SENT.into()),
                },
            },
            Frame::Update { revision: 8, document: update },
            Frame::Saved { number: 1, document: "document-a".into(), revision: 8, document_revision: 3 },
            Frame::Refused {
                number: 2,
                failure: Failure::not_run("the update was not started")
                    .capacity(CapacityReason::Waiting { seconds: 2 }),
            },
            Frame::Refused { number: 3, failure: Failure::refused("the conversation is no longer shared") },
        ],
        sends: vec![
            Mutation {
                operation: "send-a".into(),
                change: Change::SendDocument {
                    epoch: None,
                    document: "document-a".into(),
                    snapshot: Bytes(vec![0, 0]),
                    take: None,
                    attachments: Vec::new(),
                },
            },
            Mutation {
                operation: "send-native".into(),
                change: Change::SendDocument {
                    epoch: Some(0),
                    document: "document-a".into(),
                    snapshot: Bytes(vec![0, 0]),
                    take: Some(Bytes(vec![0, 0])),
                    attachments: Vec::new(),
                },
            },
        ],
    }
}
