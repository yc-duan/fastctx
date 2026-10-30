//! The account's skills as the interfaces manage them (`fastctx_types::skills`):
//! each kept in a record of its own at the account's hub, with one index of
//! their names and sentences that sessions are told. A skill is written
//! first and the index after it, so a session never lists one it cannot
//! read; two devices changing them at once are kept apart by the records'
//! versions.

use fastctx_types::account_records::{Metadata, Request, Response};
use fastctx_types::skills::{self, Index, Listed, Skill};
use fastctx_types::{AccountName, HubName};

use crate::{Error, World};

/// How many times the index is written again when another device changed
/// it meanwhile.
const TRIES: usize = 5;

impl World {
    /// The names and sentences of the account's skills.
    pub async fn skills(&self, hub: Option<HubName>, account: Option<AccountName>) -> Result<Vec<Listed>, Error> {
        Ok(self.skill_index(hub, account).await?.0.skills)
    }

    /// One skill, whole; none by that name.
    pub async fn skill(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        name: String,
    ) -> Result<Option<Skill>, Error> {
        let (body, _) = self.record(hub, account, skills::KIND, name).await?;
        body.map(|body| serde_json::from_value(body).map_err(|error| Error::Other(error.to_string()))).transpose()
    }

    /// Keeps `skill`, in place of the one named `previous` where it was
    /// renamed or changed.
    pub async fn skill_save(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        previous: Option<String>,
        skill: Skill,
    ) -> Result<(), Error> {
        let skill = Skill { description: skill.description.trim().to_string(), ..skill };
        skill.validate().map_err(Error::Other)?;
        let (index, _) = self.skill_index(hub.clone(), account.clone()).await?;
        let renamed = previous.as_deref().is_some_and(|previous| previous != skill.name);
        let taken = index.skills.iter().any(|listed| listed.name == skill.name);
        if (previous.is_none() || renamed) && taken {
            return Err(Error::Other(format!("there is a skill named {} already", skill.name)));
        }
        if !taken && index.skills.len() >= skills::MOST {
            return Err(Error::Other(skills::words::TOO_MANY.into()));
        }
        let (_, current) = self.record(hub.clone(), account.clone(), skills::KIND, skill.name.clone()).await?;
        let body = serde_json::to_value(&skill).map_err(|error| Error::Other(error.to_string()))?;
        let write = Request::Write {
            kind: skills::KIND.into(),
            key: skill.name.clone(),
            expected: current.map(|m| m.version),
            body,
        };
        self.changed(hub.clone(), account.clone(), write).await?;
        let listed = Listed { name: skill.name.clone(), description: skill.description.clone() };
        self.change_index(hub.clone(), account.clone(), |skills| {
            match skills.iter_mut().find(|one| Some(&one.name) == previous.as_ref() || one.name == listed.name) {
                Some(one) => *one = listed.clone(),
                None => skills.push(listed.clone()),
            }
            skills.retain(|one| one.name == listed.name || Some(&one.name) != previous.as_ref());
        })
        .await?;
        if let Some(previous) = previous.filter(|_| renamed) {
            self.forget_skill(hub, account, previous).await?;
        }
        Ok(())
    }

    /// Lets go of the skill `name`: out of the index first, so no session is
    /// told of one it can no longer read.
    pub async fn skill_delete(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        name: String,
    ) -> Result<(), Error> {
        self.change_index(hub.clone(), account.clone(), |skills| skills.retain(|one| one.name != name)).await?;
        self.forget_skill(hub, account, name).await
    }

    async fn forget_skill(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        name: String,
    ) -> Result<(), Error> {
        let (body, current) = self.record(hub.clone(), account.clone(), skills::KIND, name.clone()).await?;
        if body.is_none() {
            return Ok(());
        }
        let delete = Request::Delete { kind: skills::KIND.into(), key: name, expected: current.map(|m| m.version) };
        self.changed(hub, account, delete).await
    }

    async fn skill_index(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
    ) -> Result<(Index, Option<Metadata>), Error> {
        let (body, metadata) = self.record(hub, account, skills::INDEX_KIND, skills::INDEX_KEY.into()).await?;
        let index = match body {
            Some(body) => serde_json::from_value(body).map_err(|error| Error::Other(error.to_string()))?,
            None => Index::default(),
        };
        Ok((index, metadata))
    }

    /// Changes the index as `change` says, again from what is there now each
    /// time another device changed it first.
    async fn change_index(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        change: impl Fn(&mut Vec<Listed>),
    ) -> Result<(), Error> {
        for _ in 0..TRIES {
            let (mut index, current) = self.skill_index(hub.clone(), account.clone()).await?;
            change(&mut index.skills);
            let body = serde_json::to_value(&index).map_err(|error| Error::Other(error.to_string()))?;
            let write = Request::Write {
                kind: skills::INDEX_KIND.into(),
                key: skills::INDEX_KEY.into(),
                expected: current.map(|m| m.version),
                body,
            };
            match self.account_records(hub.clone(), account.clone(), write).await? {
                Response::Written { .. } => return Ok(()),
                Response::Conflict { .. } => continue,
                other => return Err(Error::Unexpected(format!("{other:?}"))),
            }
        }
        Err(Error::Other(skills::words::BUSY.into()))
    }

    /// A record's body, none where it was never written or was deleted, and
    /// what it was last written as.
    async fn record(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        kind: &str,
        key: String,
    ) -> Result<(Option<serde_json::Value>, Option<Metadata>), Error> {
        match self.account_records(hub, account, Request::Read { kind: kind.into(), key }).await? {
            Response::Read { record: Some(record) } => {
                let body = (!record.metadata.deleted).then_some(record.body);
                Ok((body, Some(record.metadata)))
            }
            Response::Read { record: None } => Ok((None, None)),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Makes a change to a record, which another device changing it first
    /// turns away.
    async fn changed(&self, hub: Option<HubName>, account: Option<AccountName>, request: Request) -> Result<(), Error> {
        match self.account_records(hub, account, request).await? {
            Response::Written { .. } => Ok(()),
            Response::Conflict { .. } => Err(Error::Other(skills::words::BUSY.into())),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}

impl World {
    /// Folder reads carry their revision; edits must return that same revision.
    /// No working computer or conversation host is needed for these operations.
    pub async fn skill_files(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        request: fastctx_types::skill_files::Request,
    ) -> Result<fastctx_types::skill_files::Reply, Error> {
        match self.call(fastctx_types::Call::SkillFiles { hub, account, request }).await?.0 {
            fastctx_types::Reply::SkillFiles { result } => Ok(*result),
            reply => Err(Error::Unexpected(format!("{reply:?}"))),
        }
    }
}
