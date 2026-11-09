//! Retirement and restoration create a new history revision without discarding
//! the selected source revision. Actual revision assignment belongs to the hub.
use super::{
    Details,
    text::{self, Attribution, Candidate, Prepared},
};
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
    let mut document = current.document().clone();
    let Details::History { retired, .. } = &mut document.details else {
        return Err(json::invalid("a plan is not retired as a history topic"));
    };
    *retired = true;
    let mut candidate = text::with_text(&document, current.bytes().to_vec(), max_bytes)?;
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
    if !current.document().same_identity(chosen.document()) {
        return Err(json::invalid("restore source belongs to another history topic"));
    }
    if chosen.bytes().len() > max_bytes {
        return Err(Error::Busy);
    }
    let mut document = current.document().clone();
    let Details::History { retired, .. } = &mut document.details else {
        return Err(json::invalid("a plan is not restored as a history topic"));
    };
    *retired = false;
    let mut candidate = text::with_text(&document, chosen.bytes().to_vec(), max_bytes)?;
    candidate.attribute(Attribution::History(author), submission)?;
    Ok(candidate)
}
