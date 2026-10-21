use fastctx_types::management::*;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Activity, Landed};
use fastctx_types::{Call, Reply};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    remote_calls: Vec<RemoteCall>,
    replies: Vec<Reply>,
    remote_replies: Vec<RemoteReply>,
}

pub fn fixtures() -> Fixtures {
    let address: fastctx_types::Address = "codex-ab12@desktop".parse().unwrap();
    let actions = [
        Management::Archive,
        Management::Restore,
        Management::Sleep,
        Management::Wake,
        Management::Move { town: Some("project-0".parse().unwrap()) },
        Management::Move { town: None },
        Management::Focus { focus: Default::default() },
        Management::Focus {
            focus: fastctx_types::focus::Focus {
                machines: vec![fastctx_types::focus::FocusedMachine {
                    machine: "laptop.home".parse().unwrap(),
                    folders: vec!["C:/work/中文 项目".into()],
                }],
            },
        },
    ];
    let mut calls: Vec<Call> = actions
        .iter()
        .map(|action| Call::Manage {
            resident: address.resident.clone(),
            machine: Some(address.machine.clone().into()),
            action: action.clone(),
        })
        .collect();
    calls.push(Call::Forget { resident: address.resident.clone(), machine: address.machine.clone().into() });
    let remote_calls =
        actions.into_iter().map(|action| RemoteCall::Manage { resident: address.resident.clone(), action }).collect();
    let state = |outcome, activity, detail: &str| Managed {
        removed: None,
        town: None,
        activity,
        steps: vec![ManagementStep { stage: "host".into(), outcome, detail: detail.into() }],
        notification: None,
    };
    let mut removed = state(ManagementOutcome::Failed, Some(Activity::Idle), "The thread has another active writer.");
    removed.removed = Some("2026-10-04T12:00:00Z".parse().unwrap());
    removed.steps.push(ManagementStep {
        stage: "records".into(),
        outcome: ManagementOutcome::Done,
        detail: "Records archived; World jobs continue.".into(),
    });
    let moved = Managed {
        removed: None,
        town: Some("project-0".parse().unwrap()),
        activity: Some(Activity::Busy),
        steps: vec![ManagementStep {
            stage: "records".into(),
            outcome: ManagementOutcome::Done,
            detail: "Project membership changed.".into(),
        }],
        notification: Some(Landed::Handed { id: "project-0123".parse().unwrap() }),
    };
    let results = vec![
        state(ManagementOutcome::Done, Some(Activity::Asleep), "The runtime unloaded."),
        state(ManagementOutcome::Done, Some(Activity::Idle), "Resumed without a message."),
        state(ManagementOutcome::Pending, Some(Activity::Idle), "Unsubscribed; waiting for thread/closed."),
        state(ManagementOutcome::Unsupported, None, "This editor window has no supported unload entrance."),
        removed,
        moved,
    ];
    Fixtures {
        calls,
        remote_calls,
        replies: results
            .iter()
            .map(|managed| Reply::Managed { resident: address.clone(), managed: managed.clone() })
            .chain(
                [Forgetting::Deleted, Forgetting::Asked, Forgetting::Queued]
                    .map(|how| Reply::Forgetting { resident: address.clone(), how }),
            )
            .collect(),
        remote_replies: results.into_iter().map(RemoteReply::Managed).collect(),
    }
}
