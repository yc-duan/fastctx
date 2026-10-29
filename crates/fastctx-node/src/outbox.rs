//! One retry task per conversation's outgoing answers. Network failures do
//! not depend on another turn or hourly maintenance to make progress.

use crate::{
    Node,
    speech::{self, Owed},
};
use fastctx_types::residents::{MessageKind, MessageLine, Notice, Origin};
use fastctx_types::{ResidentId, layout::ConversationDir};
use jiff::Timestamp;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct Outbox {
    retries: Mutex<HashMap<ResidentId, Arc<Retry>>>,
}
#[derive(Default)]
struct Retry {
    asked: AtomicBool,
    wake: tokio::sync::Notify,
}

pub(crate) fn pay<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    directory: &'a ConversationDir,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if pay_once(node, id, directory).await {
            retry(node, id, directory);
        }
    })
}

fn retry(node: &Arc<Node>, id: &ResidentId, directory: &ConversationDir) {
    let mut retries = node.outbox.retries.lock().expect("never poisoned");
    if let Some(retry) = retries.get(id) {
        retry.asked.store(true, Ordering::SeqCst);
        retry.wake.notify_one();
        return;
    }
    let state = Arc::new(Retry::default());
    retries.insert(id.clone(), state.clone());
    let (node, id, directory) = (node.clone(), id.clone(), directory.clone());
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut ways = node.ways.subscribe();
        loop {
            let wake = state.wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            state.asked.store(false, Ordering::SeqCst);
            ways.borrow_and_update();
            if !pay_once(&node, &id, &directory).await {
                let mut retries = node.outbox.retries.lock().expect("never poisoned");
                if !state.asked.load(Ordering::SeqCst) {
                    retries.remove(&id);
                    return;
                }
            }
            tokio::select! {
                () = tokio::time::sleep(again.next()) => {}
                () = &mut wake => {}
                _ = ways.changed() => {}
            }
        }
    });
}

fn abandon(
    node: &Node,
    directory: &ConversationDir,
    path: &std::path::Path,
    owing: &fastctx_types::residents::Owing,
    reason: &str,
) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    crate::inbox::without_reader(directory, owing)?;
    crate::obligations::finish(node, path, &bytes, crate::obligations::Outcome::Impossible, reason)?;
    Ok(())
}

/// Tells each reader the answers kept for it in the resident's outbox; each
/// leaves the outbox once its reader's machine has it, or telling it failed
/// for good, which the colleague's records keep.
fn pay_once<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    directory: &'a ConversationDir,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        node.ready().await;
        let Some(paying) = node.residents.paying(id) else { return false };
        // The lock is held only to take what is owed: telling another
        // conversation takes its locks, never while this one's is held
        // (`specs/world-concurrency.md`, 锁只管落账). What is being told
        // is not taken again by a payment that comes meanwhile.
        let owing = {
            let _paying = paying.lock().await;
            let reading = directory.clone();
            let owing = tokio::task::spawn_blocking(move || crate::inbox::owing(&reading)).await;
            match owing {
                Ok(Ok(owing)) => {
                    let mut telling = TELLING.lock().expect("never poisoned");
                    owing.into_iter().filter(|answer| telling.insert(answer.delivery.id.clone())).collect::<Vec<_>>()
                }
                Ok(Err(error)) => {
                    let _ = crate::obligations::retry(node, &directory.outbox().join("reading"), &error.to_string());
                    return true;
                }
                Err(error) => {
                    let _ = crate::obligations::retry(node, &directory.outbox().join("reading"), &error.to_string());
                    return true;
                }
            }
        };
        if owing.is_empty() {
            return false;
        }
        let _told = Told(owing.iter().map(|answer| answer.delivery.id.clone()).collect());
        let Some((about, _)) = node.residents.about(id) else {
            let _ = crate::obligations::retry(
                node,
                &directory.outbox().join("identity"),
                "the answer's account identity is no longer known",
            );
            return true;
        };
        let colleague = matches!(about.origin, Origin::Colleague { .. });
        let acting = about.acting;
        let mut pending = false;
        for answer in owing {
            let turn = match &answer.delivery.notice {
                Notice::Answer { turn, .. } => Some(turn.clone()).filter(|turn| !turn.is_empty()),
                _ => None,
            };
            // Telling can lead back here, as a reader's turn ends at once.
            let owed = if answer.delivery.notice.is_question()
                || matches!(&answer.delivery.notice, Notice::Words { goal_report: Some(_), .. })
            {
                Owed::Nobody
            } else {
                Owed::Working
            };
            let account = answer
                .delivery
                .notice
                .goal_account()
                .map(|account| fastctx_types::access::Acting::one(account.hub.clone(), account.account.clone()));
            let posting_account = account.as_ref().unwrap_or(&acting);
            let path = directory.outbox().join(format!("{}.json", answer.delivery.id));
            match crate::obligations::impossible(node, posting_account, &(&answer.to.machine).into()) {
                Ok(Some(reason)) => {
                    if let Err(error) = abandon(node, directory, &path, &answer, &reason) {
                        pending = true;
                        let _ = crate::obligations::retry(node, &path, &error.to_string());
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    pending = true;
                    let _ = crate::obligations::retry(node, &path, &error.to_string());
                    continue;
                }
            }
            let landed = Box::pin(speech::tell(
                node,
                account.as_ref().unwrap_or(&acting),
                &answer.to,
                answer.delivery.id.clone(),
                answer.delivery.notice.clone(),
                owed,
            ))
            .await;
            if let Err(failure) = &landed {
                if crate::obligations::permanent(failure) || crate::lifecycle::refused::reported(failure) {
                    if let Err(error) = abandon(node, directory, &path, &answer, &failure.message) {
                        pending = true;
                        let _ = crate::obligations::retry(node, &path, &error.to_string());
                    }
                    continue;
                }
                let _ = crate::obligations::retry(node, &path, &failure.message);
                pending = true;
            }
            if colleague {
                let line = MessageLine {
                    at: Timestamp::now(),
                    what: MessageKind::Told { to: answer.to.clone(), turn, outcome: speech::outcome(&landed) },
                };
                let outcome = serde_json::to_vec(&line.what).expect("delivery outcome serializes");
                let marker = path.with_extension("attempt");
                if std::fs::read(&marker).ok().as_ref() != Some(&outcome)
                    && let Err(error) = crate::residents::append_message(directory, &line).and_then(|()| {
                        fastctx_platform::publish(&marker, &outcome, fastctx_platform::Permissions::Private)
                    })
                {
                    node.log.line(format_args!("recording that {} was told: {error}", answer.to));
                }
            }
            if landed.is_err() || matches!(landed, Ok(fastctx_types::residents::Landed::Uncertain { .. })) {
                if landed.is_err() && node.is_me(&answer.to.machine) && !node.residents.knows(&answer.to.resident) {
                    match node.world.conversation(&answer.to.resident).about().try_exists() {
                        Ok(false) => {
                            if let Err(error) = crate::inbox::without_reader(directory, &answer) {
                                node.log.line(format_args!("settling the answer without a local recipient: {error}"));
                                pending = true;
                            }
                        }
                        Ok(true) => {}
                        Err(error) => node.log.line(format_args!("checking the answer's local recipient: {error}")),
                    }
                }
                continue;
            }
            if let Err(error) = crate::inbox::paid(directory, &answer.delivery.id) {
                pending = true;
                let _ = crate::obligations::retry(node, &path, &error.to_string());
            }
        }
        pending
    })
}

/// The answers a payment is telling now.
static TELLING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<fastctx_types::NoticeId>>> =
    std::sync::LazyLock::new(Default::default);

/// Answers a payment took to tell: free to be taken again once it is done.
struct Told(Vec<fastctx_types::NoticeId>);

impl Drop for Told {
    fn drop(&mut self) {
        let mut telling = TELLING.lock().expect("never poisoned");
        for id in &self.0 {
            telling.remove(id);
        }
    }
}
