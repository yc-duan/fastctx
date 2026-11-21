//! What the World keeps on this machine, and deleting it: the requests the
//! settings page 本机数据, the TUI and `fastctx data` make (`DataCall`). They
//! are answered here and never go to another machine: each machine manages
//! only its own data.

pub(crate) mod delete;
mod host;
pub(crate) mod places;
mod space;

use std::sync::{Arc, Mutex};

use fastctx_types::data::{DataCall, DataReply, Space};
use fastctx_types::ipc::Failure;

use crate::Node;

/// What was last measured here, for a page to show at once while it asks
/// again (`DataCall::List` with `kept`); and the one measure under way,
/// which those asking meanwhile wait for rather than walking everything
/// again beside it. Kept while the node runs, and in its cache for the
/// next run (`kept_file`): the page says when it was measured.
#[derive(Default)]
pub(crate) struct Measured {
    /// The last measure, with when it began.
    last: Mutex<Option<(jiff::Timestamp, Space)>>,
    measuring: tokio::sync::Mutex<()>,
}

impl Measured {
    fn last(&self) -> Option<(jiff::Timestamp, Space)> {
        self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// Where the last measure stays between the node's runs: in its cache,
/// which clearing it empties as it does whatever else is made again.
fn kept_file(node: &Node) -> std::path::PathBuf {
    node.root.join("cache").join("data-space.json")
}

pub(crate) async fn answer(node: &Arc<Node>, call: DataCall) -> Result<DataReply, Failure> {
    match call {
        DataCall::List { kept: true } => {
            if let Some((_, space)) = node.data_space.last() {
                return Ok(DataReply::Listed(Box::new(space)));
            }
            // Not measured yet in this run of the node: as the last left it.
            let file = kept_file(node);
            let kept = fastctx_platform::task::spawn_blocking(move || {
                fastctx_platform::disk::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Space>(&bytes).ok())
            })
            .await
            .ok()
            .flatten();
            Ok(kept.map_or(DataReply::Unmeasured, |space| DataReply::Listed(Box::new(space))))
        }
        DataCall::List { kept: false } => {
            let asked = jiff::Timestamp::now();
            let _turn = node.data_space.measuring.lock().await;
            // Begun after this asked, while it waited for its turn: what it
            // asked for. One begun before, deletions since may have missed.
            if let Some((_, space)) = node.data_space.last().filter(|(began, _)| *began >= asked) {
                return Ok(DataReply::Listed(Box::new(space)));
            }
            let began = jiff::Timestamp::now();
            let measuring = node.clone();
            let space = fastctx_platform::task::spawn_blocking(move || space::measure(&measuring))
                .await
                .map_err(|error| Failure::new(error.to_string()))?
                .map_err(|error| Failure::new(format!("measuring what is kept here: {error}")))?;
            *node.data_space.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((began, space.clone()));
            let (file, keeping, log) = (kept_file(node), space.clone(), node.log.clone());
            // Written behind the answer: the page waits for nothing of it.
            drop(fastctx_platform::task::spawn_blocking(move || {
                let kept = serde_json::to_vec(&keeping).map_err(std::io::Error::other).and_then(|bytes| {
                    fastctx_platform::disk::create_dir_all(file.parent().expect("in the cache"))?;
                    fastctx_platform::publish(&file, &bytes, fastctx_platform::Permissions::Private)
                });
                if let Err(error) = kept {
                    log.line(format_args!("keeping what was measured here for the next run: {error}"));
                }
            }));
            Ok(DataReply::Listed(Box::new(space)))
        }
        DataCall::Delete { names } => Ok(DataReply::Deleted { deletions: delete::delete(node, names).await }),
        DataCall::ClearCache => {
            let root = node.root.join("cache");
            let bytes = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<u64> {
                let viewed = root.join("viewed");
                let mut turn = fastctx_platform::viewed_cache::Turn::acquire(&viewed)?;
                let before = space::tally(&root)?.bytes;
                turn.clear()?;
                for path in space::entries(&root)? {
                    if path != viewed {
                        fastctx_platform::owned_directory::delete(&path)?;
                    }
                }
                Ok(before.saturating_sub(space::tally(&root)?.bytes))
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("clearing this machine's cache: {error}")))?;
            Ok(DataReply::CacheCleared { bytes })
        }
        DataCall::ClearCopies { hub, account, apply, only } => {
            let clearing = node.clone();
            let only: Option<std::collections::BTreeSet<(String, i64)>> =
                only.map(|held| held.into_iter().map(|one| (one.scope, one.holding)).collect());
            let cleared = fastctx_platform::task::spawn_blocking(move || {
                crate::assets::copies::clear(&clearing, &hub, &account, apply, only.as_ref(), None)
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::not_run)?;
            Ok(DataReply::Cleared(cleared))
        }
        DataCall::Undescribed => {
            let scopes = crate::assets::undescribed::read(&node.world.assets())
                .into_iter()
                .map(|(scope, never)| {
                    let conversation = scope.rsplit('/').next().unwrap_or_default().to_string();
                    let kept = fastctx_types::ResidentId::try_from(conversation.as_str())
                        .is_ok_and(|id| node.residents.on_record(&id).is_some());
                    fastctx_types::data::Undescribed { scope, conversation, kept, why: never.why, at: never.at }
                })
                .collect();
            Ok(DataReply::Undescribed { scopes })
        }
        DataCall::Copies => {
            let node = node.clone();
            let accounts = fastctx_platform::task::spawn_blocking(move || crate::assets::copies::report(&node))
                .await
                .map_err(|error| Failure::new(error.to_string()))?;
            Ok(DataReply::Copies { accounts })
        }
        DataCall::SetCopies { hub, account, on } => {
            use crate::assets::copies::{Source, State};
            let setting = node.clone();
            let state = if on { State::On } else { State::Paused };
            fastctx_platform::task::spawn_blocking(move || {
                crate::assets::copies::set_here(&setting, &hub, &account, None, state, Source::Owner)
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::not_run)?;
            let node = node.clone();
            let accounts = fastctx_platform::task::spawn_blocking(move || crate::assets::copies::report(&node))
                .await
                .map_err(|error| Failure::new(error.to_string()))?;
            Ok(DataReply::Copies { accounts })
        }
        DataCall::AppOpened { hub, account, opened } => {
            use crate::assets::copies::{Source, State};
            let setting = node.clone();
            fastctx_platform::task::spawn_blocking(move || {
                crate::assets::copies::set_here(&setting, &hub, &account, Some(opened), State::On, Source::App)
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::not_run)?;
            Ok(DataReply::Copies { accounts: Vec::new() })
        }
        DataCall::OutputDays { days } => {
            if days.is_some() {
                return Err(Failure::not_run("Job output is an asset and has no retention deadline."));
            }
            Ok(DataReply::OutputDays { days: 30, output_retention: Some("unlimited".into()) })
        }
    }
}
