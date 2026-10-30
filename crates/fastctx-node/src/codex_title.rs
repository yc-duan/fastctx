//! Naming a new Codex conversation the way the official client does
//! (`world-agent-logins.md`, 在哪个登录里干活). When a conversation's login
//! asks for it, the World names the conversation once its first turn is in:
//! the host makes a short title from the first words and names the thread,
//! and the name reaches every device through the ordinary thread-name path.
//! Off by default; the title is made at most once for a conversation.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_types::ResidentId;
use fastctx_types::residents::About;
use fastctx_types::session::{Happened, Speaker};

use crate::Node;

/// Conversations a title was started for, so it is made at most once.
static STARTED: LazyLock<Mutex<HashSet<ResidentId>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Starts naming `id`'s conversation when its login asks for it and it has
/// no name yet. Does nothing for conversations the World did not start, for
/// hosts other than Codex, or once a title was already started here.
pub(crate) fn maybe(node: &Arc<Node>, id: &ResidentId, about: &About) {
    if !about.world_started || about.host.agent.as_str() != "codex" {
        return;
    }
    let Some(thread) = about.session.clone() else { return };
    // A name set by the user, or an earlier title, is left alone.
    if node.residents.now(id).is_some_and(|now| now.title.is_some()) {
        return;
    }
    if !crate::logins::codex_auto_title(node, about.home.as_deref()) {
        return;
    }
    {
        let mut started = STARTED.lock().expect("never poisoned");
        if !started.insert(id.clone()) {
            return;
        }
    }
    let Some(words) = first_user_words(node, id) else {
        STARTED.lock().expect("never poisoned").remove(id);
        return;
    };
    let codex = match node.codex.of(node, about) {
        Ok(codex) => codex,
        Err(_) => {
            STARTED.lock().expect("never poisoned").remove(id);
            return;
        }
    };
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        match codex.make_title(&thread, &words).await {
            // Named by the user, or by its host, while the title was being
            // made: that name stays.
            Ok(Some(_)) if node.residents.now(&id).is_some_and(|now| now.title.is_some()) => {
                node.log.line(format_args!("{id} was named while its title was made; the title is not used"));
            }
            Ok(Some(title)) => match codex.rename(&thread, &title).await {
                Ok(()) => {
                    node.log.line(format_args!("named {id} {title:?}"));
                    super::codex::titled(&node, &id, title).await;
                }
                Err(error) => {
                    STARTED.lock().expect("never poisoned").remove(&id);
                    node.log.line(format_args!("naming {id} {title:?} failed: {error}"));
                }
            },
            Ok(None) => node.log.line(format_args!("no title was made for {id}")),
            Err(error) => {
                // A failed try does not come again on its own, but a later
                // node may: it is no longer counted as started.
                STARTED.lock().expect("never poisoned").remove(&id);
                node.log.line(format_args!("naming {id} failed: {error}"));
            }
        }
    });
}

/// The first words the user said in the conversation, from its record.
fn first_user_words(node: &Node, id: &ResidentId) -> Option<String> {
    let directory = node.world.conversation(id);
    let history = crate::delivery::history(&directory).ok()?;
    history.into_iter().find_map(|event| match event.happened {
        Happened::Heard { from: Some(Speaker::User { .. }), text, .. } if !text.trim().is_empty() => Some(text),
        _ => None,
    })
}
