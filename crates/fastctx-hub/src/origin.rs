//! A request retains its member identity across asynchronous work. Names can be
//! reused; authority belongs to the key that authenticated the original request.

use fastctx_net::{directory::Directory, sign::PublicKey};
use fastctx_types::MachineName;

#[derive(Clone)]
pub(crate) struct Origin {
    pub name: MachineName,
    pub key: PublicKey,
}
impl Origin {
    pub(crate) fn check(&self, directory: &Directory) -> Result<(), String> {
        if directory.member(&self.name).is_none_or(|member| member.key != self.key) {
            return Err("the requesting member was removed or replaced".into());
        }
        Ok(())
    }
}
impl std::ops::Deref for Origin {
    type Target = MachineName;
    fn deref(&self) -> &Self::Target {
        &self.name
    }
}
impl std::fmt::Display for Origin {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(out)
    }
}
