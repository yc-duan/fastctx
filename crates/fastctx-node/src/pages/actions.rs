//! The only page action entrance. The user's relay is checked here against
//! fresh publications and consent before any local or signed remote work,
//! and the machine the action is for checks it once more against its own
//! copy of the town's ledger before it does anything: what another machine
//! says a page is, or that the user agreed to, is not taken on its word.

use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::pages::{PageInvocation, PageResult, PageSource};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::town::Looked;
use fastctx_types::{Address, JournalCall, MachineName, NoticeId, PageName, Reply, TownName};

use crate::Node;
use crate::speech::Owed;
use crate::town::caller::Caller;

pub async fn relay(
    node: &Arc<Node>,
    opened_on: &MachineName,
    town: TownName,
    page: PageName,
    version: u32,
    action: PageInvocation,
) -> Result<PageResult, Failure> {
    let source = admitted(node, town, page, version, &action, opened_on.clone()).await?;
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
    let call = RemoteCall::PageAction { source, action };
    match node.call(&Acting::Here, &target, Purpose::Hands, call).await.map_err(|e| Failure::new(e.to_string()))?? {
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
    town: TownName,
    page: PageName,
    version: u32,
    action: &PageInvocation,
    opened_on: MachineName,
) -> Result<PageSource, Failure> {
    // Consent and publication changes affect subsequent admissions. A run
    // already admitted continues, and must not hold up revocation while
    // its normal fifteen-second wait is in progress.
    let _writing = node.writing.lock().await;
    super::refresh(node, &town).await?;
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
    if !stored.consent.as_ref().is_some_and(|consent| consent.acts.contains(&act)) {
        return Err(Failure::new(format!("the user has not consented to this action by page {page}")));
    }
    // The user agrees on some machine, which writes it down for them; what
    // it wrote counts only as far as that machine could do it itself, as a
    // member that could only write in the project could write anything.
    let by = stored.consent_by.clone().ok_or_else(|| Failure::new(format!("page {page}'s consent has no writer")))?;
    consenter_may(node, &by, action, &town)?;
    Ok(PageSource { town, page, version, by: Box::new(publication.by.clone()), on: publication.on.clone(), opened_on })
}

/// Whether the member `by`, which wrote the user's consent, could do
/// `action` itself, for an account of its that may use `town`.
fn consenter_may(node: &Node, by: &MachineName, action: &PageInvocation, town: &TownName) -> Result<(), Failure> {
    let place = crate::ledger::names::place(&node.world, town)
        .map_err(|error| Failure::new(format!("reading {town}: {error}")))?
        .ok_or_else(|| Failure::new(format!("there is no project {town} on {}", node.machine())))?;
    // A project kept on no hub is this computer's own, consented to here.
    let Some(link) = node.link_of(&place.hub) else {
        return if node.is_me(by) {
            Ok(())
        } else {
            Err(Failure::new(format!("the consent was written by {by}, and {town} is kept on this computer alone")))
        };
    };
    let view = link.view();
    if view.member(by).is_none() {
        return Err(Failure::new(format!("the consent was written by {by}, which is not a member any more")));
    }
    let accounts: Vec<_> =
        view.acts_for(by).into_iter().filter(|account| view.in_project(account, &place.town)).collect();
    if accounts.is_empty() {
        return Err(Failure::new(format!("the consent was written by {by}, which works for no account of {town}")));
    }
    let (target, purpose) = match action {
        PageInvocation::Run { machine, .. } => (machine, Purpose::Hands),
        PageInvocation::Say { to, .. } => (&to.machine, Purpose::Words),
        PageInvocation::Journal { .. } => return Ok(()),
    };
    let refusals: Vec<String> =
        accounts.iter().filter_map(|account| view.admits(by, account, target, purpose).err()).collect();
    if refusals.len() < accounts.len() {
        return Ok(());
    }
    Err(Failure::new(format!(
        "the consent was written by {by}, and {by} may not do this itself: {}",
        refusals.join("; ")
    )))
}

/// A page action another machine relayed here: done once this machine has
/// admitted it itself.
pub async fn answer(node: &Arc<Node>, source: PageSource, action: PageInvocation) -> Result<PageResult, Failure> {
    let source = admitted(node, source.town, source.page, source.version, &action, source.opened_on).await?;
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
            let owner = fastctx_types::run::JobOwner::Page { page: source };
            let request =
                fastctx_types::run::RunRequest { call_id: None, script, cwd: None, writes_off: false, wait: None };
            let (reply, _) = node.jobs.run(node, owner, town, request, None).await?;
            Ok(PageResult::Run(Box::new(reply)))
        }
        PageInvocation::Say { to, text } => {
            let target = crate::town::machines::destination(node, &source.town, &to.machine)?;
            if !node.is_here(&target) {
                return Err(Failure::new("the page's recipient lives on another machine"));
            }
            resident(node, &source.town, &to).await?;
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|e| Failure::new(e.to_string()))?;
            let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            let delivery = Delivery {
                id: NoticeId::try_from(format!("page-{suffix}")).expect("hex notice id"),
                at: jiff::Timestamp::now(),
                notice: Notice::Words { from: Speaker::Page { page: Box::new(source) }, text },
            };
            let landed = crate::speech::tell_here(node, &to.resident, delivery, Owed::Nobody).await?;
            Ok(PageResult::Said { to, landed })
        }
        PageInvocation::Journal { journal, text } => {
            let town = source.town.clone();
            let caller = Caller::Page(Box::new(source));
            let call = JournalCall::Add {
                town: Some(town),
                journal,
                text,
                description: None,
                cites: Vec::new(),
                attach: Vec::new(),
                model: None,
            };
            let reply = crate::town::journal::call(node, &caller, call).await?;
            match reply {
                Reply::EntryAdded { journal, id, .. } => Ok(PageResult::EntryAdded { journal, id }),
                _ => unreachable!("journal add returns the written entry"),
            }
        }
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
