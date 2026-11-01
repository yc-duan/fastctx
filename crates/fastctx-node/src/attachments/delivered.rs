//! Attachment receipts are indexed by delivery identity. The files stay beside
//! the conversation; recording concurrent deliveries never replaces its book.

use crate::{Node, records};
use fastctx_types::attachments::Attachment;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Notice, Pending};
use fastctx_types::session::attachments::Receipts;
use fastctx_types::{NoticeId, ResidentId};
use std::{
    collections::{BTreeMap, HashSet},
    io,
    sync::Mutex,
};
const FAMILY: &str = "delivered-attachments";

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    records::legacy::map::<Vec<Attachment>>(directory, FAMILY, &directory.attachments().join("delivered.json"))
}
pub(crate) fn record_in(rows: &records::Rows<'_>, inputs: &[Pending]) -> fastctx_storage::Result<()> {
    for input in inputs {
        if let Notice::Attached { message, .. } = &input.delivery.notice
            && !message.attachments.is_empty()
        {
            rows.put(FAMILY, message.delivery.as_str(), &message.attachments)?;
        }
    }
    Ok(())
}
pub(crate) fn inherit(
    directory: &ConversationDir,
    deliveries: impl IntoIterator<Item = (NoticeId, Vec<Attachment>)>,
) -> io::Result<()> {
    prepare(directory)?;
    let mut group = Vec::new();
    for row in deliveries {
        if row.1.is_empty() {
            continue;
        }
        group.push(row);
        if group.len() == 16 {
            keep(directory, std::mem::take(&mut group))?;
        }
    }
    keep(directory, group)
}
fn keep(directory: &ConversationDir, group: Vec<(NoticeId, Vec<Attachment>)>) -> io::Result<()> {
    if group.is_empty() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&group)?.len();
    records::write_at(directory, fastctx_storage::Priority::Interactive, bytes, move |rows| {
        for (id, attachments) in group {
            rows.put(FAMILY, id.as_str(), &attachments)?;
        }
        Ok(())
    })
}
pub(crate) fn all(directory: &ConversationDir) -> io::Result<Vec<Attachment>> {
    prepare(directory)?;
    records::read(directory, |rows| {
        let mut files = Vec::new();
        let mut known = HashSet::new();
        let mut after = 0;
        loop {
            let page = rows.page::<Vec<Attachment>>(FAMILY, after, 128)?;
            let Some(last) = page.last() else { break };
            after = last.0;
            for attachment in page.into_iter().flat_map(|(_, files)| files) {
                if known.insert(attachment.id.clone()) {
                    files.push(attachment);
                }
            }
        }
        Ok(files)
    })
}
pub(crate) struct Kept {
    directory: ConversationDir,
    book: Mutex<BTreeMap<NoticeId, Vec<Attachment>>>,
}
pub(crate) fn of(node: &Node, resident: &ResidentId) -> Kept {
    Kept { directory: node.world.conversation(resident), book: Mutex::new(BTreeMap::new()) }
}
impl Receipts for Kept {
    fn of(&self, delivery: &NoticeId) -> Vec<Attachment> {
        if let Some(row) = self.book.lock().expect("attachment receipt cache").get(delivery).cloned() {
            return row;
        }
        let result = prepare(&self.directory)
            .and_then(|()| records::read(&self.directory, |rows| rows.get(FAMILY, delivery.as_str())));
        let row: Vec<Attachment> = match result {
            Ok(row) => row.unwrap_or_default(),
            Err(error) => {
                eprintln!("reading attachment receipts for {}: {error}", self.directory.path().display());
                return Vec::new();
            }
        };
        self.book.lock().expect("attachment receipt cache").insert(delivery.clone(), row.clone());
        row
    }
}
