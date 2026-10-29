//! The receipts of the files each delivery brought, kept at the
//! conversation's home: the envelope a host keeps names its files by id
//! alone, and these find and show them (`fastctx_types::session::attachments`).
//! They are also every file the conversation was given, which follow it to
//! each machine it runs a script on (`follow`).

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use fastctx_types::attachments::Attachment;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Notice, Pending};
use fastctx_types::session::attachments::Receipts;
use fastctx_types::{NoticeId, ResidentId};

use crate::Node;

type Book = BTreeMap<NoticeId, Vec<Attachment>>;

fn path(directory: &ConversationDir) -> PathBuf {
    directory.attachments().join("delivered.json")
}

fn read(directory: &ConversationDir) -> io::Result<Book> {
    match std::fs::read(path(directory)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Book::new()),
        Err(error) => Err(error),
    }
}

/// Keeps the receipts of `deliveries`, each under its delivery, beside
/// those kept already. The caller holds the resident-storage transaction.
fn keep(
    directory: &ConversationDir,
    deliveries: impl IntoIterator<Item = (NoticeId, Vec<Attachment>)>,
) -> io::Result<()> {
    let deliveries: Vec<_> = deliveries.into_iter().filter(|(_, attachments)| !attachments.is_empty()).collect();
    if deliveries.is_empty() {
        return Ok(());
    }
    let mut book = read(directory)?;
    let mut changed = false;
    for (delivery, attachments) in deliveries {
        if book.get(&delivery) == Some(&attachments) {
            continue;
        }
        book.insert(delivery, attachments);
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    fastctx_platform::ensure_private_dir(&directory.attachments())?;
    fastctx_platform::publish(
        &path(directory),
        &serde_json::to_vec(&book).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}

/// Keeps what the user's words in `inputs` bring before they are handed to
/// the conversation's host, whose record will name the files by id alone.
pub(crate) fn record(directory: &ConversationDir, inputs: &[Pending]) -> io::Result<()> {
    let attached = inputs.iter().filter_map(|input| match &input.delivery.notice {
        Notice::Attached { message, .. } => Some((message.delivery.clone(), message.attachments.clone())),
        _ => None,
    });
    keep(directory, attached)
}

/// Keeps for a conversation forked from another the receipts of the
/// deliveries its history brings along.
pub(crate) fn inherit(
    directory: &ConversationDir,
    deliveries: impl IntoIterator<Item = (NoticeId, Vec<Attachment>)>,
) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    keep(directory, deliveries)
}

/// Every file the conversation was given, each once, in the order it came.
/// The book is replaced whole when it changes, so it is read as it stands.
pub(crate) fn all(directory: &ConversationDir) -> io::Result<Vec<Attachment>> {
    let mut files: Vec<Attachment> = Vec::new();
    for attachment in read(directory)?.into_values().flatten() {
        if !files.iter().any(|kept| kept.id == attachment.id) {
            files.push(attachment);
        }
    }
    Ok(files)
}

/// The receipts of one conversation, read once when first asked.
pub(crate) struct Kept {
    directory: ConversationDir,
    book: OnceLock<Book>,
}

/// The receipts of `resident`, which lives here.
pub(crate) fn of(node: &Node, resident: &ResidentId) -> Kept {
    Kept { directory: node.world.conversation(resident), book: OnceLock::new() }
}

impl Receipts for Kept {
    fn of(&self, delivery: &NoticeId) -> Vec<Attachment> {
        let book = self.book.get_or_init(|| {
            read(&self.directory).unwrap_or_else(|error| {
                eprintln!("reading {}: {error}", path(&self.directory).display());
                Book::new()
            })
        });
        book.get(delivery).cloned().unwrap_or_default()
    }
}
