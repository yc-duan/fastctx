//! Scope feed pagination preserves canonical heads and explicitly covers hidden kinds.
use super::read::{Result, corrupt, invalid, size, unavailable};
use crate::{
    catalog::{CatalogChange, ChangesPage, FeedEntry, HeadEvidence},
    cursor::{HoldingPeriod, ScopeCursor, Sequence},
    scope::ScopeDescriptor,
    store::{directory, feed as stored, sql},
    transfer::{KindVersion, Message, ViewDefinition},
};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_types::sync::feed;

pub(super) fn page(
    db: &Connection,
    descriptor: &ScopeDescriptor,
    holding: HoldingPeriod,
    after: ScopeCursor,
    selected: Option<ViewDefinition>,
    max: usize,
) -> Result<Message> {
    let offered = directory::feed_view(descriptor);
    let view = selected.unwrap_or_else(|| offered.clone());
    if view.scope != after.scope
        || view.acl_revision != descriptor.acl_revision
        || view.projection_schema != 1
        || view.kinds.len() > offered.kinds.len()
        || view.kinds.iter().any(|kind| !offered.kinds.contains(kind))
    {
        return Err(invalid("the feed view must select offered kinds in the current scope and ACL"));
    }
    let view_id = view.id().map_err(invalid)?;
    let current = stored::cursor(db, &after.scope)?.ok_or_else(|| unavailable("the scope feed is absent"))?;
    let reset = || Message::Reset {
        scope: after.scope.clone(),
        epoch: current.epoch.clone(),
        reason: "the feed cursor needs reconciliation".into(),
    };
    if current.epoch != after.epoch
        || current.acl_revision != after.acl_revision
        || after.view.as_ref() != Some(&view_id)
        || after.prefix_digest.is_none()
        || after.through > current.through
    {
        let response = reset();
        if size(&response)? > max {
            return Err(invalid("the byte budget cannot hold a reconciliation response"));
        }
        return Ok(response);
    }
    let anchor = if after.through == Sequence(0) {
        Some(feed::genesis(&after.scope, &after.epoch))
    } else {
        db.prepare_cached(
            "SELECT prefix_digest FROM sync_changes WHERE hub=?1 AND scope=?2 AND epoch=?3 AND sequence=?4",
        )?
        .query_row(
            params![
                after.scope.hub.to_string(),
                after.scope.scope.as_str(),
                after.epoch.as_str(),
                after.through.0.to_be_bytes().as_slice()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(super::epoch::digest)
        .transpose()?
    };
    if anchor != after.prefix_digest {
        let response = reset();
        if size(&response)? > max {
            return Err(invalid("the byte budget cannot hold a reconciliation response"));
        }
        return Ok(response);
    }
    let mut prefix = anchor.ok_or_else(|| corrupt("validated feed cursor has no prefix"))?;
    let mut page = ChangesPage {
        after: after.clone(),
        through: after.clone(),
        entries: Vec::new(),
        proofs: Vec::new(),
        more: false,
    };
    let mut shell = page.clone();
    shell.through.through = Sequence(u64::MAX);
    // false is one JSON byte longer than true. Empty vectors reserve their brackets.
    let overhead = size(&Message::ChangesPage { holding: holding.clone(), page: shell })?;
    if overhead > max {
        return Err(invalid("the byte budget cannot hold a change page envelope"));
    }
    let mut used = overhead;
    let mut query = db.prepare_cached("SELECT sequence,CASE WHEN length(canonical_change)<=?5 THEN canonical_change END,prefix_digest FROM sync_changes WHERE hub=?1 AND scope=?2 AND epoch=?3 AND sequence>?4 ORDER BY sequence LIMIT 256")?;
    let mut rows = query.query(params![
        after.scope.hub.to_string(),
        after.scope.scope.as_str(),
        after.epoch.as_str(),
        after.through.0.to_be_bytes().as_slice(),
        fastctx_types::sync::PAGE_BYTES
    ])?;
    while let Some(row) = rows.next()? {
        let sequence = Sequence(sql::counter(row, 0)?);
        let canonical: Option<Vec<u8>> = row.get(1)?;
        let canonical = canonical.ok_or_else(|| unavailable("a stored change exceeds the metadata page budget"))?;
        let change: CatalogChange = serde_json::from_slice(&canonical)?;
        let digest = super::epoch::digest(row.get(2)?)?;
        if page.through.through.0.checked_add(1) != Some(sequence.0)
            || change.sequence != sequence
            || change.head.key.scope != after.scope
            || change.head.acl_revision > current.acl_revision
        {
            return Err(corrupt("stored change is not the next canonical scope entry"));
        }
        let (expected, next_prefix) = feed::advance(&prefix, &change).map_err(corrupt)?;
        if canonical != expected || digest != next_prefix {
            return Err(corrupt("stored feed prefix or canonical bytes differ"));
        }
        let visible = view.kinds.contains(&KindVersion { kind: change.head.kind.clone(), schema: change.head.schema });
        let (entry, replace) = if visible {
            if matches!(change.head.evidence, HeadEvidence::Accepted { .. }) {
                return Err(unavailable("accepted heads need the authority's acceptance-proof reader"));
            }
            (FeedEntry::Changed { change: Box::new(change), prefix_digest: digest.clone() }, false)
        } else {
            let (start, replace) = match page.entries.last() {
                Some(FeedEntry::Skipped { after, .. }) => (*after, true),
                _ => (page.through.through, false),
            };
            (
                FeedEntry::Skipped {
                    after: start,
                    through: sequence,
                    view: view_id.clone(),
                    prefix_digest: digest.clone(),
                },
                replace,
            )
        };
        let previous_size = if replace { size(page.entries.last().expect("skipped entry exists"))? } else { 0 };
        let needed = used - previous_size + size(&entry)? + usize::from(!replace && !page.entries.is_empty());
        if needed > max {
            if page.entries.is_empty() {
                return Err(invalid("the byte budget cannot hold the next change"));
            }
            break;
        }
        if replace {
            page.entries.pop();
        }
        page.entries.push(entry);
        used = needed;
        prefix = digest.clone();
        page.through.through = sequence;
        page.through.prefix_digest = Some(digest);
    }
    if page.through.through < current.through && page.entries.is_empty() {
        return Err(corrupt("scope feed contains a sequence gap"));
    }
    page.more = page.through.through < current.through;
    if !page.more && page.through.prefix_digest != current.prefix_digest {
        return Err(corrupt("scope feed head differs from its final change"));
    }
    Ok(Message::ChangesPage { holding, page })
}
