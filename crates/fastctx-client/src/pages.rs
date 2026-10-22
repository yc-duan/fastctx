//! The page API, shared by local interfaces and device clients.

use fastctx_types::pages::{
    PageAct, PageCall, PageConsent, PageDraft, PageInvocation, PageResult, PageSummary, PageVersion, PageView,
};
use fastctx_types::{Call, PageName, Reply, TownName};

use crate::{Error, World};

impl World {
    pub async fn publish_page(
        &self,
        town: Option<TownName>,
        page: PageName,
        base: Option<u32>,
        draft: PageDraft,
    ) -> Result<PageView, Error> {
        self.page_view(PageCall::Publish { town, page, base, draft }).await
    }

    pub async fn page(&self, town: Option<TownName>, page: PageName, version: Option<u32>) -> Result<PageView, Error> {
        self.page_as(town, page, version, None).await
    }

    pub async fn page_as(
        &self,
        town: Option<TownName>,
        page: PageName,
        version: Option<u32>,
        account: Option<fastctx_types::AccountName>,
    ) -> Result<PageView, Error> {
        self.require_page_accounts().await?;
        self.page_view(PageCall::Read { town, page, version, account }).await
    }

    pub async fn pages(&self, town: Option<TownName>) -> Result<Vec<PageSummary>, Error> {
        match self.call(Call::Page(PageCall::List { town })).await?.0 {
            Reply::Pages { pages, .. } => Ok(pages),
            other => Err(unexpected(other)),
        }
    }

    pub async fn page_history(&self, town: Option<TownName>, page: PageName) -> Result<Vec<PageVersion>, Error> {
        match self.call(Call::Page(PageCall::History { town, page })).await?.0 {
            Reply::PageHistory { versions, .. } => Ok(versions),
            other => Err(unexpected(other)),
        }
    }

    pub async fn withdraw_page(&self, town: Option<TownName>, page: PageName, base: u32) -> Result<PageView, Error> {
        self.page_view(PageCall::Withdraw { town, page, base }).await
    }

    pub async fn restore_page(
        &self,
        town: Option<TownName>,
        page: PageName,
        base: u32,
        version: u32,
    ) -> Result<PageView, Error> {
        self.page_view(PageCall::Restore { town, page, base, version }).await
    }

    /// Set the complete consent set. Empty `acts` revokes every action;
    /// `base` is the consent version returned by `page`.
    pub async fn consent_page(
        &self,
        town: Option<TownName>,
        page: PageName,
        version: u32,
        base: Option<u32>,
        acts: Vec<PageAct>,
    ) -> Result<PageConsent, Error> {
        self.consent_page_as(town, page, version, base, acts, None).await
    }

    pub async fn consent_page_as(
        &self,
        town: Option<TownName>,
        page: PageName,
        version: u32,
        base: Option<u32>,
        acts: Vec<PageAct>,
        account: Option<fastctx_types::AccountName>,
    ) -> Result<PageConsent, Error> {
        self.require_page_accounts().await?;
        match self.call(Call::Page(PageCall::Consent { town, page, version, base, acts, account })).await?.0 {
            Reply::PageConsent { consent, .. } => Ok(consent),
            other => Err(unexpected(other)),
        }
    }

    pub async fn act_page(
        &self,
        town: Option<TownName>,
        page: PageName,
        version: u32,
        action: PageInvocation,
    ) -> Result<PageResult, Error> {
        self.act_page_as(town, page, version, action, None).await
    }

    pub async fn act_page_as(
        &self,
        town: Option<TownName>,
        page: PageName,
        version: u32,
        action: PageInvocation,
        account: Option<fastctx_types::AccountName>,
    ) -> Result<PageResult, Error> {
        self.require_page_accounts().await?;
        match self.call(Call::Page(PageCall::Act { town, page, version, action, account })).await?.0 {
            Reply::PageAct { result } => Ok(result),
            other => Err(unexpected(other)),
        }
    }

    async fn require_page_accounts(&self) -> Result<(), Error> {
        if self.device().is_some() {
            return Ok(());
        }
        match self.call(Call::Nodes).await?.0 {
            Reply::Nodes(nodes) if nodes.facts.as_ref().is_some_and(|facts| facts.page_accounts) => Ok(()),
            _ => Err(Error::Unexpected("upgrade the local Fastctx to keep page consent by account".into())),
        }
    }

    async fn page_view(&self, call: PageCall) -> Result<PageView, Error> {
        match self.call(Call::Page(call)).await?.0 {
            Reply::Page { view } => Ok(*view),
            other => Err(unexpected(other)),
        }
    }
}

fn unexpected(reply: Reply) -> Error {
    Error::Unexpected(format!("{reply:?}"))
}
