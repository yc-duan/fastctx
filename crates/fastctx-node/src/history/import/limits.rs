//! Limits shared by the two standalone-history importers.

pub(crate) use fastctx_types::history::MAX_DOCUMENTS as DOCUMENTS;

pub(crate) fn description(revision: &fastctx_types::history::HistoryRevision) -> Option<String> {
    let limit = fastctx_types::history::MAX_DESCRIPTION_BYTES;
    (revision.description.len() > limit)
        .then(|| format!("version {} has a description longer than {limit} bytes", revision.version))
}
