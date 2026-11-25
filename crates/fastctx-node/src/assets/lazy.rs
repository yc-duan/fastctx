//! The streams machines wrote of an account's records before beta.26 that
//! are large and seldom read, each machine's activity, usage and job output
//! (`fastctx_types::assets::records_classes`), are taken here only once
//! something reads them (the user, 2026-11-25: 四条旧账号记录流按需读). A
//! pass passes them over until a reader asks for one, then takes that
//! stream whole and follows it from then on; one this machine holds frames
//! of already, taken before, is followed as before. Job facts, small, and
//! every other stream are taken as they always were; nothing is withheld,
//! and the hub keeps serving them all. A scope is pulled whole once the
//! rest of it is: what waits of these is not owed (`unpulled`).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::Node;

/// The prefixes of the streams taken only once read.
const LAZY: [&str; 3] = ["activity/", "usage/", "output/"];

/// Whether `stream` of `scope` is taken only once read: one of `LAZY`, in
/// an account's records or a body's.
pub(crate) fn lazy(scope: &str, stream: &str) -> bool {
    let account = scope.starts_with("a/") && scope.rsplit('/').next().is_some_and(|last| last.starts_with("records-"));
    let body = scope.starts_with("b/") && scope.ends_with("/records");
    (account || body) && LAZY.iter().any(|prefix| stream.starts_with(prefix))
}

/// The streams taken only once read that something read here: by scope and
/// stream, the holding period each was last taken whole in, none while it
/// waits to be. Kept beside the library, so a node started again follows
/// them on.
pub(crate) struct Wanted {
    path: PathBuf,
    wanted: Mutex<Option<BTreeMap<(String, String), Option<i64>>>>,
}

impl Wanted {
    pub(crate) fn new(library: &Path) -> Self {
        Self { path: library.join("lazy-wanted.json"), wanted: Mutex::new(None) }
    }

    fn with<T>(&self, change: impl FnOnce(&mut BTreeMap<(String, String), Option<i64>>) -> (T, bool)) -> T {
        let mut wanted = self.wanted.lock().expect("never poisoned");
        let wanted = wanted.get_or_insert_with(|| {
            let read = || -> io::Result<Vec<(String, String, Option<i64>)>> {
                Ok(serde_json::from_slice(&fastctx_platform::disk::read(&self.path)?)?)
            };
            read().unwrap_or_default().into_iter().map(|(scope, stream, whole)| ((scope, stream), whole)).collect()
        });
        let (value, changed) = change(wanted);
        if changed {
            let list: Vec<(&String, &String, &Option<i64>)> =
                wanted.iter().map(|((scope, stream), whole)| (scope, stream, whole)).collect();
            // Written whole and put in place; lost, it is asked for again by
            // the next reader.
            if let Ok(bytes) = serde_json::to_vec(&list) {
                let next = self.path.with_extension("json.next");
                let _ = fastctx_platform::disk::write(&next, &bytes)
                    .and_then(|()| fastctx_platform::disk::rename(&next, &self.path));
            }
        }
        value
    }

    /// Asks for `stream` of `scope`, to be taken at the scope's next pass;
    /// whether it had not been asked for.
    pub(crate) fn want(&self, scope: &str, stream: &str) -> bool {
        self.with(|wanted| {
            let new = !wanted.contains_key(&(scope.to_string(), stream.to_string()));
            if new {
                wanted.insert((scope.to_string(), stream.to_string()), None);
            }
            (new, new)
        })
    }

    /// Whether `stream` of `scope` was asked for.
    pub(crate) fn asked(&self, scope: &str, stream: &str) -> bool {
        self.with(|wanted| (wanted.contains_key(&(scope.to_string(), stream.to_string())), false))
    }

    /// The streams of `scope` asked for.
    pub(crate) fn of(&self, scope: &str) -> std::collections::BTreeSet<String> {
        self.with(|wanted| {
            (wanted.keys().filter(|(at, _)| at == scope).map(|(_, stream)| stream.clone()).collect(), false)
        })
    }

    /// `stream` of `scope` was taken whole in `holding`.
    pub(crate) fn taken(&self, scope: &str, stream: &str, holding: i64) {
        self.with(|wanted| {
            let entry = wanted.entry((scope.to_string(), stream.to_string())).or_insert(None);
            let changed = *entry != Some(holding);
            *entry = Some(holding);
            ((), changed)
        });
    }

    /// Whether `stream` of `scope` was taken whole in `holding`.
    pub(crate) fn whole(&self, scope: &str, stream: &str, holding: i64) -> bool {
        self.with(|wanted| (wanted.get(&(scope.to_string(), stream.to_string())) == Some(&Some(holding)), false))
    }
}

/// Whether the stream `stream` of the held scope `scope` is here to be
/// read: always for one not taken only once read, and for one this machine
/// took before; else once it was taken whole. One not here is asked for,
/// taken at the scope's next pass, and the reader reads as if it had none.
pub(crate) fn held(node: &Node, scope: &fastctx_content::v2::HeldScope, stream: &str) -> bool {
    if !lazy(&scope.key, stream) {
        return true;
    }
    let Some(assets) = node.assets.as_ref() else { return false };
    if assets.lazy.whole(&scope.key, stream, scope.holding) {
        return true;
    }
    let asked = assets.lazy.asked(&scope.key, stream);
    // Taken before it was taken only once read: followed as it always was.
    if !asked
        && assets
            .library
            .streams(&scope.key)
            .ok()
            .is_some_and(|streams| streams.iter().any(|(name, next)| name == stream && *next > 0))
    {
        return true;
    }
    if assets.lazy.want(&scope.key, stream) {
        node.log.line(format_args!("taking {stream} of {} here: it was read", scope.key));
        assets.relook.lock().expect("relook").insert(scope.key.clone());
        // Its hub's driver comes round for it; none runs while the hub is
        // away, and it is taken once one does.
        if let Some(link) =
            node.links().into_iter().find(|link| Some(link.membership().id.as_str()) == scope.hub.as_deref())
            && let Some(wake) = assets.drivers.lock().expect("drivers").get(&link.membership().id)
        {
            wake.notify_one();
        }
    }
    false
}
