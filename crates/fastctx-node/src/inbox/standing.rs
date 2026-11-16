//! Where each of the user's words stands in the inbox, for the asset
//! library's account of them (`crate::timeline::words`, design §28.14): the
//! inbox delivers them, and the asset library follows what it did.

use std::collections::HashSet;
use std::io;
use std::ops::ControlFlow;

use fastctx_types::NoticeId;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Pending, PendingState};

use super::storage::{Receipt, read_rows};

/// Where words stand in the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stood {
    /// Kept, waiting for a way in.
    Kept,
    /// Crossing into the host: handed to its plugin, being put into its
    /// thread, or the first words of a session started again.
    Delivering,
    /// The host's record or the turn that took them says they went in.
    Delivered,
    /// They may have gone in; nothing said which turn took them.
    Uncertain,
    /// The host refused them.
    Refused,
    /// Let go before they went in.
    Gone,
    /// Nothing of them here at all: never kept by this inbox, as when the
    /// node stopped between the asset library keeping them and the inbox.
    Lost,
}

/// Where words of the inbox stand: those asked of, in their order, and the
/// others it keeps or holds in doubt.
pub(crate) struct Standing {
    pub(crate) asked: Vec<Stood>,
    pub(crate) others: Vec<(Pending, Stood)>,
}

/// Where each of `ids` stands, in their order; and the words `of_words`
/// picks out of what is kept or in doubt that are not among `ids`, with
/// where they stand.
pub(crate) fn words(
    directory: &ConversationDir,
    ids: &[String],
    of_words: impl Fn(&Pending) -> bool,
) -> io::Result<Standing> {
    let asked = ids.to_vec();
    let mut stood: Vec<Stood> = read_rows(directory, move |rows| {
        let mut stood = Vec::with_capacity(asked.len());
        for id in &asked {
            // A receipt says they went in, whatever is kept beside it: an
            // archived conversation keeps what waited (MM07).
            let found =
                if rows.get::<Receipt>("delivery-receipts", id)?.is_some_and(|receipt| receipt != Receipt::Answered) {
                    Stood::Delivered
                } else if let Some(pending) = rows.get::<Pending>("pending-inputs", id)? {
                    if pending.state == PendingState::Waiting { Stood::Kept } else { Stood::Delivering }
                } else if rows.get::<Pending>("uncertain-inputs", id)?.is_some() {
                    Stood::Uncertain
                } else if rows.get::<bool>(super::WITHDRAWN, id)?.is_some() {
                    Stood::Gone
                } else {
                    Stood::Lost
                };
            stood.push(found);
        }
        Ok(stood)
    })?;
    for (id, found) in ids.iter().zip(stood.iter_mut()) {
        if matches!(*found, Stood::Gone | Stood::Lost)
            && let Ok(id) = NoticeId::try_from(id.as_str())
            && crate::lifecycle::refused::contains(directory, &id)?
        {
            *found = Stood::Refused;
        }
    }
    let known: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut others = Vec::new();
    for pending in
        super::those(directory, |pending| of_words(pending) && !known.contains(pending.delivery.id.as_str()))?
    {
        let found = if pending.state == PendingState::Waiting { Stood::Kept } else { Stood::Delivering };
        others.push((pending, found));
    }
    super::uncertain_pages(directory, |page| {
        others.extend(
            page.into_iter()
                .filter(|pending| of_words(pending) && !known.contains(pending.delivery.id.as_str()))
                .map(|pending| (pending, Stood::Uncertain)),
        );
        Ok(ControlFlow::Continue(()))
    })?;
    Ok(Standing { asked: stood, others })
}
