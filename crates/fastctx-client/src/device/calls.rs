//! The local World contract carried by a device: to a resident's machine,
//! asked of each machine (`around`), or, for the projects' ledgers, to one
//! machine that acts for the device's account and answers from its copy.

use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_types::accounts::{AccountChange, AccountsReply};
use fastctx_types::pages::PageCall;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::town::Looked;
use fastctx_types::{Address, Call, JournalCall, MachineName, ProjectCall, Reply, ResidentId};

use super::{Device, Session, failed};
use crate::Error;

/// A device runs no agents, so model collaboration is only its accounts'.
const NO_OWN_COLLAB: &str = "a device has no model collaboration of its own; name the account";

impl Device {
    pub(crate) async fn call(self: &Arc<Self>, call: Call) -> Result<(Reply, MachineName), Error> {
        // Nested transport futures otherwise share the caller's small stack.
        Box::pin(self.call_inner(call)).await
    }

    async fn call_inner(self: &Arc<Self>, mut call: Call) -> Result<(Reply, MachineName), Error> {
        if let Call::MachineUpdates { machine, update } = &call
            && matches!(
                update,
                fastctx_types::update::UpdateCall::App { .. }
                    | fastctx_types::update::UpdateCall::Fetch { .. }
                    | fastctx_types::update::UpdateCall::Progress { .. }
            )
        {
            return Err(failed(format!("only an interface on {machine} says what it is or brings packages there")));
        }
        let preferences = match &call {
            Call::MachineUpdates {
                machine,
                update: fastctx_types::update::UpdateCall::Preferences { auto, relays },
            } => Some((machine.clone(), *auto, *relays)),
            _ => None,
        };
        if let Some((machine, auto, relays)) = preferences {
            let running = self.sessions().iter().find_map(|session| {
                session.link.versions().and_then(|n| n.machines.get(&machine).map(|m| m.kernel.clone()))
            });
            if running.as_deref().and_then(fastctx_types::update::release)
                < fastctx_types::update::release(fastctx_types::update::ASKED_SINCE)
            {
                return Err(failed("this machine has not reported support for remote update settings"));
            }
            let (reply, _) = self
                .call(Call::MachineUpdates {
                    machine: machine.clone(),
                    update: fastctx_types::update::UpdateCall::State,
                })
                .await?;
            let Reply::Update(current) = reply else {
                return Err(failed("the machine did not report its update settings"));
            };
            let update = if current.preferences {
                fastctx_types::update::UpdateCall::Preferences { auto, relays }
            } else {
                fastctx_types::update::UpdateCall::Set { settings: current.settings.with_preferences(auto, relays) }
            };
            call = Call::MachineUpdates { machine, update };
        }
        let here = || self.state().name.unwrap_or_else(|| MachineName::try_from("device").expect("a valid name"));
        match call {
            Call::AttachmentThumbnail { attachment, max } => {
                let machine = attachment.thumbnail_owner().machine.clone();
                return Ok((Reply::Thumbnail(self.thumbnail_attachment(attachment, max).await?), machine));
            }
            Call::Login { hub, account, password, name, .. } => {
                let joined = self.login(hub, account, password, name).await?;
                let name = joined.name.clone();
                return Ok((Reply::LoggedIn { hub: joined.hub, name: joined.name, account: joined.account }, name));
            }
            Call::LoginWithToken { token, name } => {
                let joined = self.login_with_token(token, name).await?;
                let name = joined.name.clone();
                return Ok((Reply::LoggedIn { hub: joined.hub, name: joined.name, account: joined.account }, name));
            }
            Call::Account { hub, account, change: AccountChange::Logout } => {
                self.logout(hub, account).await?;
                return Ok((Reply::Changed { token: None }, here()));
            }
            Call::Account { hub, account, change: AccountChange::Seen { name } } => {
                let session = self.on_hub(hub.as_ref())?;
                let path = self.store.arrivals(&session.link.membership().id);
                fastctx_net::arrivals::shown(&path, &account, &name).map_err(failed)?;
                return Ok((Reply::Changed { token: None }, here()));
            }
            Call::Account { hub, account, change } => {
                let session = self.on_hub(hub.as_ref())?;
                let token = session.link.account_change(&account, change).await.map_err(failed)?;
                return Ok((Reply::Changed { token }, here()));
            }
            Call::Mcp { hub, account, mcp } => {
                let session = self.on_hub(hub.as_ref())?;
                let account = profile_account(&session, account)?;
                return Ok((Reply::Mcp { result: super::mcp::call(&session.link, &account, mcp).await? }, here()));
            }
            Call::ProfileRead { hub, account } => {
                let session = self.on_hub(hub.as_ref())?;
                let account = profile_account(&session, account)?;
                return Ok((Reply::Profile(Box::new(session.link.profile_read(&account).map_err(failed)?)), here()));
            }
            Call::ProfileWrite { hub, account, profile } => {
                let session = self.on_hub(hub.as_ref())?;
                let account = profile_account(&session, account)?;
                let current = session.link.profile_read(&account).map_err(failed)?;
                if profile.servers != current.profile.servers {
                    return Err(failed("change MCP servers through fastctx mcp"));
                }
                let version = session.link.profile_write(&account, profile).await.map_err(failed)?;
                return Ok((Reply::ProfileWritten { version }, here()));
            }
            Call::CollabRead { hub, account } => {
                let account = account.ok_or_else(|| failed(NO_OWN_COLLAB))?;
                let session = self.on_hub(hub.as_ref())?;
                return Ok((Reply::CollabRead(session.link.collab_read(&account).map_err(failed)?), here()));
            }
            Call::NetworkUpdates { hub, account, request } => {
                let session = self.on_hub(Some(&hub))?;
                let reply = session.link.network_updates(&account, request).await.map_err(failed)?;
                return Ok((Reply::NetworkUpdates(reply), here()));
            }
            Call::MachineLog { hub, account, before } => {
                let session = self.on_hub(Some(&hub))?;
                let page = session.link.machine_log(&account, before).await.map_err(failed)?;
                return Ok((Reply::MachineLog(page), here()));
            }
            Call::CollabWrite { hub, account, collab } => {
                let account = account.ok_or_else(|| failed(NO_OWN_COLLAB))?;
                let session = self.on_hub(hub.as_ref())?;
                let version = session.link.collab_write(&account, collab).await.map_err(failed)?;
                return Ok((Reply::CollabWritten { version }, here()));
            }
            Call::Forget { resident, machine } => {
                let how = self.forget_conversation(&resident, &machine).await?;
                let address = Address { machine: machine.clone(), resident };
                return Ok((Reply::Forgetting { resident: address, how }, machine));
            }
            Call::WorldNow => return Ok((Reply::WorldNow(Box::new(self.world_now().await?)), here())),
            Call::Features => return Ok((Reply::Features(self.features()?), here())),
            Call::SetFeature { feature, on } => return Ok((Reply::Features(self.set_feature(feature, on)?), here())),
            Call::SetLanguage { language } => return Ok((Reply::Features(self.set_language(&language)?), here())),
            Call::Accounts => {
                let mut hubs = Vec::new();
                for session in self.sessions() {
                    let mut accounts = session.link.accounts();
                    accounts.arrivals =
                        fastctx_net::arrivals::fresh(&self.store.arrivals(&accounts.hub)).map_err(failed)?;
                    hubs.push(accounts);
                }
                let dissolved = self.store.dissolutions().map_err(failed)?;
                return Ok((
                    Reply::Accounts(AccountsReply { machine: here(), was: Vec::new(), hubs, dissolved }),
                    here(),
                ));
            }
            Call::Update(call) => return Ok((Reply::Update(Box::new(self.update(call).await?)), here())),
            Call::SeenDissolved => {
                self.store.seen_dissolved().map_err(failed)?;
                super::changed(&self.shown_changes);
                return Ok((Reply::Done, here()));
            }
            _ => {}
        }
        // The account a conversation the user opens works for, when they
        // chose one.
        let mut chosen = None;
        let (machine, remote, resident) = match call {
            Call::Composer { resident, machine, request } => {
                let machine = self.find(&resident, machine).await?;
                (machine, RemoteCall::Composer { resident: resident.clone(), request }, Some(resident))
            }
            Call::FindFiles { machine, request } => {
                let machine =
                    machine.ok_or_else(|| failed("choose a machine for file search; a device has no local files"))?;
                (machine, RemoteCall::FindFiles { request }, None)
            }
            Call::Folders { machine, path } => {
                let machine = machine.ok_or_else(|| failed("choose a machine for its folders; a device has none"))?;
                (machine, RemoteCall::Folders { path }, None)
            }
            Call::Stats { machine, after } => {
                let machine =
                    machine.ok_or_else(|| failed("choose a machine; a device has no details page of its own"))?;
                (machine, RemoteCall::Stats { after }, None)
            }
            Call::UpdateMachine { machine } => (machine, RemoteCall::UpdateWorld, None),
            Call::MachineUpdates { machine, update } => (machine, RemoteCall::Updates { update }, None),
            Call::Thumbnail { machine, request } => {
                let machine = machine.ok_or_else(|| failed("choose the machine holding the image"))?;
                (machine, RemoteCall::Thumbnail { request }, None)
            }
            Call::Lines { machine, request } => {
                let machine = machine.ok_or_else(|| failed("choose the machine holding the text file"))?;
                (machine, RemoteCall::Lines { request }, None)
            }
            Call::ClaudeConsent { machine, accept, home } => {
                (machine, RemoteCall::ClaudeConsent { accept, home }, None)
            }
            Call::Homes { machine, change } => (machine, RemoteCall::Homes { change }, None),
            Call::ScanAgents { machine, stop } => (machine, RemoteCall::ScanAgents { stop }, None),
            Call::AddAgentLogin { machine, agent, dir } => (machine, RemoteCall::AddAgentLogin { agent, dir }, None),
            Call::AgentFolder { machine, dir, change } => (machine, RemoteCall::AgentFolder { dir, change }, None),
            Call::SetAgentTitle { machine, agent, dir, on } => {
                (machine, RemoteCall::SetAgentTitle { agent, dir, on }, None)
            }
            Call::PreviewAgentChange { machine, agent, dir, into, take_back } => {
                (machine, RemoteCall::PreviewAgentChange { agent, dir, into, take_back }, None)
            }
            Call::ChangeAgentLogin { machine, agent, dir, into, raise_output_limit, take_back } => {
                (machine, RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit, take_back }, None)
            }
            Call::AgentCalls { machine, agent, dir, query } => {
                (machine, RemoteCall::AgentCalls { agent, dir, query }, None)
            }
            Call::AgentUsage { machine, agent, dir } => (machine, RemoteCall::AgentUsage { agent, dir }, None),
            Call::ReadAgentLimits { machine, agent, dir } => {
                (machine, RemoteCall::ReadAgentLimits { agent, dir }, None)
            }
            Call::Talk { to, machine, text, attachments, files_on, id } => {
                let machine = self.find(&to, machine).await?;
                (machine, RemoteCall::Talk { to: to.clone(), text, attachments, files_on, id }, Some(to))
            }
            Call::Open { agent, town, node, text, model, reasoning_effort, attachments, choices } => {
                let machine =
                    node.ok_or_else(|| failed("choose a machine to open a conversation; no agents live on a device"))?;
                chosen = choices.account;
                let open = RemoteCall::Open {
                    mcp_off: choices.mcp_off.clone(),
                    agent,
                    town,
                    text,
                    model,
                    reasoning_effort,
                    attachments,
                    focus: choices.focus,
                    home: choices.home,
                    files_on: choices.files_on,
                };
                (machine, open, None)
            }
            Call::Stop { resident, machine } => {
                let machine = self.find(&resident, machine).await?;
                (machine, RemoteCall::Stop { resident: resident.clone() }, Some(resident))
            }
            // A command stops as the user asks, on the machine it runs on.
            Call::StopJob { job, machine, .. } => {
                let machine = machine.ok_or_else(|| failed("say which machine the command runs on"))?;
                (machine, RemoteCall::StopJob { job, by: fastctx_types::residents::STOPPED_BY_USER.to_string() }, None)
            }
            Call::Manage { resident, machine, action } => {
                let machine = self.find(&resident, machine).await?;
                (machine, RemoteCall::Manage { resident: resident.clone(), action }, Some(resident))
            }
            Call::Control { resident, machine, control } => {
                let machine = self.find(&resident, machine).await?;
                (machine, RemoteCall::Control { resident: resident.clone(), control }, Some(resident))
            }
            Call::Offers { resident, machine } => {
                let machine = self.find(&resident, machine).await?;
                (machine, RemoteCall::Offers { resident: resident.clone() }, Some(resident))
            }
            Call::ItemDetail { resident, machine, turn, item } => {
                let machine = self.find(&resident, machine).await?;
                (machine, RemoteCall::ItemDetail { resident: resident.clone(), turn, item }, Some(resident))
            }
            // What asks of the whole World, the device asks of each machine
            // itself (`around`).
            Call::Who { all, town, since } => return Ok((Reply::Who(self.who(all, town, since).await?), here())),
            Call::Since { since, town } => return Ok((Reply::Since(self.since(since, town).await?), here())),
            Call::Look { ids } => return Ok((Reply::Looked { looked: self.look(ids).await? }, here())),
            Call::Agents { machine } => return Ok((Reply::Agents(self.agents(machine).await?), here())),
            Call::AgentLogins { machine } => {
                return Ok((Reply::AgentLogins(self.agent_logins(machine).await?), here()));
            }
            Call::Nodes => return Ok((Reply::Nodes(self.nodes().await?), here())),
            // What only the projects' ledgers hold, one machine answers from
            // its own copy, for what this device's account may use.
            call @ (Call::Project(_)
            | Call::Journal(_)
            | Call::Page(
                PageCall::Read { .. }
                | PageCall::List { .. }
                | PageCall::History { .. }
                | PageCall::Consent { .. }
                | PageCall::Act { .. }
                | PageCall::Withdraw { .. }
                | PageCall::Restore { .. },
            )) => return self.relay(call).await,
            _ => return Err(failed("this call belongs to a local node, not a device")),
        };
        let session = self.reaching(&machine)?;
        let purpose = remote.purpose();
        let account = match chosen {
            Some(chosen) if chosen.hub != session.link.membership().id => {
                return Err(failed(format!("{machine} is not on the hub of the account {}", chosen.account)));
            }
            Some(chosen) => {
                let view = session.link.view();
                view.admits(&session.link.membership().name, &chosen.account, &machine, purpose).map_err(failed)?;
                chosen.account
            }
            None => session.account_for(&machine, purpose)?,
        };
        let needs_profile = match &remote {
            RemoteCall::Manage { action: fastctx_types::management::Management::Mcp { .. }, .. } => true,
            RemoteCall::Open { mcp_off, .. } => {
                let profile = session.link.profile_read(&account).map_err(failed)?.profile;
                !mcp_off.is_empty()
                    || !profile.prompt.is_empty()
                    || !profile.claude.is_empty()
                    || !profile.codex.is_empty()
                    || !profile.servers.is_empty()
            }
            _ => false,
        };
        if needs_profile {
            super::mcp::supports(&session.link, &account, &machine).await?;
        }
        let answer = if matches!(
            &remote,
            RemoteCall::Thumbnail { .. } | RemoteCall::AttachmentThumbnail { .. } | RemoteCall::Lines { .. }
        ) {
            fastctx_net::cancellable::call(&session.link, &machine, &account, remote.clone()).await
        } else {
            session.link.call(&machine, &account, purpose, remote.clone()).await
        }
        .map_err(|error| {
            crate::Error::from(crate::CallError::Failed(fastctx_types::ipc::Failure::unanswered(error.to_string())))
        })?
        .map_err(failed)?;
        let address = resident.map(|resident| Address { machine: machine.clone(), resident });
        let reply = match (remote, answer) {
            (RemoteCall::Composer { .. }, RemoteReply::Composer(updated)) => Reply::Composer(updated),
            (RemoteCall::FindFiles { .. }, RemoteReply::FilesFound(reply)) => Reply::FilesFound(reply),
            (RemoteCall::Folders { .. }, RemoteReply::Folders(folders)) => Reply::Folders(folders),
            (RemoteCall::Stats { .. }, RemoteReply::Stats(stats)) => Reply::Stats(stats),
            (RemoteCall::UpdateWorld | RemoteCall::Updates { .. }, RemoteReply::Update(state)) => Reply::Update(state),
            (RemoteCall::Thumbnail { .. }, RemoteReply::Thumbnail(reply)) => Reply::Thumbnail(reply),
            (RemoteCall::AttachmentThumbnail { .. }, RemoteReply::Thumbnail(reply)) => Reply::Thumbnail(reply),
            (RemoteCall::Lines { .. }, RemoteReply::Lines(reply)) => Reply::Lines(reply),
            (RemoteCall::Homes { .. }, RemoteReply::Set) => Reply::Done,
            (RemoteCall::AgentCalls { .. }, RemoteReply::AgentCalls(page)) => Reply::AgentCalls(page),
            (RemoteCall::AgentUsage { .. }, RemoteReply::AgentUsage(report)) => Reply::AgentUsage(report),
            (
                RemoteCall::ScanAgents { .. }
                | RemoteCall::AddAgentLogin { .. }
                | RemoteCall::AgentFolder { .. }
                | RemoteCall::SetAgentTitle { .. }
                | RemoteCall::ReadAgentLimits { .. },
                RemoteReply::AgentLogins(logins),
            ) => Reply::MachineAgentLogins(logins),
            (RemoteCall::PreviewAgentChange { .. }, RemoteReply::AgentChangePreview(preview)) => {
                Reply::AgentChangePreview(preview)
            }
            (RemoteCall::ChangeAgentLogin { .. }, RemoteReply::AgentLoginChanged(change)) => {
                Reply::AgentLoginChanged(change)
            }
            (RemoteCall::ClaudeConsent { .. }, RemoteReply::ClaudeConsent { accepted, changed }) => {
                Reply::ClaudeConsent { machine: machine.clone(), accepted, changed }
            }
            (RemoteCall::Talk { .. }, RemoteReply::Said { landed }) => {
                Reply::Spoken { to: address.expect("talk resident"), landed, delivery: None }
            }
            (RemoteCall::Open { .. }, RemoteReply::Opened { resident, landed }) => {
                Reply::Opened { resident: Address { machine: machine.clone(), resident }, landed }
            }
            (RemoteCall::Stop { .. }, RemoteReply::Stopped { stopped }) => {
                Reply::Stopped { resident: address.expect("stop resident"), stopped }
            }
            (RemoteCall::StopJob { job, .. }, RemoteReply::JobStopped { record }) => match record {
                Some(record) => Reply::JobStopped { machine: machine.clone(), record },
                None => return Err(failed(format!("{machine} has no job {job}"))),
            },
            (RemoteCall::Manage { .. }, RemoteReply::Managed(managed)) => {
                Reply::Managed { resident: address.expect("manage resident"), managed }
            }
            (RemoteCall::Control { .. }, RemoteReply::Controlled(controlled)) => {
                Reply::Controlled { resident: address.expect("control resident"), controlled }
            }
            (RemoteCall::Offers { .. }, RemoteReply::Offers(offers)) => {
                Reply::Offers { resident: address.expect("offers resident"), offers }
            }
            (RemoteCall::ItemDetail { .. }, RemoteReply::ItemDetail { detail }) => {
                Reply::ItemDetail { resident: address.expect("item detail resident"), detail }
            }
            _ => {
                return Err(failed(format!(
                    "{machine} answered something other than the requested conversation operation"
                )));
            }
        };
        Ok((reply, machine))
    }

    /// The machine the resident `resident` lives on: `machine` when given,
    /// otherwise the one machine whose answer says so.
    /// Asks the hub of the machine the conversation of `resident` lives on
    /// to delete it everywhere. A device keeps nothing to ask later: not
    /// connected, it says so.
    async fn forget_conversation(
        &self,
        resident: &ResidentId,
        machine: &MachineName,
    ) -> Result<fastctx_types::management::Forgetting, Error> {
        let session = self
            .sessions()
            .into_iter()
            .find(|session| session.link.view().member(machine).is_some())
            .ok_or_else(|| failed(format!("no hub this device is on has a machine named {machine}")))?;
        let account = session.link.account_of(machine, resident).map_err(failed)?;
        let conversation = Address { machine: machine.clone(), resident: resident.clone() };
        match session.link.forget(&account, &conversation).await.map_err(failed)? {
            true => Ok(fastctx_types::management::Forgetting::Asked),
            false => Err(failed("not connected to the hub now; nothing was deleted, try again once connected")),
        }
    }

    pub(super) async fn find(&self, resident: &ResidentId, machine: Option<MachineName>) -> Result<MachineName, Error> {
        if let Some(machine) = machine {
            return Ok(machine);
        }
        let looked = self.look(vec![resident.to_string()]).await?;
        let mut machines = std::collections::BTreeSet::new();
        let mut unasked = Vec::new();
        for item in looked {
            unasked.extend(item.unasked);
            for found in item.found {
                if let Looked::Resident(detail) = found
                    && detail.resident.about.id == *resident
                {
                    machines.insert(detail.resident.about.machine.clone());
                }
            }
        }
        if machines.len() == 1 {
            return Ok(machines.pop_first().expect("one machine"));
        }
        if machines.is_empty() {
            return Err(failed(format!(
                "no answering machine has resident {resident}; machines not asked: {}",
                unasked.join("; ")
            )));
        }
        Err(failed(format!(
            "resident {resident} lives on more than one machine ({}); choose one",
            machines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )))
    }

    /// Asks one online machine that may act for this device's account to
    /// make `call` as a node would for its own user.
    pub(super) async fn relay(&self, call: Call) -> Result<(Reply, MachineName), Error> {
        let repeatable = matches!(
            call,
            Call::Project(ProjectCall::List | ProjectCall::Show { .. } | ProjectCall::Map { .. })
                | Call::Journal(JournalCall::List { .. } | JournalCall::Read { .. })
                | Call::Page(PageCall::Read { .. } | PageCall::List { .. } | PageCall::History { .. })
        );
        let remote = RemoteCall::Relay { made: serde_json::to_value(&call).map_err(failed)? };
        let mut unanswered = Vec::new();
        let mut asked = false;
        for session in self.sessions() {
            for (machine, account) in actors(&session) {
                asked = true;
                // Heartbeats establish connection liveness, not a deadline
                // for completing a query or transferring its answer. Only a
                // reported delivery or connection failure permits trying
                // another machine.
                match session.link.call(&machine, &account, remote.purpose(), remote.clone()).await {
                    Ok(Ok(RemoteReply::Relayed { answer })) => {
                        return Ok((serde_json::from_value(answer).map_err(failed)?, machine));
                    }
                    Ok(Ok(_)) => return Err(failed(format!("{machine} answered something other than a relayed call"))),
                    Ok(Err(error)) => return Err(failed(format!("on {machine}: {error}"))),
                    Err(error @ CallFailure::NotConnected { .. }) => return Err(failed(error)),
                    Err(error @ (CallFailure::Unknown { .. } | CallFailure::Unreadable { .. })) if !repeatable => {
                        return Err(failed(format!("{error}; this write was not sent again")));
                    }
                    Err(error) => unanswered.push(error.to_string()),
                }
            }
        }
        if !asked {
            return Err(failed(
                "no machine that may act for this device's accounts is online; a device cannot read a project's ledger itself",
            ));
        }
        Err(failed(format!("no online machine answered: {}", unanswered.join("; "))))
    }
}

/// The online machines of a session's hub that may act for one of the
/// device's logins there, each with that account.
fn actors(session: &Session) -> Vec<(MachineName, fastctx_types::AccountName)> {
    let view = session.link.view();
    let logins: Vec<_> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
    view.machines()
        .filter(|machine| machine.online)
        .filter_map(|machine| {
            logins
                .iter()
                .find(|account| view.may_act_for(account, &machine.name) && view.rights(account, &machine.name).any())
                .map(|account| (machine.name.clone(), account.clone()))
        })
        .collect()
}

fn profile_account(
    session: &Session,
    account: Option<fastctx_types::AccountName>,
) -> Result<fastctx_types::AccountName, Error> {
    let view = session.link.view();
    let logins = view.me().map(|me| &me.logins).ok_or_else(|| failed("this device has no account"))?;
    match account {
        Some(account) if logins.contains(&account) => Ok(account),
        None if logins.len() == 1 => Ok(logins.iter().next().expect("one account").clone()),
        _ => Err(failed("choose an account this device is logged in to")),
    }
}
