//! Incremental, read-only title observations from the native session file.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

#[derive(Default)]
pub struct Titles {
    offset: u64,
    anchor: Option<(u64, Vec<u8>)>,
    custom: Option<String>,
    automatic: Option<String>,
}

impl Titles {
    pub fn observe(&mut self, path: &Path) -> io::Result<Option<String>> {
        let mut file = File::open(path)?;
        if file.metadata()?.len() < self.offset {
            *self = Self::default()
        }
        if let Some((offset, line)) = &self.anchor {
            file.seek(SeekFrom::Start(*offset))?;
            let mut current = Vec::new();
            BufReader::new(&mut file).read_until(b'\n', &mut current)?;
            if &current != line {
                *self = Self::default()
            }
        }
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;
        let mut line = Vec::new();
        loop {
            line.clear();
            let length = reader.read_until(b'\n', &mut line)?;
            if length == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                break;
            }
            let row: Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
            self.row(&row)?;
            self.anchor = Some((self.offset, line.clone()));
            self.offset += length as u64;
        }
        Ok(self.title())
    }

    pub fn row(&mut self, row: &Value) -> io::Result<()> {
        let (field, target) = match row["type"].as_str() {
            Some("custom-title") => ("customTitle", &mut self.custom),
            Some("ai-title") => ("aiTitle", &mut self.automatic),
            _ => return Ok(()),
        };
        *target = Some(
            row[field].as_str().ok_or_else(|| io::Error::other(format!("Claude {field} is not a string")))?.to_owned(),
        );
        Ok(())
    }

    pub fn title(&self) -> Option<String> {
        self.custom.clone().or_else(|| self.automatic.clone())
    }
}
