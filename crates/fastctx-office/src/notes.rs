//! Notes slides can contain custom text shapes in addition to the body
//! placeholder. oxide 0.1.13 exposes only that first body placeholder.

use anyhow::{Context, Result, bail};
use office_oxide::core::{
    relationships::{Relationships, rel_types},
    xml,
};
use quick_xml::{Reader, events::Event};
use std::fs::File;
use zip::ZipArchive;

use crate::package::{self, attr};

pub(crate) struct Notes {
    zip: ZipArchive<File>,
    presentation: String,
    rels: Relationships,
}

impl Notes {
    pub fn open(file: File) -> Result<Self> {
        let mut zip = package::open(file)?;
        let root = Relationships::parse(&package::bytes(&mut zip, "_rels/.rels")?)?;
        let presentation = package::target(
            "",
            root.first_by_type(rel_types::OFFICE_DOCUMENT).context("missing presentation relationship")?,
        )?;
        let rels = package::relationships(&mut zip, &presentation)?;
        Ok(Self { zip, presentation, rels })
    }

    pub fn slide(&mut self, id: &str) -> Result<String> {
        let slide =
            package::target(&self.presentation, self.rels.get_by_id(id).context("missing slide relationship")?)?;
        let rels = package::relationships(&mut self.zip, &slide)?;
        let Some(rel) = rels.first_by_type(rel_types::NOTES_SLIDE) else { return Ok(String::new()) };
        let part = package::target(&slide, rel)?;
        let bytes = package::bytes(&mut self.zip, &part)?;
        let mut reader = Reader::from_reader(bytes.as_slice());
        reader.config_mut().expand_empty_elements = true;
        let mut body = String::new();
        let mut shape = String::new();
        let mut placeholder = String::new();
        let mut in_text = false;
        let mut closed = false;
        loop {
            match reader.read_event()? {
                Event::Start(e) => match e.local_name().as_ref() {
                    "sp" | "graphicFrame" => {
                        shape.clear();
                        placeholder.clear();
                    }
                    "ph" => placeholder = attr(&e, "type")?.unwrap_or_default(),
                    "t" => in_text = true,
                    "br" => shape.push('\n'),
                    "tab" => shape.push('\t'),
                    _ => {}
                },
                Event::End(e) => match e.local_name().as_ref() {
                    "t" => in_text = false,
                    "notes" => closed = true,
                    "p" => shape.push('\n'),
                    "sp" | "graphicFrame"
                        if !matches!(placeholder.as_str(), "sldImg" | "sldNum" | "dt" | "hdr" | "ftr") =>
                    {
                        body.push_str(&shape);
                    }
                    _ => {}
                },
                Event::Text(e) if in_text => body_text(&mut shape, &e.xml_content(quick_xml::XmlVersion::Implicit1_0)),
                Event::CData(e) if in_text => body_text(&mut shape, e.as_ref()),
                Event::GeneralRef(e) if in_text => body_text(&mut shape, &xml::resolve_general_ref(&e)?),
                Event::Eof => break,
                _ => {}
            }
        }
        if !closed {
            bail!("notes XML ended before its closing tag: {part}");
        }
        Ok(body)
    }
}

fn body_text(shape: &mut String, text: &str) {
    shape.push_str(text);
}
