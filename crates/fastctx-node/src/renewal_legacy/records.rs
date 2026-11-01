//! New record projections are reported truthfully to older reply-shape readers.
use fastctx_types::{Reply, account_records::Response, plan};

pub(super) fn present(reply: &Reply) -> bool {
    match reply {
        Reply::Plan { result } => match result {
            plan::Result::Page { page } => {
                page.metadata.as_ref().is_some_and(|metadata| metadata.by.is_none())
                    || page.body.as_ref().is_some_and(|body| matches!(body.author, plan::Author::Elsewhere))
            }
            plan::Result::Conflict { current } => current.as_ref().is_some_and(|metadata| metadata.by.is_none()),
        },
        Reply::AccountRecords { response } => match response {
            Response::Listed { entries } => entries.iter().any(|metadata| metadata.by.is_none()),
            Response::Read { record } => record.as_ref().is_some_and(|record| record.metadata.by.is_none()),
            Response::Written { metadata } => metadata.by.is_none(),
            Response::Conflict { current } => current.as_ref().is_some_and(|metadata| metadata.by.is_none()),
        },
        _ => false,
    }
}
