//! Startup cleanup and delayed recovery of work that remains on disk.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::layout::WorldDir;

use crate::{Node, log::Log};

const RETRY: Duration = Duration::from_secs(300);

/// These collectors need startup's exclusive access. Failed objects remain
/// on disk for the next start; repeating them while serving could remove a
/// new upload or a job whose record has not been published yet.
pub(crate) fn prepare(world: &WorldDir, log: &Log) {
    if let Err(error) = crate::unpublished::collect(world.path()) {
        log.line(format_args!("unpublished records remain for the next start: {error}"));
    }
    if let Err(error) = fastctx_platform::owned_directory::delete(&world.image_reading()) {
        log.line(format_args!("old image reads remain for the next start: {error}"));
    }
    match crate::gather::gather(world) {
        Ok(gathered) => {
            for name in gathered.moved {
                log.line(format_args!("gathered {name} into its directory"));
            }
            for (path, error) in gathered.problems {
                log.line(format_args!("{} remains to be gathered at the next start: {error}", path.display()));
            }
        }
        Err(error) => log.line(format_args!("conversation gathering remains for the next start: {error}")),
    }
    if let Err(error) = crate::unstarted::collect(world) {
        log.line(format_args!("unfinished job preparations remain for the next start: {error}"));
    }
}

/// Each worker finishes a pass before waiting. A successful deletion scan
/// does not retire its worker: a later request can leave a new intent.
pub(crate) fn start(node: &Arc<Node>, jobs_reconciled: bool) {
    let sentinels = node.clone();
    tokio::spawn(async move {
        let mut previous = Vec::new();
        loop {
            let errors = crate::sentinels::deletion::recover(&sentinels).await;
            for error in &errors {
                if !previous.contains(error) { sentinels.log.line(error); }
            }
            previous = errors;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    let deleting = node.clone();
    tokio::spawn(async move {
        loop {
            crate::data::delete::finish_interrupted(&deleting).await;
            crate::attachments::lifetime::collect(&deleting).await;
            for link in deleting.links() {
                crate::ledger::letgo::let_go(&deleting, &link).await;
            }
            tokio::time::sleep(RETRY).await;
        }
    });

    // Who runs how many: the jobs left running when the node started.
    let counting = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let scanning = counting.clone();
            match tokio::task::spawn_blocking(move || crate::job_accounts::recover(&scanning)).await {
                Ok(Ok(())) => break,
                Ok(Err(error)) => {
                    counting.log.line(format_args!("counting the jobs left running; will retry: {error}"))
                }
                Err(error) => counting.log.line(format_args!("counting the jobs left running; will retry: {error}")),
            }
            tokio::time::sleep(again.next().max(Duration::from_secs(1))).await;
        }
    });

    let jobs = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut complete = jobs_reconciled;
        while !complete {
            tokio::time::sleep(again.next().max(Duration::from_secs(1))).await;
            let checking = jobs.clone();
            let reconciled = tokio::task::spawn_blocking(move || reconcile(&checking.world, &checking.log)).await;
            match reconciled {
                Ok(done) => complete = done,
                Err(error) => jobs.log.line(format_args!("recovering interrupted jobs: {error}")),
            }
        }
    });

    let reports = node.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RETRY).await;
            crate::codex::replay(&reports).await;
        }
    });

    let answers = node.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RETRY).await;
            for resident in answers.residents.everyone() {
                let directory = answers.world.conversation(&resident.id);
                crate::turns::pay(&answers, &resident.id, &directory).await;
            }
        }
    });
}

pub(crate) fn reconcile(world: &WorldDir, log: &Log) -> bool {
    match crate::jobs::reconcile(world) {
        Ok(reconciled) => {
            for path in reconciled.interrupted {
                log.line(format_args!("marked interrupted: {}", path.display()));
            }
            let complete = reconciled.problems.is_empty();
            for (path, error) in reconciled.problems {
                log.line(format_args!("could not check {}; will retry: {error}", path.display()));
            }
            complete
        }
        Err(error) => {
            log.line(format_args!("could not reconcile jobs; will retry: {error}"));
            false
        }
    }
}
