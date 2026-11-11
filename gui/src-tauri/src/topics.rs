//! One cancellable wait for changes to the local node's named state.

use std::sync::atomic::Ordering;

use fastctx_types::topics::{TopicsAsk, TopicsTold};
use tauri::ipc::Channel;

use crate::Shell;

#[derive(Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Answer {
    Told { told: TopicsTold },
    Unsupported,
    Aborted,
    Error { error: String },
}

struct Answering(Option<Channel<Answer>>);

impl Answering {
    fn finish(mut self, reply: Answer) {
        if let Some(answer) = self.0.take() {
            let _ = answer.send(reply);
        }
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        if let Some(answer) = self.0.take() {
            let _ = answer.send(Answer::Aborted);
        }
    }
}

/// `unwatch` cancels this wait when the subscriptions or visibility change.
#[fastctx_gui_command::command]
pub(crate) async fn world_topics(
    shell: tauri::State<'_, Shell>,
    request: TopicsAsk,
    answer: Channel<Answer>,
) -> Result<u64, String> {
    request.validate()?;
    let page = shell.page.load(Ordering::Relaxed);
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    let world = shell.world();
    // Capture before spawning: even a task aborted before its first poll replies.
    let answer = Answering(Some(answer));
    shell.follow(page, id, async move {
        let reply = match world.topics_when_supported(request).await {
            Ok(Some(told)) => Answer::Told { told },
            Ok(None) => Answer::Unsupported,
            Err(error) => Answer::Error { error: error.to_string() },
        };
        answer.finish(reply);
    })?;
    Ok(id)
}
