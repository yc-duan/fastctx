//! Small skill descriptions reference complete directories. Directory semantics
//! and publication are supplied by materialization, never inferred from a hash.
use crate::{Error, json};
use fastctx_types::{
    skill_files,
    skills::{self, Listed, Skill},
    sync::manifest::ContentRef,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    pub description: String,
    pub directory: ContentRef,
}
impl Metadata {
    pub fn validate(&self) -> Result<(), Error> {
        if !skills::valid_name(&self.name) {
            return Err(json::invalid(skills::words::NAME));
        }
        let description = self.description.trim();
        if description.is_empty()
            || description.chars().count() > skills::DESCRIPTION_MOST
            || description.contains('\n')
        {
            return Err(json::invalid(skills::words::DESCRIPTION));
        }
        if !matches!(self.directory, ContentRef::Manifest { .. }) {
            return Err(json::invalid("a skill requires a complete directory root"));
        }
        self.directory.validate().map_err(Error::Invalid)
    }
    pub fn listed(&self) -> Listed {
        Listed { name: self.name.clone(), description: self.description.clone() }
    }
}
pub fn decode(body: &[u8]) -> Result<Metadata, Error> {
    let metadata: Metadata = json::typed(serde_json::Value::Object(json::object(body)?))?;
    metadata.validate()?;
    Ok(metadata)
}

/// Converts already parsed public metadata. The caller must separately prove
/// that `directory` is the exact prepared files root before publishing it.
pub fn describe(skill: &Skill, directory: ContentRef) -> Result<Metadata, Error> {
    skill.validate().map_err(Error::Invalid)?;
    let value = Metadata { name: skill.name.clone(), description: skill.description.clone(), directory };
    value.validate()?;
    Ok(value)
}
pub fn unique_names(entries: &[Metadata]) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    for entry in entries {
        entry.validate()?;
        if !names.insert(skill_files::path_key(&entry.name)) {
            return Err(json::invalid("skill name is already occupied"));
        }
    }
    Ok(())
}

/// Preserves the old folder editor's pure rewrite rules and auxiliary files.
/// Resource accounting, expected versions, the account-wide legacy editor gate
/// and atomic indexes are adapter work.
pub fn apply_legacy_write(before: &Skill, incoming: &Skill) -> Result<Skill, Error> {
    incoming.validate().map_err(Error::Invalid)?;
    if let Some(files) = &incoming.files {
        return skill_files::from_files(files.clone()).map_err(Error::Invalid);
    }
    if skill_files::requires_folder_editor(before).map_err(Error::Invalid)? {
        return Err(json::invalid("legacy skill editor cannot represent this directory"));
    }
    let mut files = before.folder().map_err(Error::Invalid)?;
    let original =
        files.iter_mut().find(|file| file.path == "SKILL.md").ok_or_else(|| json::invalid("missing SKILL.md"))?;
    *original = skill_files::legacy_document(original, before, incoming).map_err(Error::Invalid)?;
    skill_files::from_files(files).map_err(Error::Invalid)
}
