use fastctx_types::sync::identity::Digest;

/// A UUID match is only a candidate. Reuse requires the same complete row bytes;
/// a changed body with the same UUID is a revision of the same native item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowMatch {
    Unchanged,
    Revision,
    Different,
}
pub fn match_row(old_uuid: &str, new_uuid: &str, old: &Digest, new: &Digest) -> RowMatch {
    if old_uuid != new_uuid || old_uuid.is_empty() {
        RowMatch::Different
    } else if old == new {
        RowMatch::Unchanged
    } else {
        RowMatch::Revision
    }
}
