use fastctx_types::composer::{Change, Composer, Draft, Mutation, Queued, QueuedCommand, Target, Updated};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    snapshots: Vec<Composer>,
    mutations: Vec<Mutation>,
    updated: Updated,
}

pub fn fixtures() -> Fixtures {
    let id: fastctx_types::NoticeId = "queued-ab12".parse().unwrap();
    let chosen = Draft { files_on: Some("target.lab".parse().unwrap()), ..Default::default() };
    let composer = Composer {
        draft: chosen.clone(),
        queue: vec![Queued {
            id: id.clone(),
            from: "phone".parse().unwrap(),
            draft: chosen,
            editing: true,
            sending: false,
            error: None,
        }],
        commands: vec![
            QueuedCommand { id: "control-1792585279907-0-ab12".parse().unwrap(), after: None },
            QueuedCommand { id: "control-1792585279908-1-cd34".parse().unwrap(), after: Some(id.clone()) },
        ],
        ..Default::default()
    };
    Fixtures {
        snapshots: vec![Composer::default(), composer.clone()],
        mutations: vec![
            Mutation {
                operation: "destination-a".into(),
                change: Change::FilesOn {
                    target: Target::Draft,
                    epoch: 0,
                    files_on: Some("target.lab".parse().unwrap()),
                },
            },
            Mutation {
                operation: "destination-b".into(),
                change: Change::FilesOn { target: Target::Queued { id }, epoch: 0, files_on: None },
            },
        ],
        updated: Updated { composer, landed: None },
    }
}
