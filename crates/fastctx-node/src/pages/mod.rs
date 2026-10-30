//! Publications and user consent. Mutations compare the caller's base
//! against a fresh ledger; immutable paths arbitrate cross-machine races.

pub mod actions;
pub(crate) mod identity;
pub mod read;

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::pages::{
    PageAct, PageCall, PageConsent, PageDraft, PageVersion, account_consent_path, html_path, version_path,
};
use fastctx_types::project::{Stamp, Writer};
use fastctx_types::{MachineName, PageName, Reply, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::town::caller::Caller;

pub async fn call(
    node: &Arc<Node>,
    caller: &Caller,
    opened_on: &MachineName,
    call: PageCall,
) -> Result<Reply, Failure> {
    let mut reply = answer(node, caller, opened_on, call).await?;
    // Who published a page and who consented to it, and where, as whoever
    // asks may know them (`ledger::query::usable`).
    let acting = crate::around::acting_of(node, caller);
    let town = match &reply {
        Reply::Page { view } => Some(view.town.clone()),
        Reply::PageHistory { town, .. } | Reply::Pages { town, .. } => Some(town.clone()),
        _ => None,
    };
    let usable = match &town {
        Some(town) => crate::ledger::query::usable(node, &acting, town).map_err(io_failure)?,
        None => None,
    };
    if let Some(usable) = usable {
        match &mut reply {
            Reply::Page { view } => view.for_reader(&*usable),
            Reply::PageHistory { versions, .. } => versions.iter_mut().for_each(|version| version.for_reader(&*usable)),
            Reply::Pages { pages, .. } => pages.iter_mut().for_each(|summary| summary.current.for_reader(&*usable)),
            _ => {}
        }
    }
    Ok(reply)
}

async fn answer(node: &Arc<Node>, caller: &Caller, opened_on: &MachineName, call: PageCall) -> Result<Reply, Failure> {
    match call {
        PageCall::Publish { town, page, base, draft } => {
            publisher(caller)?;
            let town = caller.town_or(node, town)?;
            refresh(node, &town).await?;
            let stored = stored(node, &town, &page)?;
            check_base(&page, base, stored.current().map(|v| v.version))?;
            validate_acts(node, &town, &draft.acts).await?;
            publish(node, caller, &town, &page, base, draft, false, None).await
        }
        PageCall::Withdraw { town, page, base } => {
            publisher(caller)?;
            let town = caller.town_or(node, town)?;
            refresh(node, &town).await?;
            let stored = stored(node, &town, &page)?;
            check_base(&page, Some(base), stored.current().map(|v| v.version))?;
            let current = stored.view(&town, &page, None, None).map_err(Failure::new)?;
            let draft = PageDraft {
                title: current.publication.title,
                description: current.publication.description,
                acts: Vec::new(),
                html: String::new(),
            };
            publish(node, caller, &town, &page, Some(base), draft, true, None).await
        }
        PageCall::Restore { town, page, base, version } => {
            publisher(caller)?;
            let town = caller.town_or(node, town)?;
            refresh(node, &town).await?;
            let stored = stored(node, &town, &page)?;
            check_base(&page, Some(base), stored.current().map(|v| v.version))?;
            let source = stored.view(&town, &page, Some(version), None).map_err(Failure::new)?;
            let withdrawn = source.publication.withdrawn;
            let draft = PageDraft {
                title: source.publication.title,
                description: source.publication.description,
                acts: source.publication.acts,
                html: source.html.unwrap_or_default(),
            };
            publish(node, caller, &town, &page, Some(base), draft, withdrawn, Some(version)).await
        }
        PageCall::Read { town, page, version, account } => {
            let town = caller.town_or(node, town)?;
            read_ready(node, &town).await?;
            let identity = identity::resolve(node, caller, opened_on, &town, account.as_ref())?;
            Ok(Reply::Page {
                view: Box::new(
                    stored(node, &town, &page)?.view(&town, &page, version, Some(&identity)).map_err(Failure::new)?,
                ),
            })
        }
        PageCall::History { town, page } => {
            let town = caller.town_or(node, town)?;
            read_ready(node, &town).await?;
            let stored = stored(node, &town, &page)?;
            stored.view(&town, &page, None, None).map_err(Failure::new)?;
            Ok(Reply::PageHistory { town, page, versions: stored.versions.into_values().map(|(v, _)| v).collect() })
        }
        PageCall::List { town } => {
            let town = caller.town_or(node, town)?;
            read_ready(node, &town).await?;
            require_town(node, &town)?;
            let pages = read::summaries(&node.world, &town).map_err(io_failure)?;
            Ok(Reply::Pages { town, pages })
        }
        PageCall::Consent { town, page, version, base, acts, account } => {
            user(caller)?;
            let town = caller.town_or(node, town)?;
            refresh(node, &town).await?;
            let stored = stored(node, &town, &page)?;
            check_base(&page, Some(version), stored.current().map(|v| v.version))?;
            let current = stored.current().expect("checked current version");
            let identity = identity::resolve(node, caller, opened_on, &town, account.as_ref())?;
            let consent = stored.consents.get(&identity).map(|(consent, _)| consent);
            let current_consent = consent.map(|c| c.version);
            if base != current_consent {
                let current = current_consent.map_or_else(|| "none".to_string(), |v| v.to_string());
                return Err(Failure::new(format!(
                    "consent for {page} is at version {current}; read the page before changing consent"
                )));
            }
            if acts.iter().any(|act| !current.acts.contains(act) && !consent.is_some_and(|c| c.acts.contains(act))) {
                return Err(Failure::new("consent includes an action this page has not declared"));
            }
            let consent = PageConsent {
                identity: Some(identity.clone()),
                version: next(base)?,
                base,
                page_version: version,
                acts: unique(acts),
                stamp: Stamp { by: Writer::User, on: Some(opened_on.clone()), at: Timestamp::now() },
            };
            let files = vec![(account_consent_path(&page, &identity, consent.version), pretty(&consent)?)];
            identity::resolve(node, caller, opened_on, &town, account.as_ref())?;
            crate::ledger::write(node, &town, files, None).await.map_err(|failure| {
                Failure::new(format!(
                    "changing consent for {page}: {failure}; read the page before changing consent again"
                ))
            })?;
            Ok(Reply::PageConsent { town, page, consent })
        }
        PageCall::Act { town, page, version, action, account } => {
            user(caller)?;
            let town = caller.town_or(node, town)?;
            let identity = identity::resolve(node, caller, opened_on, &town, account.as_ref())?;
            actions::relay(node, opened_on, identity, town, page, version, action)
                .await
                .map(|result| Reply::PageAct { result })
        }
    }
}

pub(super) fn stored(node: &Node, town: &TownName, page: &PageName) -> Result<read::Page, Failure> {
    require_town(node, town)?;
    Ok(read::all(&node.world, town).map_err(io_failure)?.remove(page).unwrap_or_default())
}

fn require_town(node: &Node, town: &TownName) -> Result<(), Failure> {
    if crate::town::read::latest(&node.world, town).map_err(io_failure)?.is_none() {
        return Err(crate::town::map::no_ledger(node, town));
    }
    Ok(())
}

pub(super) async fn refresh(node: &Arc<Node>, town: &TownName) -> Result<(), Failure> {
    crate::town::map::query(node, town).await?;
    current(node, town, true).await
}

/// Online readers include consent just written on another device. When
/// disconnected, the already replicated publication remains readable.
async fn read_ready(node: &Arc<Node>, town: &TownName) -> Result<(), Failure> {
    crate::town::map::query(node, town).await?;
    current(node, town, false).await
}

/// Queries the same verified ledger path for every relay, whether the
/// project spans it or not. Reads may use an offline copy; an admission
/// cannot use possibly superseded consent or a withdrawn publication.
async fn current(node: &Arc<Node>, town: &TownName, required: bool) -> Result<(), Failure> {
    let place = crate::ledger::names::place(&node.world, town).map_err(io_failure)?;
    let Some((place, link)) = place.and_then(|place| node.link_of(&place.hub).map(|link| (place, link))) else {
        return require_town(node, town);
    };
    let offline = || {
        Failure::new(format!(
            "cannot verify the current page and consent for {town}: the hub connection ended; no action was admitted"
        ))
    };
    let Some(catalog) = link.try_ledger_catalog().await.map_err(Failure::new)? else {
        return if required { Err(offline()) } else { Ok(()) };
    };
    let latest = catalog
        .into_iter()
        .find(|(listed, _)| *listed == place.town)
        .map(|(_, seq)| seq)
        .ok_or_else(|| crate::town::map::no_ledger(node, town))?;
    crate::ledger::take(node, &link, town, &place.town, latest).await.map_err(Failure::new)?;
    require_town(node, town)
}

pub(super) fn check_base(page: &PageName, base: Option<u32>, current: Option<u32>) -> Result<(), Failure> {
    if base == current {
        return Ok(());
    }
    Err(Failure::new(match current {
        Some(current) => {
            format!("page {page} is at version {current}; read it first, then publish with --base {current}")
        }
        None => format!("page {page} has no version yet; read before publishing without a base"),
    }))
}

pub(super) fn user(caller: &Caller) -> Result<(), Failure> {
    if matches!(caller, Caller::User) {
        Ok(())
    } else {
        Err(Failure::new("only the user can change page consent or relay an open page's action"))
    }
}

fn publisher(caller: &Caller) -> Result<(), Failure> {
    if matches!(caller, Caller::Page(_)) { Err(Failure::new("pages cannot publish pages")) } else { Ok(()) }
}

fn next(base: Option<u32>) -> Result<u32, Failure> {
    base.unwrap_or(0).checked_add(1).ok_or_else(|| Failure::new("the version number is exhausted"))
}

fn unique(acts: Vec<PageAct>) -> Vec<PageAct> {
    let mut unique = Vec::new();
    for act in acts {
        if !unique.contains(&act) {
            unique.push(act);
        }
    }
    unique
}

async fn validate_acts(node: &Arc<Node>, town: &TownName, acts: &[PageAct]) -> Result<(), Failure> {
    for act in acts {
        match act {
            PageAct::Run { machine, script } => {
                if script.trim().is_empty() {
                    return Err(Failure::new("a page run needs its exact nonempty command"));
                }
                let target = crate::town::machines::destination(node, town, machine)?;
                if !node.is_here(&target)
                    && !node.locate(&target)?.iter().any(|link| link.view().machines().any(|m| m.named(machine)))
                {
                    return Err(Failure::new(format!("there is no machine named {machine}")));
                }
            }
            PageAct::Say { to } => actions::resident(node, town, to).await?,
            PageAct::Journal { .. } => return Err(Failure::not_run(fastctx_types::memory::LEGACY)),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn publish(
    node: &Arc<Node>,
    caller: &Caller,
    town: &TownName,
    page: &PageName,
    base: Option<u32>,
    draft: PageDraft,
    withdrawn: bool,
    restored_from: Option<u32>,
) -> Result<Reply, Failure> {
    let version = next(base)?;
    let publication = PageVersion {
        version,
        base,
        title: draft.title,
        description: draft.description,
        acts: unique(draft.acts),
        withdrawn,
        restored_from,
        by: caller.writer(),
        on: Some(node.machine()),
        at: Timestamp::now(),
    };
    let mut files = vec![(version_path(page, version), pretty(&publication)?)];
    let html = (!withdrawn).then_some(draft.html);
    let html_bytes = html.as_ref().map_or(0, String::len);
    if let Some(html) = &html {
        files.push((html_path(page, version), html.as_bytes().to_vec()));
    }
    if let Err(failure) = crate::ledger::write(node, town, files, None).await {
        if failure.message.contains("already") {
            refresh(node, town).await?;
            let current = stored(node, town, page)?.current().map(|v| v.version);
            check_base(page, base, current)?;
        }
        return Err(Failure::new(format!("publishing page {page} (HTML {html_bytes} bytes): {failure}")));
    }
    let consent = None;
    Ok(Reply::Page {
        view: Box::new(fastctx_types::pages::PageView {
            town: town.clone(),
            page: page.clone(),
            publication,
            html,
            consent,
        }),
    })
}

fn pretty(value: &impl serde::Serialize) -> Result<Vec<u8>, Failure> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| Failure::new(e.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn io_failure(error: std::io::Error) -> Failure {
    Failure::new(format!("reading the page ledger: {error}"))
}
