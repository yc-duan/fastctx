//! Skill folder validation and account-local recovery. A committed intent is
//! the atomic decision for a folder and its index; canonical files are merely
//! materialized from it, under the same account gate used by record readers.

use crate::account_records::storage::{Account, Entry, Slot};
use fastctx_types::{
    account_records::{Metadata, Record},
    skill_files::Limits,
    skills::{self, Skill},
};
use std::path::{Path, PathBuf};

pub(crate) fn limits(slot: &Slot) -> Result<Limits, String> {
    Ok(Limits::resources(
        fastctx_platform::memory::physical().map_err(|error| error.to_string())?,
        fastctx_platform::space::available(slot.root()).map_err(|error| error.to_string())?,
    ))
}

pub(crate) fn check_size(
    slot: &Slot,
    account: &Account,
    old: Option<&Entry>,
    skill: &Skill,
    bytes: usize,
) -> Result<(), String> {
    let limits = limits(slot)?;
    let files = skill.folder()?;
    let size = fastctx_types::skill_files::validate_files(&files)?;
    if size > limits.skill_bytes || files.len() as u64 > limits.files || bytes as u64 > limits.encoded_bytes {
        return Err(format!(
            "the skill exceeds this hub's resource limits ({} decoded bytes, {} encoded bytes, {} files)",
            limits.skill_bytes, limits.encoded_bytes, limits.files
        ));
    }
    let held: usize = account
        .entries
        .values()
        .filter(|entry| entry.metadata.kind == skills::KIND || entry.metadata.kind == skills::INDEX_KIND)
        .map(|entry| entry.size)
        .sum();
    let next = held - old.map_or(0, |entry| entry.size) + bytes;
    if next > held && next as u64 > limits.account_bytes {
        return Err(format!(
            "the account's skills would use {next} bytes; this hub's resource limit is {}",
            limits.account_bytes
        ));
    }
    Ok(())
}

/// Old readers write only instructions. Preserve auxiliary files and replace
/// SKILL.md, instead of treating absence of the new field as an empty folder.
pub(crate) fn normalize(path: &Path, incoming: Skill) -> Result<Skill, String> {
    if let Some(files) = incoming.files {
        return fastctx_types::skill_files::from_files(files);
    }
    incoming.validate()?;
    let old = match std::fs::read(path) {
        Ok(bytes) => {
            let record: Record = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            if record.metadata.deleted {
                None
            } else {
                Some(serde_json::from_value::<Skill>(record.body).map_err(|error| error.to_string())?)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let mut files = old.as_ref().map(|skill| skill.folder()).transpose()?.unwrap_or_default();
    let main = match (old.as_ref(), files.iter().find(|file| file.path == "SKILL.md")) {
        (Some(before), Some(existing)) => fastctx_types::skill_files::legacy_document(existing, before, &incoming)?,
        _ => incoming.folder()?.pop().expect("legacy folder has SKILL.md"),
    };
    files.retain(|file| file.path != "SKILL.md");
    files.push(main);
    fastctx_types::skill_files::from_files(files)
}

pub(crate) fn intent(slot: &Slot) -> PathBuf {
    slot.root().join("skill-edit.json")
}

pub(crate) fn recover(slot: &Slot, account: &mut Account) -> Result<Vec<Metadata>, String> {
    let path = intent(slot);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let records: Vec<Record> = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if !(2..=3).contains(&records.len())
        || records[..records.len() - 1].iter().any(|record| record.metadata.kind != skills::KIND)
        || records.last().is_none_or(|record| record.metadata.kind != skills::INDEX_KIND)
    {
        return Err("invalid committed skill transaction".into());
    }
    for record in &records {
        let target = slot.path(&record.metadata.kind, &record.metadata.key);
        let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
        fastctx_platform::publish(&target, &bytes, fastctx_platform::Permissions::Private)
            .map_err(|error| error.to_string())?;
        let identity = (record.metadata.kind.clone(), record.metadata.key.clone());
        let before = account.entries.get(&identity).map_or(0, |entry| entry.size);
        account.bytes = account.bytes - before + bytes.len();
        account.entries.insert(identity, Entry { metadata: record.metadata.clone(), size: bytes.len() });
    }
    std::fs::remove_file(path).map_err(|error| error.to_string())?;
    slot.count.store(account.entries.len(), std::sync::atomic::Ordering::Relaxed);
    slot.bytes.store(account.bytes, std::sync::atomic::Ordering::Relaxed);
    Ok(records.into_iter().map(|record| record.metadata).collect())
}

/// A legacy rename creates a new key before touching the old one. Refuse its
/// first write once the account uses folder-only data, so failure cannot leave
/// an orphaned replacement that has already lost the auxiliary files.
pub(crate) fn legacy_edit(slot: &Slot, account: &Account) -> Result<(), String> {
    for entry in account.entries.values().filter(|entry| entry.metadata.kind == skills::KIND && !entry.metadata.deleted)
    {
        let record = crate::account_records::storage::read(&slot.path(skills::KIND, &entry.metadata.key))?;
        let skill: Skill = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
        if fastctx_types::skill_files::requires_folder_editor(&skill)? {
            return Err("this account uses skill folders; use the folder-aware skill editor".into());
        }
    }
    Ok(())
}

pub(crate) fn check_index(
    slot: &Slot,
    account: &Account,
    key: &str,
    body: &serde_json::Value,
    deleted: bool,
) -> Result<(), String> {
    if key != skills::INDEX_KEY {
        return Err("the skill index has one account-owned key".into());
    }
    let next: skills::Index = if deleted {
        skills::Index::default()
    } else {
        serde_json::from_value(body.clone()).map_err(|error| error.to_string())?
    };
    let mut names = std::collections::BTreeSet::new();
    for item in &next.skills {
        if !skills::valid_name(&item.name)
            || !names.insert(item.name.clone())
            || item.description.trim().is_empty()
            || item.description.chars().count() > skills::DESCRIPTION_MOST
            || item.description.contains('\n')
        {
            return Err("invalid skill index entry".into());
        }
        if account.entries.get(&(skills::KIND.into(), item.name.clone())).is_none_or(|entry| entry.metadata.deleted) {
            return Err("save a skill before adding it to the index".into());
        }
        let record = crate::account_records::storage::read(&slot.path(skills::KIND, &item.name))?;
        let skill: Skill = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
        if skill.description != item.description {
            return Err("the skill changed before its index was saved; reopen the skill".into());
        }
    }
    if let Some(entry) =
        account.entries.get(&(skills::INDEX_KIND.into(), key.into())).filter(|entry| !entry.metadata.deleted)
    {
        let before: skills::Index =
            serde_json::from_value(crate::account_records::storage::read(&slot.path(skills::INDEX_KIND, key))?.body)
                .map_err(|error| error.to_string())?;
        for item in before.skills.into_iter().filter(|item| !names.contains(&item.name)) {
            if account
                .entries
                .get(&(skills::KIND.into(), item.name.clone()))
                .is_some_and(|entry| !entry.metadata.deleted)
            {
                let record = crate::account_records::storage::read(&slot.path(skills::KIND, &item.name))?;
                let skill: Skill = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
                if fastctx_types::skill_files::requires_folder_editor(&skill)? {
                    return Err("use a folder-aware client to delete or rename a skill with auxiliary files".into());
                }
            }
        }
        let _ = entry;
    }
    let limits = limits(slot)?;
    let bytes = serde_json::to_vec(body).map_err(|error| error.to_string())?.len();
    let held: usize = account
        .entries
        .values()
        .filter(|entry| entry.metadata.kind == skills::KIND || entry.metadata.kind == skills::INDEX_KIND)
        .map(|entry| entry.size)
        .sum();
    let next =
        held - account.entries.get(&(skills::INDEX_KIND.into(), key.into())).map_or(0, |entry| entry.size) + bytes;
    if next > held && next as u64 > limits.account_bytes {
        return Err(format!("the account's skills exceed this hub's {}-byte resource limit", limits.account_bytes));
    }
    Ok(())
}
