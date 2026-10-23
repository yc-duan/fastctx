//! Durable definitions with a replaceable transport to their selected home.
use crate::records::{Error, Records};
use fastctx_agent::FutureResult;
use fastctx_types::{Address, SentinelId, access::HubAccount, sentinel::Kind, sentinel_control::*};
pub const KIND: &str = "sentinel";

/// Home calls preserve the authenticated scope. `begin` is idempotent for the
/// same assignment; a timeout does not undo the already durable definition.
pub trait Homes: Send + Sync {
    fn begin<'a>(&'a self, registration: &'a Registration, version: u64) -> FutureResult<'a, Summary>;
    fn inspect<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, Summary>;
    fn withdraw<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, ()>;
}
pub async fn get(
    records: &dyn Records,
    scope: &HubAccount,
    id: &SentinelId,
) -> Result<Option<(u64, Registration)>, Error> {
    let record = tokio::time::timeout(std::time::Duration::from_secs(60), records.get(scope, KIND, id.as_str()))
        .await
        .map_err(|_| Error::Unavailable("reading sentinel timed out".into()))??;
    record
        .map(|record| {
            let registration: Registration = serde_json::from_value(record.value)
                .map_err(|e| Error::Unavailable(format!("invalid sentinel definition: {e}")))?;
            if registration.scope != *scope
                || registration.definition.account != scope.account
                || registration.definition.id != *id
            {
                return Err(Error::Unavailable("sentinel record identity mismatch".into()));
            }
            Ok((record.version, registration))
        })
        .transpose()
}
pub async fn create(records: &dyn Records, homes: &dyn Homes, registration: Registration) -> Result<Summary, Error> {
    let value = serde_json::to_value(&registration).map_err(|e| Error::Unavailable(e.to_string()))?;
    let record = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        records.compare_exchange(&registration.scope, KIND, registration.definition.id.as_str(), None, Some(value)),
    )
    .await
    .map_err(|_| Error::Unavailable("registering sentinel timed out; the outcome is unknown".into()))??
    .ok_or_else(|| Error::Unavailable("no registered sentinel returned".into()))?;
    match tokio::time::timeout(std::time::Duration::from_secs(180), homes.begin(&registration, record.version)).await {
        Ok(Ok(summary)) => Ok(summary),
        result => Ok(Summary {
            definition: registration.definition,
            phase: Phase::Starting,
            plan: String::new(),
            latest: None,
            reason: Some(match result {
                Ok(Err(error)) => error,
                _ => "registered; the first wake has not answered yet".into(),
            }),
        }),
    }
}
pub async fn list(
    records: &dyn Records,
    homes: &dyn Homes,
    scope: &HubAccount,
    owner: Option<&Address>,
) -> Result<Vec<Summary>, Error> {
    list_where(records, homes, scope, &|candidate| owner.is_none_or(|owner| owner == candidate)).await
}
/// Applies owner identity resolution before contacting homes. The node supplies
/// its directory's machine-alias comparison without changing account scope.
pub async fn list_where(
    records: &dyn Records,
    homes: &dyn Homes,
    scope: &HubAccount,
    include: &(dyn Fn(&Address) -> bool + Sync),
) -> Result<Vec<Summary>, Error> {
    tokio::time::timeout(std::time::Duration::from_secs(60), list_inner(records, homes, scope, include))
        .await
        .map_err(|_| Error::Unavailable("listing sentinels exceeded 60 seconds".into()))?
}
async fn list_inner(
    records: &dyn Records,
    homes: &dyn Homes,
    scope: &HubAccount,
    include: &(dyn Fn(&Address) -> bool + Sync),
) -> Result<Vec<Summary>, Error> {
    let heads = tokio::time::timeout(std::time::Duration::from_secs(60), records.list(scope, KIND))
        .await
        .map_err(|_| Error::Unavailable("listing sentinels timed out".into()))??;
    let mut result = Vec::new();
    for head in heads {
        let id = head.key.parse().map_err(|e: fastctx_types::NameError| Error::Unavailable(e.to_string()))?;
        let Some((_, registration)) = get(records, scope, &id).await? else { continue };
        if !registration.active || !include(&registration.definition.owner) {
            continue;
        }
        let summary = match tokio::time::timeout(std::time::Duration::from_secs(15), homes.inspect(&registration)).await
        {
            Ok(Ok(summary)) => summary,
            result => Summary {
                definition: registration.definition,
                phase: Phase::Unavailable,
                plan: String::new(),
                latest: None,
                reason: Some(match result {
                    Ok(Err(error)) => error,
                    _ => "the sentinel home did not answer".into(),
                }),
            },
        };
        result.push(summary);
    }
    Ok(result)
}
pub async fn stop(
    records: &dyn Records,
    homes: &dyn Homes,
    scope: &HubAccount,
    id: &SentinelId,
    owner: Option<&Address>,
) -> Result<Option<String>, Error> {
    let Some((version, registration)) = get(records, scope, id).await? else {
        return Err(Error::Unavailable("sentinel not found".into()));
    };
    if registration.definition.kind == Kind::Goal {
        return Err(Error::Unavailable("a goal sentinel is removed with its goal".into()));
    }
    if owner.is_some_and(|owner| *owner != registration.definition.owner) {
        return Err(Error::Unavailable("this conversation does not own the sentinel".into()));
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        records.compare_exchange(scope, KIND, id.as_str(), Some(version), None),
    )
    .await
    .map_err(|_| Error::Unavailable("withdrawing sentinel timed out; the outcome is unknown".into()))??;
    // The durable deletion is authoritative. Report separately whether the home
    // confirmed it; offline homes reconcile the deletion when they return.
    Ok(match tokio::time::timeout(std::time::Duration::from_secs(15), homes.withdraw(&registration)).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some("the sentinel home has not confirmed withdrawal yet".into()),
    })
}

/// An authoritative conversation deletion also withdraws its goal sentinel.
pub async fn delete_owner(
    records: &dyn Records,
    homes: &dyn Homes,
    scope: &HubAccount,
    owner: &Address,
) -> Result<Vec<String>, Error> {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let mut unconfirmed = Vec::new();
        for head in records.list(scope, KIND).await? {
            let id = head.key.parse().map_err(|e: fastctx_types::NameError| Error::Unavailable(e.to_string()))?;
            let Some((version, registration)) = get(records, scope, &id).await? else { continue };
            if registration.definition.owner != *owner {
                continue;
            }
            records.compare_exchange(scope, KIND, id.as_str(), Some(version), None).await?;
            match tokio::time::timeout(std::time::Duration::from_secs(15), homes.withdraw(&registration)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => unconfirmed.push(error),
                Err(_) => unconfirmed.push("home withdrawal acknowledgement timed out".into()),
            }
        }
        Ok(unconfirmed)
    })
    .await
    .map_err(|_| Error::Unavailable("withdrawing a deleted conversation's sentinels timed out".into()))?
}
