//! The network's versions (`fastctx_types::update`): what this machine runs
//! and knows of, told to a hub that keeps them, at each session and as it
//! changes; what the hub tells back; the packages it holds, downloaded from
//! it; and the ones it asks this machine to fetch for others, delivered.

use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_types::update::{Fetch, Network, Report};
use serde::{Deserialize, Serialize};

use super::{Inner, Link};
use crate::data::{CHUNK, Endpoint, Stream};
use crate::wire::{Intent, Question, Told, Up};

#[derive(Default)]
pub(crate) struct Versions {
    /// The latest report, and the session it was told on.
    report: Mutex<(Option<Report>, Option<u64>)>,
    /// What the hub last told.
    network: Mutex<Option<Network>>,
}

/// What each end of a package's data connection says before its bytes.
#[derive(Serialize, Deserialize)]
pub struct Sized {
    pub size: u64,
}

impl Inner {
    /// Tells the hub the versions of this machine, once the session takes
    /// them and again whenever they change.
    pub(crate) fn tell_versions(&self, report: Report) {
        let mut kept = self.versions.report.lock().expect("never poisoned");
        if kept.0.as_ref() == Some(&report) {
            return;
        }
        *kept = (Some(report), None);
        self.send_versions(&mut kept);
    }

    /// The session `generation` has the view: a hub that keeps versions is
    /// told this machine's.
    pub(super) fn versions_synced(&self, generation: u64) {
        let mut kept = self.versions.report.lock().expect("never poisoned");
        if kept.1 != Some(generation) {
            kept.1 = None;
            self.send_versions(&mut kept);
        }
    }

    fn send_versions(&self, kept: &mut (Option<Report>, Option<u64>)) {
        let Some(report) = &kept.0 else { return };
        if !self.view().versions {
            return;
        }
        let Some(connection) = self.connection() else { return };
        if connection.up.send(Up::Versions { report: Box::new(report.clone()) }).is_ok() {
            kept.1 = Some(connection.generation);
        }
    }

    pub(super) fn heard_versions(&self, network: Network) {
        *self.versions.network.lock().expect("never poisoned") = Some(network);
        self.node.versions_changed();
    }

    /// The hub asks this machine to fetch a package for others: the node
    /// brings it here, checked, and it is delivered; or the hub is told why
    /// not, and asks another.
    pub(super) fn fetch_for_hub(self: &Arc<Self>, fetch: Fetch) {
        let inner = self.clone();
        tokio::spawn(async move {
            let id = fetch.id;
            let delivered = match inner.node.relay(fetch).await {
                Ok(path) => inner.deliver(id, &path).await,
                Err(why) => Err(why),
            };
            if let Err(why) = delivered {
                inner.node.log(format!("fetching a package for the hub's request {id}: {why}"));
                if let Some(connection) = inner.connection() {
                    let _ = connection.up.send(Up::Unfetched { id, why });
                }
            }
        });
    }

    async fn deliver(&self, id: u64, path: &Path) -> Result<(), String> {
        let mut file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        let Endpoint::Hub { upstreams, hub, cert } = self.data_endpoint() else {
            return Err("packages go to the hub itself".to_string());
        };
        let mut stream =
            Stream::to_hub(&upstreams, &hub, cert, &self.key, |key, proof| Intent::Deliver { key, proof, id }).await?;
        stream.say(&Sized { size }).await?;
        let mut buffer = vec![0; CHUNK];
        loop {
            let read = file.read(&mut buffer).map_err(|error| format!("{}: {error}", path.display()))?;
            if read == 0 {
                break;
            }
            stream.send(&buffer[..read]).await?;
        }
        stream.finish().await
    }
}

impl Link {
    /// Tells the hub this machine's versions, now and at every session.
    pub fn report_versions(&self, report: Report) {
        self.inner.tell_versions(report);
    }

    /// What the hub last told of the network's versions; none from a hub
    /// that does not keep them, or before it told.
    pub fn versions(&self) -> Option<Network> {
        self.inner.versions.network.lock().expect("never poisoned").clone()
    }

    /// Whether the hub keeps the network's versions and hands out packages.
    pub fn keeps_versions(&self) -> bool {
        self.inner.view().versions
    }

    /// Asks the hub for the package `target` of `version`, answered once
    /// it holds it, bringing it in first when it has to: its size.
    pub async fn ask_package(&self, version: &str, target: &str) -> Result<u64, String> {
        if !self.keeps_versions() {
            return Err("the network's hub does not hand out packages".to_string());
        }
        match self.inner.ask(Question::Package { version: version.to_string(), target: target.to_string() }).await? {
            Told::Package { size } => Ok(size),
            _ => Err("the hub answered something other than a package".to_string()),
        }
    }

    /// Downloads the package `target` of `version`, `size` bytes the hub
    /// holds (`ask_package`), into `file`, going on from what an earlier
    /// download left there. `progress` hears the bytes in the file and the
    /// package's size as they grow. The bytes are the caller's to check
    /// against the package's signature.
    pub async fn package(
        &self,
        version: &str,
        target: &str,
        size: u64,
        file: &Path,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<(), String> {
        // Written at the end it reaches, and cut back when it is longer than
        // the package: a file opened only to append cannot be cut on Windows.
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let mut have = out.seek(std::io::SeekFrom::End(0)).map_err(|error| error.to_string())?;
        if have > size {
            out.set_len(0).map_err(|error| error.to_string())?;
            out.seek(std::io::SeekFrom::Start(0)).map_err(|error| error.to_string())?;
            have = 0;
        }
        progress(have, Some(size));
        if have < size {
            let Endpoint::Hub { upstreams, hub, cert } = self.inner.data_endpoint() else {
                return Err("packages come from the hub itself".to_string());
            };
            let (version, target) = (version.to_string(), target.to_string());
            let mut stream = Stream::to_hub(&upstreams, &hub, cert, &self.inner.key, |key, proof| Intent::Package {
                key,
                proof,
                version,
                target,
                from: have,
            })
            .await?;
            let said: Sized = stream.hear().await?;
            if said.size != size {
                return Err(format!("the hub holds {} bytes of the package now, not {size}", said.size));
            }
            let mut written = have;
            stream
                .take(size - have, |piece| {
                    out.write_all(piece).map_err(|error| format!("{}: {error}", file.display()))?;
                    written += piece.len() as u64;
                    progress(written, Some(size));
                    Ok(())
                })
                .await?;
            out.sync_all().map_err(|error| error.to_string())?;
            stream.done().await?;
        }
        Ok(())
    }
}

/// Where a package is kept while it comes, beside where it goes.
pub fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}
