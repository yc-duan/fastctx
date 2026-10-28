//! Resource subjects of calls. These identities never grant permission.

use crate::remote::RemoteCall;
use crate::{Address, MachineName};

impl RemoteCall {
    /// The conversation whose work is scheduled together. Remote residents
    /// include their home: two machines may choose the same resident id.
    /// Other operations share the caller account's machine bucket.
    pub fn scheduling_subject(&self, from: &MachineName, here: &MachineName) -> Option<String> {
        let addressed = |address: &Address| {
            if &address.machine == here { address.resident.to_string() } else { address.to_string() }
        };
        match self {
            Self::Composer { resident, .. }
            | Self::Plan { resident, .. }
            | Self::Collaboration { resident, .. }
            | Self::Goal { resident, .. }
            | Self::ConversationDebt { resident, .. }
            | Self::EnterProject { resident, .. }
            | Self::Find { resident }
            | Self::Watch { resident, .. }
            | Self::Stop { resident }
            | Self::Control { resident, .. }
            | Self::Manage { resident, .. }
            | Self::Offers { resident }
            | Self::ItemDetail { resident, .. }
            | Self::AgentActivity { resident, .. }
            | Self::Talk { to: resident, .. } => Some(resident.to_string()),
            Self::Run { resident, .. }
            | Self::WordsCame { resident }
            | Self::TurnInterrupted { resident, .. }
            | Self::Forget { resident, .. }
            | Self::Write { resident, .. }
            | Self::Place { resident, .. }
            | Self::Ask { asker: resident, .. } => Some(format!("{resident}@{from}")),
            Self::GoalCommand { actor, .. }
            | Self::Scratch { owner: actor }
            | Self::WaitingJobs { owner: actor }
            | Self::OnReceipt { owner: actor, .. }
            | Self::AttachmentReferences { conversation: actor, .. } => Some(addressed(actor)),
            Self::On { origin, .. }
            | Self::PipeOut { origin, .. }
            | Self::Follow { origin, .. }
            | Self::StopOn { origin, .. } => Some(addressed(&origin.resident)),
            Self::Upload { target, .. }
            | Self::ReserveAttachment { target, .. }
            | Self::AttachmentPlaces { target, .. } => match target.conversation_target() {
                crate::attachments::UploadTarget::Resident { resident }
                | crate::attachments::UploadTarget::Draft { resident, .. } => Some(resident.to_string()),
                _ => None,
            },
            Self::CommitAttachment { attachment, .. } => Some(addressed(attachment.conversation())),
            _ => None,
        }
    }
}
