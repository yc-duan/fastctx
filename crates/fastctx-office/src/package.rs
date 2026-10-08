use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use anyhow::{Context, Result, bail};
use office_oxide::core::relationships::{Relationships, TargetMode};
use quick_xml::events::BytesStart;
use zip::ZipArchive;

pub(crate) fn open(mut file: File) -> Result<ZipArchive<File>> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if magic == [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1] {
        bail!("encrypted OOXML package; decryption is not supported");
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(ZipArchive::new(file)?)
}

pub(crate) fn bytes(zip: &mut ZipArchive<File>, part: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    zip.by_name(part).with_context(|| format!("missing package part {part}"))?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn relationships(zip: &mut ZipArchive<File>, part: &str) -> Result<Relationships> {
    let name = match part.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/_rels/{name}.rels"),
        None => format!("_rels/{part}.rels"),
    };
    // OPC omits the relationships part when a source has no relationships.
    match zip.by_name(&name) {
        Err(zip::result::ZipError::FileNotFound) => return Ok(Relationships::empty()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    Ok(Relationships::parse(&bytes(zip, &name)?)?)
}

pub(crate) fn target(source: &str, rel: &office_oxide::core::relationships::Relationship) -> Result<String> {
    if rel.target_mode != TargetMode::Internal {
        bail!("external document part {}", rel.target);
    }
    let path = if rel.target.starts_with('/') {
        rel.target[1..].to_string()
    } else {
        format!("{}{}", source.rsplit_once('/').map(|(p, _)| format!("{p}/")).unwrap_or_default(), rel.target)
    };
    let mut components = Vec::new();
    for part in path.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                if components.pop().is_none() {
                    bail!("package relationship escapes its root");
                }
            }
            _ => components.push(part),
        }
    }
    Ok(components.join("/"))
}

pub(crate) fn attr(element: &BytesStart<'_>, name: &str) -> Result<Option<String>> {
    for a in element.attributes() {
        let a = a?;
        if a.key.local_name().as_ref() == name {
            return Ok(Some(a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?.into_owned()));
        }
    }
    Ok(None)
}
