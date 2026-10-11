use fastctx_types::composer::{Change, Composer, Draft, Mutation, Queued, Target, Updated};
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
