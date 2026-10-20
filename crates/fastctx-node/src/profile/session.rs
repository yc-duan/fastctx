//! The account layer is selected only for a single-account conversation.

use crate::Node;
use fastctx_types::{access::Acting,profile::Profile};

pub(crate) fn get(node: &Node, acting: &Acting) -> Profile {
    let Acting::Accounts { accounts } = acting else { return Profile::default() };
    let [account] = accounts.as_slice() else { return Profile::default() };
    node.link_of(&account.hub)
        .and_then(|link| link.profile_read(&account.account).ok())
        .filter(|v| v.kept)
        .map(|v| v.profile)
        .unwrap_or_default()
}
