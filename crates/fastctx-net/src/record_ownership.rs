//! A record's readers are independent of the accounts its agent works for.

use crate::authorization::{Action, Lookup, authorize};
use crate::directory::{Directory, Member};
use fastctx_types::{AccountName, MachineName};

/// The opener always owns the record. A non-assisted birth also belongs to
/// the current holders of its originating machine. Administration alone is
/// not ownership, and sharing never grants access to an assisted birth.
pub fn allows<L: Lookup + ?Sized>(
    directory: &L,
    reader: &AccountName,
    openers: &[AccountName],
    shared_on: Option<&MachineName>,
) -> bool {
    directory.find_account(reader).is_some_and(|account| !account.locked)
        && (openers.contains(reader)
            || shared_on.is_some_and(|machine| authorize(directory, reader, Action::Hold(machine)).is_ok()))
}

/// A machine as a birth on it is sealed: its key and the account it is
/// registered under, so neither a newcomer taking its name nor a new owner of
/// the same machine inherits what its earlier holders shared there.
pub fn seal(member: &Member) -> Option<String> {
    member.home.as_ref().map(|home| format!("{}:{home}", member.key))
}

/// A registered, logged-in computer keeps the records of its own account.
/// A shared computer keeps only records belonging to its own holders, never
/// one sharer's private records. Managed machines and phones keep no files.
pub fn keeps(directory: &Directory, member: &Member, openers: &[AccountName], shared_on: Option<&MachineName>) -> bool {
    !member.managed
        && member.is_machine()
        && member
            .home
            .as_ref()
            .is_some_and(|home| member.logins.contains(home) && allows(directory, home, openers, shared_on))
        && directory
            .shares
            .iter()
            .filter(|share| share.machine == member.name && directory.holds(&share.account, &member.name))
            .all(|share| allows(directory, &share.account, openers, shared_on))
}
