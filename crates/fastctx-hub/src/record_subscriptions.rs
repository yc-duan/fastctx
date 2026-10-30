//! Reader capability for complete skill-folder records.
use crate::Hub;
use fastctx_net::sign::PublicKey;
use fastctx_types::MachineName;

pub(crate) fn folder_reader(hub: &Hub, from: &MachineName, key: &PublicKey) -> bool {
    hub.sessions().get(from).is_some_and(|session| session.key == *key && session.skill_files)
}
