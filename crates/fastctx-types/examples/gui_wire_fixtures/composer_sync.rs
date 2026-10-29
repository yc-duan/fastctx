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
    };
    Fixtures {
        subscription: Subscription {
            version: VERSION,
            known: vec![Vector { document: "document-a".into(), vector: Bytes(vec![0]) }],
        },
        inputs: vec![Input::Update { number: 1, document: "document-a".into(), update: Bytes(vec![0, 0]) }],
        frames: vec![
            Frame::State { version: VERSION, state: Box::new(state), documents: vec![update.clone()] },
            Frame::Update { revision: 8, document: update },
            Frame::Saved { number: 1, document: "document-a".into(), revision: 8, document_revision: 3 },
            Frame::Refused {
                number: 2,
                failure: Failure::not_run("the update was not started")
                    .capacity(CapacityReason::Waiting { seconds: 2 }),
            },
            Frame::Refused { number: 3, failure: Failure::refused("the conversation is no longer shared") },
        ],
        sends: vec![Mutation {
            operation: "send-a".into(),
            change: Change::SendDocument {
                document: "document-a".into(),
                snapshot: Bytes(vec![0, 0]),
                attachments: Vec::new(),
            },
        }],
    }
}
