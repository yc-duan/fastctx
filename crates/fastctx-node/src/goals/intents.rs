//! Durable account changes and their local effects, recovered by the conversation that owns them.

use fastctx_types::Address;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::Goal;
use fastctx_types::layout::ConversationDir;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub operation: String,
    pub resident: Address,
    pub input_hash: String,
    pub account: HubAccount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::record_access::Access>,
    pub expected: Option<u64>,
    pub proposed: Option<Goal>,
    pub at: Timestamp,
    pub phase: Phase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum Phase {
    Prepared,
    Accepted { version: u64 },
    Applied { version: u64 },
    Conflict { current: Option<u64> },
    Superseded { current: Option<u64> },
}

impl Phase {
    pub fn pending(&self) -> bool {
        matches!(self, Self::Prepared | Self::Accepted { .. })
    }
}

pub(crate) const FAMILY: &str = "goal-intents";
pub(crate) fn prepare(conversation: &ConversationDir) -> io::Result<()> {
    let folder = conversation.books().join("goal-operations");
    crate::records::ensure_each(conversation, FAMILY, move |push| {
        let entries = match fastctx_platform::disk::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json")
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let intent: Intent = serde_json::from_slice(&fastctx_platform::disk::read(&path)?)?;
                if path.file_stem().and_then(|stem| stem.to_str())
                    != Some(fastctx_hands::files::content_hash(intent.operation.as_bytes()).as_str())
                {
                    return Err(crate::record_migration::errors::invalid(
                        "goal operation identity differs from its record",
                    ));
                }
                let mut source =
                    crate::records::Source::new(intent.operation.clone(), path.clone(), serde_json::to_vec(&intent)?)?
                        .ordered(intent.at)
                        .categorized(if intent.phase.pending() { "pending" } else { "terminal" }.into());
                if !intent.phase.pending() {
                    let expiry = source.modified() + jiff::SignedDuration::from_hours(30 * 24);
                    source = source.expiring(expiry);
                }
                push(source)
            })?;
        }
        Ok(())
    })
}
pub(crate) async fn read(conversation: &ConversationDir, operation: &str) -> io::Result<Option<Intent>> {
    let (directory, operation) = (conversation.clone(), operation.to_string());
    fastctx_platform::task::spawn_blocking(move || {
        prepare(&directory)?;
        crate::records::read(&directory, |rows| rows.get(FAMILY, &operation))
    })
    .await
    .map_err(io::Error::other)?
}
pub(crate) fn keep_in(rows: &crate::records::Rows<'_>, intent: &Intent) -> fastctx_storage::Result<()> {
    if let Some(prior) = rows.get::<Intent>(FAMILY, &intent.operation)?
        && (prior.input_hash != intent.input_hash
            || prior.account != intent.account
            || prior.resident != intent.resident)
    {
        return Err(fastctx_storage::Error::new("goal operation identity changed"));
    }
    let pending = intent.phase.pending();
    let expiry = (!pending).then(|| Timestamp::now() + jiff::SignedDuration::from_hours(30 * 24));
    rows.put_until(FAMILY, &intent.operation, intent, expiry)?;
    rows.connection().execute(
        "UPDATE conversation_records SET category=?4,arrival=?5 WHERE conversation=?1 AND family=?2 AND identity=?3",
        (
            rows.conversation(),
            FAMILY,
            &intent.operation,
            if pending { "pending" } else { "terminal" },
            (intent.at.as_nanosecond() ^ i128::MIN).to_be_bytes().as_slice(),
        ),
    )?;
    Ok(())
}
fn keep_now(
    conversation: &ConversationDir,
    intent: &Intent,
    committed: impl FnOnce(&()) + Send + 'static,
) -> io::Result<()> {
    prepare(conversation)?;
    let intent = intent.clone();
    crate::records::write_then_at(
        conversation,
        fastctx_storage::Priority::Interactive,
        serde_json::to_vec(&intent)?.len(),
        move |rows| keep_in(rows, &intent),
        committed,
    )
}
/// Schedule unfinished authority changes from the commit notification, so an
/// abandoned caller cannot leave a durable operation asleep until restart.
pub(crate) fn committed(node: &std::sync::Arc<crate::Node>, intent: &Intent) -> impl FnOnce(&()) + Send + 'static {
    let (node, resident, pending) = (node.clone(), intent.resident.clone(), intent.phase.pending());
    let runtime = tokio::runtime::Handle::current();
    move |_| {
        if pending {
            fastctx_platform::task::spawn_on(&runtime, async move {
                super::recovery::schedule(&node, resident);
            });
        }
    }
}
pub(crate) async fn keep(
    node: &std::sync::Arc<crate::Node>,
    conversation: &ConversationDir,
    intent: &Intent,
) -> io::Result<()> {
    let committed = committed(node, intent);
    let (directory, intent) = (conversation.clone(), intent.clone());
    fastctx_platform::task::spawn_blocking(move || keep_now(&directory, &intent, committed))
        .await
        .map_err(io::Error::other)?
}
/// A bounded prefix. Recovery repeats until the pending family is empty.
pub(crate) fn pending(conversation: &ConversationDir) -> io::Result<Vec<Intent>> {
    prepare(conversation)?;
    crate::records::read(conversation, |rows| {
        let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND category='pending' ORDER BY arrival,identity LIMIT 32")?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for row in query.query_map((rows.conversation(), FAMILY), |row| row.get::<_, Vec<u8>>(0))? {
            let body = row?;
            if !result.is_empty() && bytes + body.len() > 2 * 1024 * 1024 {
                break;
            }
            bytes += body.len();
            result.push(crate::records::decode(&body)?);
        }
        Ok(result)
    })
}
