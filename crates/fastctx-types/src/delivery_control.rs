//! Durable control facts that may be mailed to a node advertising control_letters.
//! These are narrowly typed, not a way to queue arbitrary remote execution.

use serde::{Deserialize, Serialize};

use crate::remote::RemoteCall;
use crate::{Address, JobId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlLetter {
    CompletionReceipt {
        owner: Address,
        delivery: crate::run::JobDelivery,
        origin: Option<crate::on::OnOrigin>,
        #[serde(default)]
        through_exit: bool,
    },
    StopRun {
        owner: Address,
        dispatch: crate::NoticeId,
    },
    StopOn {
        origin: crate::on::OnOrigin,
        job: Option<JobId>,
    },
}

impl ControlLetter {
    pub fn owner(&self) -> &Address {
        match self {
            Self::CompletionReceipt { owner, .. } | Self::StopRun { owner, .. } => owner,
            Self::StopOn { origin, .. } => &origin.resident,
        }
    }

    /// The same operation used by pre-mail peers and by admission checks.
    pub fn call(&self) -> RemoteCall {
        match self {
            Self::CompletionReceipt { owner, delivery, origin, through_exit } => RemoteCall::Forget {
                resident: owner.resident.clone(),
                town: origin.as_ref().and_then(|origin| origin.town.clone()),
                job: delivery.job.clone(),
                delivery: Some(delivery.clone()),
                origin: origin.clone(),
                through_exit: *through_exit,
            },
            Self::StopRun { owner, dispatch } => {
                RemoteCall::TurnInterrupted { resident: owner.resident.clone(), dispatch: Some(dispatch.clone()) }
            }
            Self::StopOn { origin, job } => RemoteCall::StopOn { origin: origin.clone(), job: job.clone() },
        }
    }
}
