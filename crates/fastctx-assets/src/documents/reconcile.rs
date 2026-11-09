//! Online rejection and offline branch retention are distinct outcomes. This
//! module never assigns revisions or silently changes the selected version.
use super::{
    Document, Kind,
    text::{self, Candidate, Prepared},
};
use crate::{Error, json};
use fastctx_sync::{
    catalog::Lifecycle,
    store::mutation::{SubmissionMode, TextEdit},
};

pub enum Intent<'a> {
    Write,
    Edit(&'a [TextEdit]),
}
/// Accounts for the simultaneously held inputs and line-diff working space.
/// Admission supplies this budget from its resource pool, not a product quota.
pub struct Budget {
    pub text_bytes: usize,
    pub work_bytes: usize,
}
pub enum Outcome {
    Applied(Box<Candidate>),
    Rejected {
        reason: String,
    },
    /// The caller must retain base, incoming and current roots under the same
    /// operation. Selecting incoming requires a separate resolution operation.
    Branch(Box<Branch>),
}
pub struct Branch {
    pub base: Document,
    pub base_fence: text::Fence,
    pub selected: Document,
    pub selected_fence: text::Fence,
    pub incoming: Document,
    pub incoming_fence: text::Fence,
    pub reason: String,
}
pub struct Options<'a> {
    pub intent: Intent<'a>,
    pub mode: SubmissionMode,
    pub lifecycle: Lifecycle,
    pub creating: bool,
    pub budget: Budget,
}
pub fn reconcile(
    base: &Prepared<'_>,
    current: &Prepared<'_>,
    incoming: &Prepared<'_>,
    options: Options<'_>,
) -> Result<Outcome, Error> {
    let Options { intent, mode, lifecycle, creating, budget } = options;
    if base.fence().object != current.fence().object
        || base.fence().object != incoming.fence().object
        || !base.document().same_identity(current.document())
        || !base.document().same_identity(incoming.document())
    {
        return Err(json::invalid("a document merge cannot change its identity"));
    }
    if lifecycle == Lifecycle::Deleted {
        return Ok(Outcome::Rejected { reason: "the document was deleted".into() });
    }
    text::validate_text(incoming.document(), incoming.text(), text::Validation::NewWrite)?;
    let inputs = [base.bytes(), current.bytes(), incoming.bytes()];
    let mut work = 0usize;
    for input in inputs {
        if input.len() > budget.text_bytes {
            return Err(Error::Busy);
        }
        let lines = input.iter().filter(|byte| **byte == b'\n').count().checked_add(1).ok_or(Error::Busy)?;
        work = work
            .checked_add(input.len().checked_mul(16).ok_or(Error::Busy)?)
            .and_then(|n| n.checked_add(lines.checked_mul(256)?))
            .ok_or(Error::Busy)?;
    }
    if work > budget.work_bytes {
        return Err(Error::Busy);
    }
    if current.document().retired() || lifecycle == Lifecycle::Removed {
        return conflict(base, current, incoming, mode, "the document was retired");
    }
    if creating && incoming.document().kind()? != Kind::Plan && base.fence() != current.fence() {
        return conflict(base, current, incoming, mode, "the history name was created concurrently");
    }
    let candidate = match intent {
        Intent::Edit(edits) => {
            // Prove the incoming body implements its declared intent before
            // replaying that exact intent against a newer accepted document.
            let intended = text::edit(base, edits, budget.text_bytes)?;
            if intended.bytes() != incoming.bytes() {
                return Err(json::invalid("incoming text differs from its original edit"));
            }
            text::edit(current, edits, budget.text_bytes)
                .and_then(|candidate| text::with_text(incoming.document(), candidate.into_bytes(), budget.text_bytes))
        }
        Intent::Write => match fastctx_types::write::merge::try_merge(base.text(), current.text(), incoming.text()) {
            Some(merged) => text::with_text(incoming.document(), merged.into_bytes(), budget.text_bytes),
            None => return conflict(base, current, incoming, mode, "the document changed and these writes conflict"),
        },
    };
    match candidate {
        Ok(candidate) => Ok(Outcome::Applied(Box::new(candidate))),
        Err(Error::Busy) => Err(Error::Busy),
        Err(Error::Invalid(reason)) => conflict(base, current, incoming, mode, &reason),
        Err(error) => Err(error),
    }
}
fn conflict(
    base: &Prepared<'_>,
    current: &Prepared<'_>,
    incoming: &Prepared<'_>,
    mode: SubmissionMode,
    reason: &str,
) -> Result<Outcome, Error> {
    Ok(match mode {
        SubmissionMode::Online => Outcome::Rejected { reason: reason.into() },
        SubmissionMode::Offline => Outcome::Branch(Box::new(Branch {
            base: base.document().clone(),
            base_fence: base.fence().clone(),
            selected: current.document().clone(),
            selected_fence: current.fence().clone(),
            incoming: incoming.document().clone(),
            incoming_fence: incoming.fence().clone(),
            reason: reason.into(),
        })),
    })
}
