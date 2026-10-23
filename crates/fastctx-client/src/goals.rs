//! Goal controls and a scoped live watch, shared by desktop and node-less devices.

use crate::{Error, Watching, World};
use fastctx_types::access::{HubAccount, Written};
use fastctx_types::goals::{Request, Updated, View};
use fastctx_types::session::{Happened, SessionChange, Watched};
use fastctx_types::{Call, Reply, ResidentId};

impl World {
    pub async fn goal(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        account: Option<HubAccount>,
        request: Request,
    ) -> Result<Updated, Error> {
        match self.ask(Call::Goal { resident, machine, account, request }).await? {
            Reply::Goal { updated } => Ok(*updated),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn watch_goal(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        account: Option<HubAccount>,
    ) -> Result<(GoalWatch, View), Error> {
        let Updated::Goal { view } = self.goal(resident.clone(), machine.clone(), account, Request::Read).await? else {
            return Err(Error::Unexpected("a goal read returned a mutation conflict".into()));
        };
        let account = view.account;
        let watching = if let Some(device) = self.device() {
            Watching::device(device.watch(resident, machine, None, None, false, Some(account.clone())).await?)
        } else {
            self.connect()
                .await?
                .watch(Call::Watch {
                    resident,
                    machine,
                    composer: None,
                    held: None,
                    now: false,
                    goal: Some(account.clone()),
                })
                .await?
        };
        let mut watching = GoalWatch { watching, account, last: None };
        let initial = watching
            .next()
            .await?
            .ok_or_else(|| Error::Unexpected("the goal watch ended before its snapshot".into()))?;
        Ok((watching, initial))
    }
}

pub struct GoalWatch {
    watching: Watching,
    account: HubAccount,
    last: Option<View>,
}

impl GoalWatch {
    pub async fn next(&mut self) -> Result<Option<View>, Error> {
        while let Some(piece) = self.watching.next().await? {
            let Watched::Live { event } = piece else { continue };
            let Happened::Changed { change: SessionChange::Goal { goal }, .. } = event.happened else { continue };
            if goal["world"] != true {
                continue;
            }
            let view: View =
                serde_json::from_value(goal["view"].clone()).map_err(|error| Error::Unexpected(error.to_string()))?;
            if view.account != self.account {
                return Err(Error::Unexpected("the goal watch returned another account's record".into()));
            }
            if self.last.as_ref().is_some_and(|last| view.version < last.version || &view == last) {
                continue;
            }
            self.last = Some(view.clone());
            return Ok(Some(view));
        }
        Ok(None)
    }
}
