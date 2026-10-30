//! Account-authorized plan history and three-way working-copy submissions.
//! No network operation is performed while an account's storage gate is held.

use fastctx_net::plan_merge as merge;

use crate::{
    Hub,
    account_records::{self, storage},
};
use fastctx_net::{sign::PublicKey, wire::Told};
use fastctx_types::{
    AccountName, Address, MachineName,
    account_records::{Record, Request as Records, Response},
    plan::{self, Author, Body, Page},
    plan_copies::{Reply, Request},
};
use jiff::Timestamp;
use std::sync::Arc;

fn page(owner: &Address, record: Option<Record>) -> Result<Page, String> {
    match record {
        None => Ok(Page { owner: owner.resident.clone(), metadata: None, body: None }),
        Some(record) => {
            let body = if record.metadata.deleted {
                None
            } else {
                Some(serde_json::from_value::<Body>(record.body).map_err(|error| error.to_string())?)
            };
            Ok(Page { owner: owner.resident.clone(), metadata: Some(record.metadata), body })
        }
    }
}

fn belongs(hub: &Hub, page: &Page, owner: &Address) -> Result<(), String> {
    if page.body.as_ref().and_then(Body::of).is_some_and(|old| {
        old.resident != owner.resident || !hub.store().book.directory.same(&old.machine, &owner.machine)
    }) {
        return Err("the plan belongs to another conversation with the same short name".into());
    }
    Ok(())
}

pub(crate) async fn ask(
    hub: &Arc<Hub>,
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
) -> Result<Told, String> {
    request.validate()?;
    if let Request::Merge { owner, base, markdown, submission: Some(id) } = &request
        && *id != fastctx_net::plan_merge::submission(owner, *base, markdown)
    {
        return Err("the submission identity does not describe this edit".into());
    }
    let owner = request.owner().clone();
    let slot = hub.account_records.account(&account, opened);
    for _ in 0..8 {
        let records = slot.records.lock().await;
        account_records::authorized(hub, &from, &key, &account, opened)?;
        crate::record_subscriptions::follow(hub, &from, &key, &account, plan::KIND, owner.resident.as_str())?;
        let current = slot.path(plan::KIND, owner.resident.as_str());
        let history = slot.history(plan::KIND, owner.resident.as_str());
        let exists = records.entries.contains_key(&(plan::KIND.into(), owner.resident.to_string()));
        let wanted = request.clone();
        let (current, historical, submitted) = tokio::task::spawn_blocking(move || {
            let current = if exists { Some(storage::read(&current)?) } else { None };
            if let Request::Merge { submission: Some(id), .. } = &wanted
                && let Some(submitted) = account_records::history::submitted(current.as_ref(), &history, id)?
            {
                return Ok((current, Vec::new(), Some(submitted)));
            }
            let version = match wanted {
                Request::Revision { version, .. } => Some(version),
                Request::Merge { base, .. } => base,
                Request::Contains { ref markdown, .. } => {
                    let mut found = Vec::new();
                    match std::fs::read_dir(history) {
                        Ok(entries) => {
                            for entry in entries {
                                let path = entry.map_err(|error| error.to_string())?.path();
                                if path.extension().is_some_and(|extension| extension == "json") {
                                    let record = storage::read(&path)?;
                                    if !record.metadata.deleted {
                                        let body: Body = serde_json::from_value(record.body.clone())
                                            .map_err(|error| error.to_string())?;
                                        if body.markdown == *markdown {
                                            found.push(record);
                                        }
                                    }
                                }
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.to_string()),
                    }
                    return Ok((current, found, None));
                }
            };
            let base = if let Some(version) = version {
                if current.as_ref().is_some_and(|record| record.metadata.version == version) {
                    current.clone().into_iter().collect()
                } else {
                    vec![storage::read(&history.join(format!("{version}.json")))?]
                }
            } else {
                Vec::new()
            };
            Ok::<_, String>((current, base, None))
        })
        .await
        .map_err(|error| error.to_string())??;
        account_records::authorized(hub, &from, &key, &account, opened)?;
        let current = page(&owner, current)?;
        belongs(hub, &current, &owner)?;
        if current.deleted() {
            return Err(plan::words::DELETED.into());
        }
        if let Some(submitted) = submitted {
            let submitted = page(&owner, Some(submitted))?;
            belongs(hub, &submitted, &owner)?;
            return Ok(Told::PlanCopies { reply: Reply::Page { page: Box::new(submitted) } });
        }
        let historical =
            historical.into_iter().map(|record| page(&owner, Some(record))).collect::<Result<Vec<_>, _>>()?;
        for revision in &historical {
            belongs(hub, revision, &owner)?;
        }
        match &request {
            Request::Revision { .. } => {
                return Ok(Told::PlanCopies {
                    reply: Reply::Page {
                        page: Box::new(historical.into_iter().next().ok_or("the base plan version is unavailable")?),
                    },
                });
            }
            Request::Contains { markdown, .. } => {
                return Ok(Told::PlanCopies {
                    reply: Reply::Contains {
                        found: std::iter::once(&current)
                            .chain(historical.iter())
                            .any(|page| page.body.as_ref().is_some_and(|body| body.markdown == *markdown)),
                    },
                });
            }
            Request::Merge { markdown, .. } => {
                let base =
                    historical.first().and_then(|page| page.body.as_ref()).map_or("", |body| body.markdown.as_str());
                let text = current.body.as_ref().map_or("", |body| body.markdown.as_str());
                let markdown = merge::merge(base, text, markdown);
                plan::validate(&markdown)?;
                if current.body.as_ref().is_some_and(|body| body.markdown == markdown) {
                    if let Request::Merge { submission: Some(id), .. } = &request {
                        let history = slot.history(plan::KIND, owner.resident.as_str());
                        let id = id.clone();
                        let record = Record {
                            metadata: current.metadata.clone().expect("a stored page has metadata"),
                            body: serde_json::to_value(&current.body).map_err(|error| error.to_string())?,
                        };
                        tokio::task::spawn_blocking(move || account_records::history::receipt(&history, &id, &record))
                            .await
                            .map_err(|error| error.to_string())?
                            .map_err(|error| error.to_string())?;
                    }
                    return Ok(Told::PlanCopies { reply: Reply::Page { page: Box::new(current) } });
                }
                let body = Body {
                    markdown,
                    author: Author::Resident { address: owner.clone() },
                    owner: Some(Box::new(owner.clone())),
                    submission: match &request {
                        Request::Merge { submission, .. } => submission.clone().map(Box::new),
                        _ => None,
                    },
                };
                let operation = Records::Write {
                    kind: plan::KIND.into(),
                    key: owner.resident.to_string(),
                    expected: current.version(),
                    body: serde_json::to_value(&body).map_err(|error| error.to_string())?,
                };
                drop(records);
                match account_records::ask(hub, from.clone(), key, account.clone(), opened, operation).await? {
                    Told::AccountRecords { response: Response::Written { metadata } } => {
                        return Ok(Told::PlanCopies {
                            reply: Reply::Page {
                                page: Box::new(Page {
                                    owner: owner.resident,
                                    metadata: Some(metadata),
                                    body: Some(body),
                                }),
                            },
                        });
                    }
                    Told::AccountRecords { response: Response::Conflict { .. } } => {}
                    _ => return Err("the plan write returned an unexpected answer".into()),
                }
            }
        }
    }
    Err("the plan keeps changing; this file has not been accepted yet".into())
}
