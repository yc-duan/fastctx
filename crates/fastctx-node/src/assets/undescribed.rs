//! The scopes of conversations begun here that their hub said it will never
//! describe: the account they were begun for is gone from it, or another
//! account of that name stands there now. Kept in `undescribed.json` beside
//! the asset library, they are asked no more — no pass fails for them, and
//! none is described later for an account of that name opened since — but
//! once a day where the hub judges which account each was begun for, which
//! then describes it for that one alone (`fastctx_types::assets::BORN`); and
//! go with their conversation (`let_go`).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const FILE: &str = "undescribed.json";
/// One change at a time: each hub's driver may make one.
static CHANGING: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Undescribed {
    pub(crate) why: String,
    pub(crate) at: jiff::Timestamp,
}

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    scopes: BTreeMap<String, Undescribed>,
}

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// The scopes kept as never to be described, in the asset library's
/// directory `dir`; none where none are kept or the file cannot be read.
pub(crate) fn read(dir: &Path) -> BTreeMap<String, Undescribed> {
    fastctx_platform::durable_file::read_bounded(&path(dir), 4 << 20)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .map(|kept| kept.scopes)
        .unwrap_or_default()
}

fn change(dir: &Path, change: impl FnOnce(&mut BTreeMap<String, Undescribed>)) -> io::Result<()> {
    let _changing = CHANGING.lock().expect("never poisoned");
    let mut kept = Kept { scopes: read(dir) };
    change(&mut kept.scopes);
    let mut bytes = serde_json::to_vec_pretty(&kept).map_err(io::Error::other)?;
    bytes.push(b'\n');
    fastctx_platform::publish(&path(dir), &bytes, fastctx_platform::Permissions::Private)
}

/// Keeps `scope` as one its hub will never describe, and why.
pub(crate) fn keep(dir: &Path, scope: &str, why: &str) -> io::Result<()> {
    change(dir, |scopes| {
        scopes.insert(scope.to_string(), Undescribed { why: why.to_string(), at: jiff::Timestamp::now() });
    })
}

/// `scope` was described after all: no longer kept as never to be.
pub(crate) fn forget(dir: &Path, scope: &str) -> io::Result<()> {
    change(dir, |scopes| {
        scopes.remove(scope);
    })
}

/// How long a scope kept as never to be described goes unasked.
const RECHECK_SECONDS: i64 = 24 * 60 * 60;

/// Whether what was kept of a scope as never to be described is old enough
/// to be asked again, where it may be.
pub(crate) fn due(kept: &Undescribed) -> bool {
    jiff::Timestamp::now().as_second().saturating_sub(kept.at.as_second()) >= RECHECK_SECONDS
}

/// Lets go, with the conversation `id` as it is deleted, the scopes begun
/// here for it that no hub described: nothing of them went anywhere, and
/// none would ever be asked to go. Each is let go only while it is so still
/// (`Library::revoke_proposed`); one described meanwhile goes with the
/// deletion as any described does. Says how many went. What this leaves —
/// cut short — the driver lets go once the conversation is gone
/// (`session::describe`).
pub(crate) async fn let_go(node: &crate::Node, id: &str) -> Result<usize, String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(0) };
    let library = assets.library.clone();
    let id = id.to_string();
    let gone = fastctx_platform::task::blocking(move || -> Result<Vec<String>, String> {
        let mut gone = Vec::new();
        for (key, _) in library.provisionals().map_err(|error| error.to_string())? {
            if fastctx_types::assets::of_conversation(&key)
                && key.rsplit('/').next() == Some(id.as_str())
                && library.revoke_proposed(&key).map_err(|error| error.to_string())?
            {
                gone.push(key);
            }
        }
        Ok(gone)
    })
    .await?;
    if !gone.is_empty() {
        let dir = node.world.assets();
        fastctx_platform::task::blocking(move || {
            change(&dir, |scopes| {
                for key in &gone {
                    scopes.remove(key);
                }
            })
            .map(|()| gone.len())
        })
        .await
        .map_err(|error| error.to_string())
    } else {
        Ok(0)
    }
}
