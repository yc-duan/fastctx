//! Retirement and restoration keep a verified revision's exact text and parsed
//! description. New authoring limits do not invalidate an older saved revision.
use super::text::{self, Attribution, Candidate, Prepared};
use crate::{Error, json};

pub fn retire(
    current: &Prepared<'_>,
    author: fastctx_types::history::Author,
    submission: Option<String>,
    max_bytes: usize,
) -> Result<Candidate, Error> {
    if current.bytes().len() > max_bytes {
        return Err(Error::Busy);
    }
    let mut candidate = text::retained(current);
    candidate.set_retired(true)?;
    candidate.attribute(Attribution::History(author), submission)?;
    Ok(candidate)
}
pub fn restore(
    current: &Prepared<'_>,
    chosen: &Prepared<'_>,
    author: fastctx_types::history::Author,
    submission: Option<String>,
    max_bytes: usize,
) -> Result<Candidate, Error> {
    if current.fence().object != chosen.fence().object || !current.document().same_identity(chosen.document()) {
        return Err(json::invalid("restore source belongs to another history topic"));
    }
    if chosen.bytes().len() > max_bytes {
        return Err(Error::Busy);
    }
    let mut candidate = text::retained(chosen);
    candidate.set_retired(false)?;
    candidate.attribute(Attribution::History(author), submission)?;
    Ok(candidate)
}
