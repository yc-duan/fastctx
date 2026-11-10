//! The representation row selects storage; size never determines how old bytes are read.

use super::files::FileKey;
use crate::{manifest::Representation, store::Error};

pub(super) enum Location {
    File(FileKey),
    Inline,
}
impl Location {
    pub fn decode(key: &str, representation: &Representation) -> Result<Self, Error> {
        if key == "inline" { Ok(Self::Inline) } else { FileKey::decode(key, representation).map(Self::File) }
    }
    pub fn storage_key(&self) -> Result<String, Error> {
        match self {
            Self::File(key) => key.json(),
            Self::Inline => Ok("inline".into()),
        }
    }
    pub fn rebind(&self, representation: &Representation) -> Result<Self, Error> {
        match self {
            Self::File(key) => key.rebind(representation).map(Self::File),
            Self::Inline => Ok(Self::Inline),
        }
    }
}
