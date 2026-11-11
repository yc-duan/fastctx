//! Admission checks that do not need to change anything on the hub.

use super::super::Link;
use fastctx_types::accounts::AccountChange;

impl Link {
    /// Whether this hub understands the change. Its authoritative handler still
    /// checks permissions and business rules each time the change is executed.
    pub fn check_account_change(&self, change: &AccountChange) -> Result<(), String> {
        match change {
            // An older hub would not read the change, and would end the
            // connection that asked it.
            AccountChange::Direct { .. } if !self.view().direct_links => {
                return Err(format!(
                    "the hub {} runs an older fastctx that does not take direct connections",
                    self.hub_name()
                ));
            }
            AccountChange::Remove { then: Some(_), .. } if !self.view().uninstalls => {
                return Err(format!(
                    "the hub {} runs an older fastctx that has no machine it takes off uninstall Fastctx",
                    self.hub_name()
                ));
            }
            AccountChange::Rename { .. } if !self.view().renames => {
                return Err(format!("the hub {} runs an older fastctx that renames no machine", self.hub_name()));
            }
            AccountChange::Group { .. } | AccountChange::RenameGroup { .. } if !self.view().groups => {
                return Err(format!(
                    "the hub {} runs an older fastctx that keeps no groups of machines",
                    self.hub_name()
                ));
            }
            AccountChange::Token { managed, uses, label, .. }
                if (*managed || uses.is_some() || label.is_some()) && !self.view().managed =>
            {
                return Err(format!(
                    "the hub {} runs an older fastctx that makes no code for managed machines or several computers",
                    self.hub_name()
                ));
            }
            AccountChange::RevokeToken { .. } if !self.view().managed => {
                return Err(format!("the hub {} runs an older fastctx that takes no token back", self.hub_name()));
            }
            AccountChange::SetRole { .. } | AccountChange::AcceptRole { .. } if !self.view().roles => {
                return Err(format!("the hub {} runs an older fastctx that has no visitors", self.hub_name()));
            }
            AccountChange::OpenAccount { visitor: true, .. } if !self.view().roles => {
                return Err(format!("the hub {} runs an older fastctx that has no visitors", self.hub_name()));
            }
            AccountChange::Updates { .. } if !self.view().versions => {
                return Err(format!("the hub {} runs an older fastctx that keeps no versions", self.hub_name()));
            }
            AccountChange::Intake { .. } if !self.view().paced_intake => {
                return Err(format!(
                    "the hub {} runs an older fastctx that sets no ceiling on its intake",
                    self.hub_name()
                ));
            }
            AccountChange::UpdateDue { .. } if !self.view().due => {
                return Err(format!(
                    "the hub {} runs an older fastctx that keeps no updates for machines away",
                    self.hub_name()
                ));
            }
            AccountChange::UpdateDue { batch: Some(_), .. } if !self.view().package_progress => {
                return Err(format!(
                    "the hub {} must be updated before it can reserve a batch's packages",
                    self.hub_name()
                ));
            }
            AccountChange::Dissolve { .. } if !self.view().dissolves => {
                return Err(format!("the hub {} runs an older fastctx that dissolves no network", self.hub_name()));
            }
            AccountChange::DeleteProject { .. } if !self.view().deletes_project_conversations => {
                return Err(format!(
                    "the hub {} must be updated before deleting a project and its conversations",
                    self.hub_name()
                ));
            }
            AccountChange::Relay { .. } if !self.view().lan_relays => {
                return Err(format!("the hub {} runs an older fastctx that keeps no relays", self.hub_name()));
            }
            _ => {}
        }
        Ok(())
    }
}
