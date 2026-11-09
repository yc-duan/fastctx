//! Document permissions follow the record's ownership, not whether the editing
//! agent happens to be the conversation whose plan is being changed.
use super::{Details, Document, Kind, Target};
use crate::Error;
use fastctx_sync::{
    catalog::Lifecycle,
    identity::{AccountId, ObjectKey},
    store::access::{AccessContext, Principal},
};
use fastctx_types::{history, plan, project::Writer};

pub enum Attribution<'a> {
    Plan(&'a plan::Author),
    History(&'a history::Author),
}
/// Current directory/ownership and authenticated author facts. `permitted`
/// includes Project or record_ownership, opened, holding and any path Work gate.
/// No part of this evidence is populated from the incoming document description.
pub struct Authority<'a> {
    pub context: &'a AccessContext,
    pub object: &'a ObjectKey,
    pub target: &'a Target,
    pub permitted: bool,
    pub account: &'a AccountId,
    pub acl_revision: fastctx_sync::cursor::Revision,
    pub logged_in: bool,
    pub act_for: bool,
    pub author: Attribution<'a>,
    /// Canonical identity resolved from the real resident author's address.
    pub writer: Option<&'a ObjectKey>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Write,
    Edit,
    Retire,
    Restore,
}
pub struct Change<'a> {
    pub before: Option<&'a Document>,
    pub after: &'a Document,
    pub action: Action,
    pub lifecycle: Lifecycle,
    pub history_observed: bool,
    pub restore_source: Option<&'a Document>,
}
pub fn authorize(change: Change<'_>, authority: Authority<'_>) -> Result<(), Error> {
    let after = change.after;
    match change.action {
        Action::Write | Action::Edit => after.validate()?,
        Action::Retire | Action::Restore => {
            after.validate_stored()?;
            if let Details::History { author, submission, .. } = &after.details {
                super::schema::attribution(author)?;
                super::schema::submission_id(submission)?;
            }
        }
    }
    if !authority.permitted
        || authority.context.scope != authority.object.scope
        || authority.context.observed_acl != authority.acl_revision
        || authority.object.scope != after.target.object().scope
        || &after.target != authority.target
        || change.lifecycle == Lifecycle::Deleted
        || change.before.is_some_and(|before| !before.same_identity(after))
    {
        return Err(denied("document identity or record permission changed"));
    }
    let user = match &authority.context.principal {
        Principal::Account { account, .. } if account == authority.account && authority.logged_in => true,
        Principal::Agent { account, conversation, .. }
            if account == authority.account && authority.act_for && authority.writer == Some(conversation) =>
        {
            false
        }
        _ => return Err(denied("document editor lacks a current authenticated identity")),
    };
    let attributed = match (&after.details, &authority.author, user) {
        (Details::Plan { author, .. }, Attribution::Plan(expected), true) => {
            author == *expected && matches!(author, plan::Author::User)
        }
        (Details::Plan { author, .. }, Attribution::Plan(expected), false) => {
            author == *expected && matches!(author, plan::Author::Resident { .. })
        }
        (Details::History { author, .. }, Attribution::History(expected), true) => {
            author == *expected && matches!(author.stamp.by, Writer::User)
        }
        (Details::History { author, .. }, Attribution::History(expected), false) => {
            author == *expected && matches!(author.stamp.by, Writer::Resident { .. })
        }
        _ => false,
    };
    if !attributed {
        return Err(denied("document attribution differs from its real caller"));
    }
    match change.action {
        Action::Write | Action::Edit => {
            if after.retired() || change.before.is_some_and(Document::retired) || change.lifecycle == Lifecycle::Removed
            {
                return Err(denied("retired documents require an explicit user restore"));
            }
            if !user
                && change.action == Action::Write
                && after.kind()? != Kind::Plan
                && change.before.is_some_and(|before| before.text.bytes.0 != 0)
                && !change.history_observed
            {
                return Err(denied("view the history document before replacing it whole"));
            }
        }
        Action::Retire | Action::Restore => {
            if !user
                || after.kind()? == Kind::Plan
                || change.before.is_none()
                || after.retired() != (change.action == Action::Retire)
            {
                return Err(denied("history retirement and restoration require their user lifecycle command"));
            }
            let source = if change.action == Action::Retire { change.before } else { change.restore_source }
                .ok_or_else(|| denied("restore requires its verified chosen revision"))?;
            if !source.same_identity(after)
                || source.text.digest != after.text.digest
                || source.text.bytes != after.text.bytes
                || source.text.utf8_bom != after.text.utf8_bom
                || !matches!((&source.details, &after.details),
                    (Details::History { description: a, .. }, Details::History { description: b, .. }) if a == b)
            {
                return Err(denied("lifecycle command changed the chosen revision's text"));
            }
        }
    }
    Ok(())
}
fn denied(message: &str) -> Error {
    Error::Denied(message.into())
}
