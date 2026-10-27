//! Business identities follow a computer's public identity; old names are migration aliases only.

use crate::Node;
use fastctx_net::sign::PublicKey;
use fastctx_types::access::Written;
use fastctx_types::layout::ConversationDir;
use fastctx_types::{MachineName, NoticeId};
use std::io;

fn piece(path: &std::path::Path, limit: usize) -> io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("fragment record {} exceeds {limit} bytes", path.display()),
        ));
    }
    Ok(bytes)
}

pub(crate) struct Identity {
    pub id: NoticeId,
    key: PublicKey,
    original: String,
    names: Vec<MachineName>,
    previous: Vec<NoticeId>,
}

impl Identity {
    pub fn new(node: &Node, key: PublicKey, original: &str, names: &[MachineName]) -> Self {
        let mut names = names.to_vec();
        for link in node.links() {
            for member in &link.view().members {
                if member.key == key {
                    names.push(member.name.clone());
                    names.extend(member.was.iter().cloned());
                }
            }
        }
        names.sort();
        names.dedup();
        let previous = names.iter().map(|name| crate::speech::fixed_id("letter", &[name.key(), original])).collect();
        let encoded = serde_json::to_string(&key).expect("public identities serialize");
        let id = crate::speech::fixed_id("letter", &["machine-key", &encoded, original]);
        Self { id, key, original: original.to_string(), names, previous }
    }

    pub fn local(node: &Node, original: &str) -> io::Result<Self> {
        let local = node.machine_names();
        let mut names = local.was;
        names.push(local.name);
        Ok(Self::new(node, crate::world::key(&node.world)?.public(), original, &names))
    }

    pub fn at(node: &Node, machine: &Written, original: &str) -> io::Result<Self> {
        if node.is_here(machine) {
            return Self::local(node, original);
        }
        let mut found = None;
        let mut names = Vec::new();
        for link in node.locate(machine).map_err(io::Error::other)? {
            let view = link.view();
            let Some(member) = view.member(&machine.name) else { continue };
            if found.is_some_and(|key| key != member.key) {
                return Err(io::Error::other(format!("{machine} names different computer identities")));
            }
            found = Some(member.key);
            names.push(member.name.clone());
            names.extend(member.was.iter().cloned());
        }
        let key = found.ok_or_else(|| io::Error::other(format!("the identity of {machine} is not available")))?;
        Ok(Self::new(node, key, original, &names))
    }

    pub(crate) fn original_id(&self) -> &str {
        &self.original
    }

    pub(super) fn source(&self) -> (&PublicKey, &str) {
        (&self.key, &self.original)
    }

    pub fn same_computer(&self, other: &Self) -> bool {
        self.key == other.key
    }

    pub fn with_id(&self, node: &Node, original: &str) -> Self {
        Self::new(node, self.key, original, &self.names)
    }

    pub fn previous(&self) -> impl Iterator<Item = &NoticeId> {
        self.previous.iter()
    }

    pub fn ids(&self) -> impl Iterator<Item = &NoticeId> {
        std::iter::once(&self.id).chain(&self.previous)
    }

    pub fn recognizes(&self, id: &str) -> bool {
        self.ids().any(|known| known.as_str() == id)
    }

    /// An old inbox, uncertainty or delivery receipt keeps its original id.
    /// This also preserves references in already durable host handovers.
    pub fn delivery(&self, directory: &ConversationDir) -> io::Result<NoticeId> {
        for id in self.ids() {
            if crate::inbox::knows(directory, id)?
                || directory.book("received").join(format!("{id}.json")).try_exists()?
            {
                return Ok(id.clone());
            }
        }
        Ok(self.id.clone())
    }

    /// Called while the canonical fragment directory's lease is held.
    pub fn migrate_fragments(&self, root: &std::path::Path, limit: usize) -> io::Result<()> {
        let target = root.join(self.id.as_str());
        for id in self.previous() {
            let old = root.join(id.as_str());
            if !old.try_exists()? {
                continue;
            }
            fastctx_platform::ensure_private_dir(root)?;
            if !target.try_exists()? {
                std::fs::rename(&old, &target)?;
                continue;
            }
            for entry in std::fs::read_dir(&old)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name
                    .to_str()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fragment name is not UTF-8"))?;
                if name.starts_with('.') {
                    continue;
                }
                if name != "done" && name.parse::<usize>().is_err() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unrecognized fragment record {}", entry.path().display()),
                    ));
                }
                let bytes = piece(&entry.path(), limit)?;
                let to = target.join(name);
                match piece(&to, limit) {
                    Ok(kept) if kept != bytes => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "fragment migration conflicts between {} and {}; both were preserved",
                                entry.path().display(),
                                to.display()
                            ),
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                fastctx_platform::publish(&to, &bytes, fastctx_platform::Permissions::Private)?;
                std::fs::remove_file(entry.path())?;
            }
            match std::fs::remove_dir(&old) {
                Err(error) if error.kind() != io::ErrorKind::DirectoryNotEmpty => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn consume(&self, directory: &ConversationDir) -> io::Result<()> {
        for id in self.ids() {
            crate::inbox::consume(directory, id)?;
        }
        Ok(())
    }
}
