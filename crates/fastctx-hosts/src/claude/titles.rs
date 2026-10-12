//! The session's title, from the rows of the native session file that set it.

use std::io;

use serde_json::Value;

#[derive(Default)]
pub struct Titles {
    custom: Option<String>,
    automatic: Option<String>,
}

impl Titles {
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
