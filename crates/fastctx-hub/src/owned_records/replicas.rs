//! A computer discovers all its current copies in one metadata-only request.

use crate::Hub;
use fastctx_net::sign::PublicKey;
use fastctx_types::{
    AccountName, MachineName,
    replicas::{Catalog, Document, Head, Scope},
};
use std::sync::Arc;

pub(super) async fn catalog(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
) -> Result<Catalog, String> {
    let (conversations, projects) = {
        let store = hub.store();
        let directory = &store.book.directory;
        let member = directory
            .member(from)
            .filter(|member| {
                member.key == *key
                    && !member.managed
                    && member.home.as_ref() == Some(account)
                    && member.logins.contains(account)
            })
            .ok_or("replicas belong only on the account's registered, logged-in computers")?;
        let conversations = hub
            .account_records
            .owners
            .lock()
            .expect("never poisoned")
            .candidates(directory, account)
            .into_iter()
            .filter(|conversation| {
                fastctx_net::record_ownership::keeps(
                    directory,
                    member,
                    std::slice::from_ref(&conversation.account),
                    conversation.shared_on.as_ref(),
                )
            })
            .filter_map(|conversation| {
                directory.account(&conversation.account).map(|owner| (conversation, owner.opened))
            })
            .collect::<Vec<_>>();
        let projects = directory
            .projects
            .iter()
            .filter(|project| directory.member_keeps_project(from, &project.town))
            .filter_map(|project| directory.account(&project.owner).map(|owner| (project.clone(), owner.opened)))
            .collect::<Vec<_>>();
        (conversations, projects)
    };
    let mut catalog = Catalog::default();
    for (conversation, opened) in conversations {
        let slot = hub.account_records.account(&conversation.account, opened);
        let records = slot.records.lock().await;
        let address = &conversation.address;
        crate::record_subscriptions::follow(
            hub,
            from,
            key,
            &conversation.account,
            fastctx_types::plan::KIND,
            address.resident.as_str(),
        )?;
        let plan = records.entries.get(&(fastctx_types::plan::KIND.into(), address.resident.to_string()));
        catalog.documents.push(Head {
            owner: conversation.account.clone(),
            opened,
            document: Document::Plan { owner: address.clone() },
            version: plan.map(|entry| entry.metadata.version),
            retired: plan.is_some_and(|entry| entry.metadata.deleted),
            history: None,
        });
        let scope = Scope::Conversation { conversation: conversation.clone() };
        for entry in records.entries.values().filter(|entry| {
            entry.metadata.kind == fastctx_types::history::RECORD_KIND
                && super::record_key(&conversation, &entry.metadata.kind, &entry.metadata.key).is_ok()
        }) {
            if let Some(revision) = &entry.history {
                catalog.documents.push(Head {
                    owner: conversation.account.clone(),
                    opened,
                    document: Document::History { scope: scope.clone(), name: revision.name.clone() },
                    version: Some(revision.version),
                    retired: revision.retired,
                    history: Some(revision.clone()),
                });
            }
        }
        catalog.scopes.push(scope);
    }
    {
        let ledgers = hub.ledgers.lock().expect("never poisoned");
        for (project, opened) in projects {
            let scope = Scope::Project { town: project.town.clone(), name: project.name.clone() };
            if let Some(ledger) = ledgers.towns.get(&project.town) {
                for revision in ledger.history.catalog().documents {
                    catalog.documents.push(Head {
                        owner: project.owner.clone(),
                        opened,
                        document: Document::History { scope: scope.clone(), name: revision.name.clone() },
                        version: Some(revision.version),
                        retired: revision.retired,
                        history: Some(revision),
                    });
                }
            }
            catalog.scopes.push(scope);
        }
    }
    // Grants can change while an account's storage gate is being acquired.
    let store = hub.store();
    let directory = &store.book.directory;
    let member = directory
        .member(from)
        .filter(|member| {
            member.key == *key
                && member.home.as_ref() == Some(account)
                && member.logins.contains(account)
                && !member.managed
        })
        .ok_or("the computer's replica ownership changed while its catalog was read")?;
    let keeps = |scope: &Scope| match scope {
        Scope::Project { town, .. } => directory.member_keeps_project(from, town),
        Scope::Conversation { conversation } => fastctx_net::record_ownership::keeps(
            directory,
            member,
            std::slice::from_ref(&conversation.account),
            conversation.shared_on.as_ref(),
        ),
    };
    catalog.scopes.retain(keeps);
    catalog.documents.retain(|head| {
        directory.account(&head.owner).is_some_and(|owner| owner.opened == head.opened)
            && match &head.document {
                Document::Plan { owner } => catalog.scopes.iter().any(
                    |scope| matches!(scope, Scope::Conversation { conversation } if &conversation.address == owner),
                ),
                Document::History { scope, .. } => keeps(scope),
            }
    });
    Ok(catalog)
}
