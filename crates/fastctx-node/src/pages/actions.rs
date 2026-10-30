//! The only page action entrance. The user's relay is checked here against
//! fresh publications and consent before any local or signed remote work,
//! and the machine the action is for checks it once more against its own
//! copy of the town's ledger before it does anything: what another machine
//! says a page is, or that the user agreed to, is not taken on its word.

use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::pages::{PageConsent, PageIdentity, PageInvocation, PageResult, PageSource};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::town::Looked;
use fastctx_types::{Address, MachineName, NoticeId, PageName, TownName};

use crate::Node;
use crate::speech::Owed;

pub async fn relay(
    node: &Arc<Node>,
    opened_on: &MachineName,
    identity: PageIdentity,
    town: TownName,
    page: PageName,
    version: u32,
    action: PageInvocation,
) -> Result<PageResult, Failure> {
    let source = admitted(node, identity, town, page, version, &action, opened_on.clone()).await?;
    let target = match &action {
        PageInvocation::Run { machine, .. } => machine.clone(),
        PageInvocation::Say { to, .. } => to.machine.clone(),
        PageInvocation::Journal { .. } => return execute(node, source, action).await,
    };
    let target = crate::town::machines::destination(node, &source.town, &target)?;
    if node.is_here(&target) {
        return execute(node, source, action).await;
    }
    // The remote request is issued only after this node's gate, signed as
    // this machine. Devices cannot submit this internal execution call.
    let acting = super::identity::acting(node, &source)?;
    match node
        .call(&acting, &target, Purpose::Look, RemoteCall::Facts)
        .await
        .map_err(|e| Failure::new(e.to_string()))??
    {
        RemoteReply::Facts(facts) if facts.page_accounts => {}
        _ => return Err(Failure::not_run(format!("upgrade {target} to execute pages for their consenting account"))),
    }
    let call = RemoteCall::PageAction { source, action };
    match node.call(&acting, &target, call.purpose(), call).await.map_err(|e| Failure::new(e.to_string()))?? {
        RemoteReply::PageAction { result } => Ok(result),
        _ => Err(Failure::new(format!("{target} answered something other than a page action"))),
    }
}

/// Checks, against this machine's copy of `town`'s ledger brought up to
/// date, that `version` is `page`'s current publication, not withdrawn,
/// that it declared `action` and that the user consented to it. Returns
/// what the action runs as, from that publication.
async fn admitted(
    node: &Arc<Node>,
    identity: PageIdentity,
    town: TownName,
    page: PageName,
    version: u32,
    action: &PageInvocation,
    opened_on: MachineName,
) -> Result<PageSource, Failure> {
    // Consent and publication changes affect subsequent admissions. A run
    // already admitted continues, and must not hold up revocation while
    // its normal fifteen-second wait is in progress.
    super::refresh(node, &town).await?;
    let _syncing = node.syncing.lock().await;
    let stored = super::stored(node, &town, &page)?;
    super::check_base(&page, Some(version), stored.current().map(|v| v.version))?;
    let publication = stored.current().expect("checked current version");
    if publication.withdrawn {
        return Err(Failure::new(format!("page {page} is withdrawn")));
    }
    let act = action.act();
    if !publication.acts.contains(&act) {
        return Err(Failure::new(format!("page {page} version {version} did not declare this action")));
    }
    let (consent, by) = stored
        .consents
        .get(&identity)
        .filter(|(consent, _)| consent.acts.contains(&act))
        .ok_or_else(|| Failure::refused(format!("this account has not consented to this action by page {page}")))?;
    consenter_may(node, &identity, by, consent, action, &town)?;
    let source = PageSource {
        identity: Some(identity),
        town,
        page,
        version,
        by: Box::new(publication.by.clone()),
        // A publication is written down with the machine it was published on.
        on: publication
            .on
            .clone()
            .ok_or_else(|| Failure::refused("the page's publication does not say where it was published"))?,
        opened_on,
    };
    super::identity::acting(node, &source)?;
    Ok(source)
}

/// Whether the member `by`, which wrote the user's consent, could do
/// `action` itself, for an account of its that may use `town`.
fn consenter_may(
    node: &Node,
    identity: &PageIdentity,
    by: &MachineName,
    consent: &PageConsent,
    action: &PageInvocation,
    town: &TownName,
) -> Result<(), Failure> {
    let PageIdentity::Account { hub, account } = identity else {
        // A consent is written down with the machine it was given on.
        return if node.is_me(by) && consent.stamp.on.as_ref().is_some_and(|on| node.is_me(on)) {
            Ok(())
        } else {
            Err(Failure::refused("local page consent was not given here"))
        };
    };
    let place = crate::ledger::names::place(&node.world, town)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| Failure::refused("the page's project is absent"))?;
    let link = node.link_of(hub).ok_or_else(|| Failure::refused("the page's hub is not connected"))?;
    let view = link.view();
    if place.hub != *hub
        || view.account(account).is_none_or(|account| account.locked)
        || !view.in_project(account, &place.town)
        || !view.may_act_for(account, by)
        || consent.stamp.on.as_ref().is_none_or(|on| !view.logged_in(on, account))
    {
        return Err(Failure::refused("the account's page consent is no longer authorized"));
    }
    let (target, purpose) = match action {
        PageInvocation::Run { machine, .. } => (machine, Purpose::Hands),
        PageInvocation::Say { to, .. } => (&to.machine, Purpose::Words),
        PageInvocation::Journal { .. } => return Ok(()),
    };
    view.admits(&view.me, account, target, purpose).map_err(Failure::refused)
}

/// A page action another machine relayed here: done once this machine has
/// admitted it itself.
pub async fn answer(
    node: &Arc<Node>,
    asker: &crate::admission::Asker,
    source: PageSource,
    action: PageInvocation,
) -> Result<PageResult, Failure> {
    let identity = source
        .identity
        .ok_or_else(|| Failure::refused("upgrade the sending Fastctx and consent to this page again"))?;
    if super::identity::account(&identity)
        != Some(fastctx_types::access::HubAccount { hub: asker.hub.clone(), account: asker.account.clone() })
    {
        return Err(Failure::refused("the page action's account differs from its sender"));
    }
    let source = admitted(node, identity, source.town, source.page, source.version, &action, source.opened_on).await?;
    execute(node, source, action).await
}

/// Executes a request admitted by this node. No browser or device reaches
/// this.
async fn execute(node: &Arc<Node>, source: PageSource, action: PageInvocation) -> Result<PageResult, Failure> {
    match action {
        PageInvocation::Run { machine, script } => {
            let target = crate::town::machines::destination(node, &source.town, &machine)?;
            if !node.is_here(&target) {
                return Err(Failure::new(format!("this command is for {machine}, not {}", node.machine())));
            }
            let town = Some(source.town.clone());
            let account = source.identity.as_ref().and_then(super::identity::account);
            let owner = fastctx_types::run::JobOwner::Page { page: source };
            let request = fastctx_types::run::RunRequest {
                turn: None,
                call_id: None,
                script,
                cwd: None,
                writes_off: false,
                wait: None,
                attachments: Vec::new(),
            };
            let (reply, _) = node.jobs.run(node, owner, town, request, account, false, None).await?;
            Ok(PageResult::Run(Box::new(reply)))
        }
        PageInvocation::Say { to, text } => {
            let target = crate::town::machines::destination(node, &source.town, &to.machine)?;
            if !node.is_here(&target) {
                return Err(Failure::new("the page's recipient lives on another machine"));
            }
            resident(node, &source.town, &to).await?;
            if let Some(account) = source.identity.as_ref().and_then(super::identity::account)
                && !node
                    .residents
                    .about(&to.resident)
                    .is_some_and(|(about, _)| node.serves(&about.acting, &account.hub, &account.account))
            {
                return Err(Failure::refused("the page's recipient does not work for its account"));
            }
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|e| Failure::new(e.to_string()))?;
            let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            let delivery = Delivery {
                id: NoticeId::try_from(format!("page-{suffix}")).expect("hex notice id"),
                at: jiff::Timestamp::now(),
                notice: Notice::Words {
                    from: Speaker::Page { page: Box::new(source) },
                    text,
                    goal_report: None,
                    goal_question: None,
                },
            };
            let landed = crate::speech::tell_here(node, &to.resident, delivery, Owed::Nobody).await?;
            Ok(PageResult::Said { to, landed })
        }
        PageInvocation::Journal { .. } => Err(Failure::not_run(fastctx_types::memory::LEGACY)),
    }
}

pub(super) async fn resident(node: &Arc<Node>, town: &TownName, to: &Address) -> Result<(), Failure> {
    let target = crate::town::machines::destination(node, town, &to.machine)?;
    let lives = if node.is_here(&target) {
        node.residents.active(&to.resident)?;
        node.residents.town_of(&to.resident).flatten().as_ref() == Some(town)
    } else {
        let call = RemoteCall::Look { id: to.resident.to_string() };
        match node
            .call(&Acting::Here, &target, Purpose::Look, call)
            .await
            .map_err(|e| Failure::new(e.to_string()))??
        {
            RemoteReply::Looked { looked: Some(Looked::Resident(detail)), .. } => {
                detail.resident.removed.is_none() && detail.resident.town.as_ref() == Some(town)
            }
            RemoteReply::Looked { .. } => false,
            _ => return Err(Failure::new("the recipient's machine did not answer the resident lookup")),
        }
    };
    if lives { Ok(()) } else { Err(Failure::new(format!("{to} is not a resident of {town}"))) }
}
