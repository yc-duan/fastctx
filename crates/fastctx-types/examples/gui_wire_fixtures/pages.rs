//! Every page request and result, including its distinct action identity.

use fastctx_types::ipc::Peer;
use fastctx_types::pages::*;
use fastctx_types::project::{Stamp, Writer};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::Landed;
use fastctx_types::run::{ExitStatus, JobOwner, OutputPiece, OutputWindow, RunReply, RunState};
use fastctx_types::session::Speaker;
use fastctx_types::{Address, Call, Reply};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    replies: Vec<Reply>,
    remote_calls: Vec<RemoteCall>,
    remote_replies: Vec<RemoteReply>,
    speaker: Speaker,
    writer: Writer,
    job_owner: JobOwner,
    peer: Peer,
    map: fastctx_types::project::MapView,
}

pub fn fixtures() -> Fixtures {
    let town: fastctx_types::TownName = "mark50".parse().unwrap();
    let page: fastctx_types::PageName = "training".parse().unwrap();
    let machine: fastctx_types::MachineName = "desktop".parse().unwrap();
    let to = Address { machine: machine.clone(), resident: "codex-abcd".parse().unwrap() };
    let journal: fastctx_types::JournalName = "experiments".parse().unwrap();
    let acts = vec![
        PageAct::Run { machine: machine.clone(), script: "printf 'hello\\n'".into() },
        PageAct::Say { to: to.clone() },
        PageAct::Journal { journal: journal.clone() },
    ];
    let at = "2026-10-04T12:00:00Z".parse().unwrap();
    let publication = PageVersion {
        version: 1,
        base: None,
        title: "Training".into(),
        description: "Loss by step.".into(),
        acts: acts.clone(),
        withdrawn: false,
        restored_from: None,
        by: Writer::Resident { address: to.clone() },
        on: Some(machine.clone()),
        at,
    };
    let mut withdrawn = publication.clone();
    withdrawn.version = 2;
    withdrawn.base = Some(1);
    withdrawn.withdrawn = true;
    withdrawn.acts.clear();
    let mut restored = publication.clone();
    restored.version = 3;
    restored.base = Some(2);
    restored.restored_from = Some(1);
    let source = PageSource {
        identity: None,
        town: town.clone(),
        page: page.clone(),
        version: 1,
        by: Box::new(publication.by.clone()),
        on: machine.clone(),
        opened_on: "phone".parse().unwrap(),
    };
    let consent = PageConsent {
        identity: None,
        version: 1,
        base: None,
        page_version: 1,
        acts: acts.clone(),
        stamp: Stamp { by: Writer::User, on: Some("phone".parse().unwrap()), at },
    };
    let html = "<!doctype html><title>Training</title><p>Loss</p>".to_string();
    let draft = PageDraft {
        title: publication.title.clone(),
        description: publication.description.clone(),
        acts: acts.clone(),
        html: html.clone(),
    };
    let invocations = vec![
        PageInvocation::Run { machine: machine.clone(), script: "printf 'hello\\n'".into() },
        PageInvocation::Say { to: to.clone(), text: "Show the latest loss.".into() },
        PageInvocation::Journal { journal: journal.clone(), text: "The user opened the training page.".into() },
    ];
    let calls = vec![
        PageCall::Publish { town: Some(town.clone()), page: page.clone(), base: None, draft: draft.clone() },
        PageCall::Publish { town: Some(town.clone()), page: page.clone(), base: Some(1), draft },
        PageCall::Read { town: Some(town.clone()), page: page.clone(), version: None, account: None },
        PageCall::Read { town: Some(town.clone()), page: page.clone(), version: Some(1), account: None },
        PageCall::List { town: Some(town.clone()) },
        PageCall::History { town: Some(town.clone()), page: page.clone() },
        PageCall::Withdraw { town: Some(town.clone()), page: page.clone(), base: 1 },
        PageCall::Restore { town: Some(town.clone()), page: page.clone(), base: 2, version: 1 },
        PageCall::Consent {
            town: Some(town.clone()),
            page: page.clone(),
            version: 1,
            base: None,
            acts: acts.clone(),
            account: None,
        },
        PageCall::Consent {
            town: Some(town.clone()),
            page: page.clone(),
            version: 3,
            base: Some(1),
            acts: Vec::new(),
            account: None,
        },
    ];
    let calls = calls
        .into_iter()
        .chain(invocations.iter().cloned().map(|action| PageCall::Act {
            account: None,
            town: Some(town.clone()),
            page: page.clone(),
            version: 1,
            action,
        }))
        .map(Call::Page)
        .collect();
    let results = vec![
        PageResult::Run(Box::new(RunReply {
            scratch: Vec::new(),
            run: None,
            consumed: None,
            privacy: Vec::new(),
            missing: Vec::new(),
            machine: machine.clone(),
            job: "j-abc123".parse().unwrap(),
            state: RunState::Ended { status: ExitStatus::Code(0) },
            output: OutputWindow {
                log: "C:/world/towns/mark50/pages/training/out/j-abc123.log".into(),
                bytes: 6,
                lines: 1,
                pieces: vec![OutputPiece::Text { first_line: 1, text: "hello\n".into() }],
                legacy: Vec::new(),
                escaped_lines: 0,
                failed: Vec::new(),
            },
            visited: Vec::new(),
            answered: None,
        })),
        PageResult::Said { to, landed: Landed::Sent },
        PageResult::EntryAdded { journal, id: "20261004T120000Z-desktop".parse().unwrap() },
    ];
    let map = fastctx_types::project::MapView {
        memory: None,
        town: town.clone(),
        ledger: "C:/world/towns/mark50/ledger".into(),
        record: fastctx_types::project::Record {
            version: 1,
            name: "Mark 50".into(),
            what: "Training experiments".into(),
            machines: vec![machine.clone()],
            places: Vec::new(),
            canon: None,
            by: Writer::User,
            on: Some(machine.clone()),
            at,
        },
        opened: Stamp { by: Writer::User, on: Some(machine.clone()), at },
        journals: Vec::new(),
        pages: vec![PageSummary { page: page.clone(), current: restored.clone() }],
        canon: None,
    };
    let mut replies = vec![
        Reply::Page {
            view: Box::new(PageView {
                town: town.clone(),
                page: page.clone(),
                publication: publication.clone(),
                html: Some(html),
                consent: Some(consent.clone()),
            }),
        },
        Reply::Page {
            view: Box::new(PageView {
                town: town.clone(),
                page: page.clone(),
                publication: withdrawn.clone(),
                html: None,
                consent: Some(consent.clone()),
            }),
        },
        Reply::Pages { town: town.clone(), pages: vec![PageSummary { page: page.clone(), current: restored.clone() }] },
        Reply::PageHistory { town: town.clone(), page: page.clone(), versions: vec![publication, withdrawn, restored] },
        Reply::PageConsent { town, page, consent },
    ];
    replies.extend(results.iter().cloned().map(|result| Reply::PageAct { result }));
    Fixtures {
        map,
        calls,
        replies,
        remote_calls: invocations
            .into_iter()
            .take(2)
            .map(|action| RemoteCall::PageAction { source: source.clone(), action })
            .collect(),
        remote_replies: results.into_iter().take(2).map(|result| RemoteReply::PageAction { result }).collect(),
        speaker: Speaker::Page { page: Box::new(source.clone()) },
        writer: Writer::Page { page: Box::new(source.clone()) },
        job_owner: JobOwner::Page { page: source.clone() },
        peer: Peer::Page { page: Box::new(source), job: None },
    }
}
