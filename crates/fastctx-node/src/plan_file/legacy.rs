//! Retire plan files written on an assisting host by earlier nodes. An
//! unarchived file stays in place, disconnected from synchronization, until
//! its contents can be accounted for without changing the account's plan.

use crate::Node;
use fastctx_types::{Address, ResidentId, ipc::Failure};

pub(super) async fn retire(node: &Node, id: &ResidentId) -> Result<(), Failure> {
    let path = super::path(node, id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Failure::new(error.to_string())),
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| Failure::new(error.to_string()))?;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("the conversation is unavailable"))?;
    let scope = crate::plan::file_account(node, id, &about.acting)?
        .ok_or_else(|| Failure::new("the legacy plan has no unambiguous account"))?;
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::new("the plan's hub is unavailable"))?;
    let page = link.plan_read(&scope.account, id.clone()).await.map_err(Failure::new)?;
    let owner = Address { machine: about.machine, resident: id.clone() };
    if link.plan_of_another(&page, &owner).is_some() {
        return Err(Failure::new("legacy plan retained: its account record belongs to another conversation"));
    }
    let mut archived = page.body.as_ref().is_some_and(|body| body.markdown == text);
    if !archived && link.view().plan_copies {
        let answer = link
            .plan_copy(&scope.account, fastctx_types::plan_copies::Request::Contains { owner, markdown: text.into() })
            .await
            .map_err(Failure::new)?;
        archived = matches!(answer, fastctx_types::plan_copies::Reply::Contains { found: true });
    }
    if !archived {
        return Err(Failure::new("legacy plan retained: its contents are not confirmed in the hub's versions"));
    }
    // Recheck after the network read; never remove a file changed while the
    // hub was being consulted (2026-10-30, legacy assisted-plan migration).
    if std::fs::read(&path).map_err(|error| Failure::new(error.to_string()))? != bytes {
        return Err(Failure::new("legacy plan changed while checking its hub version; retained"));
    }
    std::fs::remove_file(&path).map_err(|error| Failure::new(error.to_string()))?;
    node.log.line(format_args!("retired the legacy plan file of {id}; its contents are on the hub"));
    Ok(())
}
