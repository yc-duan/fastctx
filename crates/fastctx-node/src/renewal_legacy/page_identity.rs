//! Protocol 5 cannot attribute a page to a resident or to the user.
//! Only typed identity fields count; provider error JSON is opaque data.

use fastctx_types::Reply;
use fastctx_types::project::{JournalSummary, MapView, Writer};
use fastctx_types::residents::MessageKind;
use fastctx_types::run::JobOwner;
use fastctx_types::session::Speaker;
use fastctx_types::town::{Looked, MachineWho, RunningJob, WhoScope};

fn writer(by: &Writer) -> bool {
    matches!(by, Writer::Page { .. })
}

fn jobs(jobs: &[RunningJob]) -> bool {
    jobs.iter().any(|job| matches!(job.owner, JobOwner::Page { .. }))
}

fn journal(journal: &JournalSummary) -> bool {
    journal.description.as_ref().is_some_and(|description| writer(&description.by))
        || journal.latest.as_ref().is_some_and(|entry| writer(&entry.by))
}

fn map(map: &MapView) -> bool {
    writer(&map.record.by) || writer(&map.opened.by) || map.journals.iter().any(journal)
}

pub(super) fn present(reply: &Reply) -> bool {
    match reply {
        Reply::Who(who) => {
            let scope = match &who.scope {
                WhoScope::Town { record, .. } => record.as_ref().is_some_and(|record| writer(&record.by)),
                WhoScope::World { towns } => towns.iter().any(|town| writer(&town.record.by)),
            };
            scope
                || who
                    .machines
                    .iter()
                    .any(|machine| matches!(machine, MachineWho::Answered { reply , ..} if jobs(&reply.jobs)))
        }
        Reply::Looked { looked } => looked.iter().flat_map(|looked| &looked.found).any(|found| match found {
            Looked::Job { record, .. } => matches!(record.owner, JobOwner::Page { .. }),
            Looked::Resident(detail) => {
                jobs(&detail.jobs)
                    || detail
                        .answers
                        .iter()
                        .any(|line| matches!(line.what, MessageKind::Heard { from: Speaker::Page { .. }, .. }))
            }
        }),
        Reply::Towns { towns } => towns.iter().any(|town| writer(&town.record.by)),
        Reply::Record { record, opened, .. } => writer(&record.by) || writer(&opened.by),
        Reply::Map { map: view } | Reply::Entered { map: view, .. } => map(view),
        Reply::Journals { journals, .. } => journals.iter().any(journal),
        Reply::Journal { view } => {
            view.description.as_ref().is_some_and(|description| writer(&description.by))
                || view.entries.iter().any(|entry| writer(&entry.stamp.by))
        }
        _ => false,
    }
}
