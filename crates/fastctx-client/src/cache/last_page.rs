//! Bounded display pages. Callers supply an identity checked against the live scope.

use super::{Cache, key};
use fastctx_types::Address;
use fastctx_types::timeline_pages::TimelineTold;
use std::io::{self, BufRead, BufReader, Read};

const FORMAT: u8 = 1;
const PAGE_LIMIT: u64 = 2 << 20;
const TOTAL_LIMIT: u64 = 24 << 20;
const PAGES: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct LastPageKey {
    pub hub: fastctx_types::access::HubId,
    pub address: Address,
    pub scope: String,
    pub holding: i64,
    pub machine_key: String,
    pub account: fastctx_types::AccountName,
    pub opened: i64,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct Saved {
    format: u8,
    page: TimelineTold,
}

fn address_key(address: &Address) -> String {
    key(&format!("{}\n{}", address.machine.key(), address.resident))
}

impl Cache {
    /// Captured before a reading so deletion cannot republish its answer.
    pub fn last_page_generation(&self) -> io::Result<u64> {
        Ok(self.turn()?.generation())
    }

    pub fn last_page(&self, identity: &LastPageKey) -> io::Result<Option<TimelineTold>> {
        let _turn = self.turn()?;
        let path = self.root().join("last-pages").join(file_name(identity)?);
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(PAGE_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > PAGE_LIMIT {
            return Ok(None);
        }
        let Some(header) = bytes.iter().position(|byte| *byte == b'\n') else { return Ok(None) };
        if serde_json::from_slice::<LastPageKey>(&bytes[..header]).ok().as_ref() != Some(identity) {
            return Ok(None);
        }
        let Ok(Saved { format: FORMAT, page }) = serde_json::from_slice(&bytes[header + 1..]) else { return Ok(None) };
        Ok(usable(&page).then_some(page))
    }

    pub fn keep_last_page(&self, identity: &LastPageKey, generation: u64, page: TimelineTold) -> io::Result<()> {
        if !usable(&page) {
            return Ok(());
        }
        let mut bytes = serde_json::to_vec(identity)?;
        bytes.push(b'\n');
        serde_json::to_writer(&mut bytes, &Saved { format: FORMAT, page })?;
        if bytes.len() as u64 > PAGE_LIMIT {
            return Ok(());
        }
        let turn = self.turn()?;
        if turn.generation() != generation {
            return Ok(());
        }
        let folder = self.root().join("last-pages");
        fastctx_platform::ensure_private_dir(&folder)?;
        let path = folder.join(file_name(identity)?);
        let mut others = Vec::new();
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.path() == path || !entry.file_type()?.is_file() {
                continue;
            }
            let metadata = entry.metadata()?;
            others.push((metadata.modified()?, metadata.len(), entry.path()));
        }
        others.sort_unstable();
        let mut bytes_kept = others.iter().map(|(_, bytes, _)| bytes).sum::<u64>() + bytes.len() as u64;
        let mut count = others.len() + 1;
        for (_, bytes, old) in others {
            if bytes_kept <= TOTAL_LIMIT && count <= PAGES {
                break;
            }
            std::fs::remove_file(old)?;
            bytes_kept -= bytes;
            count -= 1;
        }
        fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
    }

    pub fn forget_last_page_scope(&self, scope: &str) -> io::Result<()> {
        let mut turn = self.turn()?;
        turn.invalidate()?;
        let scope = key(scope);
        self.remove_last_pages(|name| name.split('-').nth(1) == Some(scope.as_str()))
    }

    pub fn discard_last_pages(&self, address: &Address) -> io::Result<()> {
        let mut turn = self.turn()?;
        turn.invalidate()?;
        self.forget_last_pages_of(address)
    }

    pub(super) fn forget_last_pages_of(&self, address: &Address) -> io::Result<()> {
        let address = address_key(address);
        self.remove_last_pages(|name| name.split('-').next() == Some(address.as_str()))
    }

    /// Drops pages on a hub for which its current directory no longer proves access.
    pub fn prune_last_pages(
        &self,
        hub: &fastctx_types::access::HubId,
        allowed: impl Fn(&LastPageKey) -> bool,
    ) -> io::Result<()> {
        let mut turn = self.turn()?;
        let Some(entries) = self.last_pages()? else { return Ok(()) };
        let mut gone = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let mut header = Vec::new();
            BufReader::new(std::fs::File::open(entry.path())?.take(4096)).read_until(b'\n', &mut header)?;
            match serde_json::from_slice::<LastPageKey>(&header) {
                Ok(identity) if identity.hub != *hub || allowed(&identity) => {}
                _ => gone.push(entry.path()),
            }
        }
        if !gone.is_empty() {
            turn.invalidate()?;
            for path in gone {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    pub fn clear_last_pages(&self) -> io::Result<()> {
        let mut turn = self.turn()?;
        turn.invalidate()?;
        match std::fs::remove_dir_all(self.root().join("last-pages")) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    fn last_pages(&self) -> io::Result<Option<std::fs::ReadDir>> {
        match std::fs::read_dir(self.root().join("last-pages")) {
            Ok(entries) => Ok(Some(entries)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn remove_last_pages(&self, remove: impl Fn(&str) -> bool) -> io::Result<()> {
        let Some(entries) = self.last_pages()? else { return Ok(()) };
        for entry in entries {
            let entry = entry?;
            if entry.file_name().to_str().is_some_and(&remove) {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

fn file_name(identity: &LastPageKey) -> io::Result<String> {
    let encoded = serde_json::to_string(identity)?;
    Ok(format!("{}-{}-{}.json", address_key(&identity.address), key(&identity.scope), key(&encoded)))
}

fn usable(page: &TimelineTold) -> bool {
    matches!(page, TimelineTold::Page { revision: Some(_), through: Some(_), initializing, events, .. }
        if *initializing != Some(true) && !events.is_empty() && events.len() <= fastctx_types::timeline_pages::PAGE_EVENTS as usize)
}
