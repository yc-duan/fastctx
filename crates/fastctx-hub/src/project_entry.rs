//! A job's request to enter a project goes back to its resident's home,
//! without granting the executing machine general account authority.

use fastctx_types::{AccountName, Address, MachineName, ResidentId, TownName};

use crate::store::Store;

pub(crate) fn check(
    store: &Store,
    from: &MachineName,
    account: &AccountName,
    to: &MachineName,
    resident: &ResidentId,
    town: &TownName,
) -> Result<(), String> {
    let directory = &store.book.directory;
    if !directory.rights(account, from).hands {
        return Err(format!("the account {account} may not run the requesting job on {from}"));
    }
    if directory.project_named(account, town).is_none() {
        return Err(format!("the account {account} may not use the project {town}"));
    }
    let resident = Address { machine: to.clone(), resident: resident.clone() };
    if !store.worked.contains(&(from.clone(), resident)) {
        return Err("the requesting machine has not worked for this conversation".into());
    }
    Ok(())
}
