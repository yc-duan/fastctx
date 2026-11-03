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
    fastctx_platform::task::spawn(async move {
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

/// Tells each reader the answers kept for it in the resident's outbox; each
/// leaves the outbox once its reader's machine has it, or telling it failed
/// for good, which the colleague's records keep.
fn pay_once<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    directory: &'a ConversationDir,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        node.ready_for(id).await;
        let Some(paying) = node.residents.paying(id) else { return false };
        let Some((about, _)) = node.residents.about(id) else { return true };
        let colleague = matches!(about.origin, Origin::Colleague { .. });
        let acting = about.acting;
        let mut pending = false;
        let mut after = crate::inbox::answers::Cursor::default();
        loop {
            // Claim one answer under the local payment gate. Delivery can call
            // back into this conversation, so no gate crosses the network.
            let answer = {
                let _paying = paying.lock().await;
                let (reading, cursor) = (directory.clone(), after.clone());
                let next =
                    fastctx_platform::task::spawn_blocking(move || crate::inbox::answers::next(&reading, &cursor))
                        .await
                        .map_err(std::io::Error::other)
                        .and_then(|result| result);
                match next {
                    Ok(Some((cursor, answer))) => {
                        after = cursor;
                        if !TELLING.lock().expect("never poisoned").insert(answer.delivery.id.clone()) {
                            continue;
                        }
                        answer
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = crate::inbox::outgoing::retry(
                            node,
                            directory,
                            &directory.outbox().join("reading"),
                            &error.to_string(),
                        )
                        .await;
                        return true;
                    }
                }
            };
            let _told = Told(vec![answer.delivery.id.clone()]);
            let turn = match &answer.delivery.notice {
                Notice::Answer { turn, .. } => Some(turn.clone()).filter(|turn| !turn.is_empty()),
                _ => None,
            };
            // Telling can lead back here, as a reader's turn ends at once.
            let owed = if answer.delivery.notice.owes_no_answer() { Owed::Nobody } else { Owed::Working };
            let account = answer
                .delivery
                .notice
                .goal_account()
                .map(|account| fastctx_types::access::Acting::one(account.hub.clone(), account.account.clone()));
            let posting_account = account.as_ref().unwrap_or(&acting);
            let path = directory.outbox().join(format!("{}.json", answer.delivery.id));
            let impossible = {
                let (asking, posting, to) =
                    (node.clone(), posting_account.clone(), fastctx_types::access::Written::from(&answer.to.machine));
                fastctx_platform::task::blocking(move || crate::obligations::impossible(&asking, &posting, &to)).await
            };
            match impossible {
                Ok(Some(reason)) => {
                    if let Err(error) = crate::inbox::outgoing::abandon(node, directory, &answer, &reason).await {
                        pending = true;
                        let _ = crate::inbox::outgoing::retry(node, directory, &path, &error.to_string()).await;
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    pending = true;
                    let _ = crate::inbox::outgoing::retry(node, directory, &path, &error.to_string()).await;
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
                    if let Err(error) =
                        crate::inbox::outgoing::abandon(node, directory, &answer, &failure.message).await
                    {
                        pending = true;
                        let _ = crate::inbox::outgoing::retry(node, directory, &path, &error.to_string()).await;
                    }
                    continue;
                }
                let _ = crate::inbox::outgoing::retry(node, directory, &path, &failure.message).await;
                pending = true;
            }
            if colleague {
                let line = MessageLine {
                    at: Timestamp::now(),
                    what: MessageKind::Told { to: answer.to.clone(), turn, outcome: speech::outcome(&landed) },
                };
                if let Err(error) = crate::inbox::outgoing::attempt(directory, &answer.delivery.id, line).await {
                    node.log.line(format_args!("recording that {} was told: {error}", answer.to));
                }
            }
            // Words whose arrival is not known stay owed, and are told again
            // under the same id, which their reader takes once.
            if let Ok(fastctx_types::residents::Landed::Uncertain { why }) = &landed {
                pending = true;
                let _ = crate::inbox::outgoing::retry(node, directory, &path, why).await;
            }
            if landed.is_err() || matches!(landed, Ok(fastctx_types::residents::Landed::Uncertain { .. })) {
                if landed.is_err() && node.is_me(&answer.to.machine) && !node.residents.knows(&answer.to.resident) {
                    let about = node.world.conversation(&answer.to.resident).about();
                    match fastctx_platform::task::blocking(move || fastctx_platform::disk::exists(about)).await {
                        Ok(false) => {
                            let (settling, answer) = (directory.clone(), answer.clone());
                            if let Err(error) = fastctx_platform::task::spawn_blocking(move || {
                                crate::inbox::without_reader(&settling, &answer)
                            })
                            .await
                            .map_err(std::io::Error::other)
                            .and_then(|result| result)
                            {
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
            let (settling, key) = (directory.clone(), answer.delivery.id.clone());
            if let Err(error) = fastctx_platform::task::spawn_blocking(move || crate::inbox::paid(&settling, &key))
                .await
                .map_err(std::io::Error::other)
                .and_then(|result| result)
            {
                pending = true;
                let _ = crate::inbox::outgoing::retry(node, directory, &path, &error.to_string()).await;
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
