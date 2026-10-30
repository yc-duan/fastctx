//! Upload placeholders and verified receipts belong to the same durable input.

use fastctx_types::attachments::{Attachment, UploadFile};
use fastctx_types::composer::{Target, UploadScope, Uploading};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UploadRecord {
    from: MachineName,
    scope: UploadScope,
    attachment: Attachment,
    outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Outcome {
    Uploading,
    Attached,
    Failed { why: String },
    Detached { why: String },
}

impl UploadRecord {
    pub(super) fn finished(&self) -> bool {
        !matches!(self.outcome, Outcome::Uploading)
    }
}

/// What became of an upload operation tried before.
pub enum Prior {
    /// None went under it.
    Fresh,
    /// It ended, and this was its answer.
    Ended(Box<fastctx_types::composer::UploadResult>),
    /// It broke while its file is still in the words: it goes on now, as
    /// this attachment, from what arrived of it (`world-network.md`,
    /// 断开不是结束).
    Again(Box<Attachment>, Guard),
}

pub async fn prior(
    node: &Arc<Node>,
    id: &ResidentId,
    from: &MachineName,
    scope: &UploadScope,
    file: &UploadFile,
) -> Result<Prior, Failure> {
    node.ready_for(id).await;
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    node.residents.active(id)?;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let state = cell.as_ref().expect("loaded");
    let Some(mut record) = receipts::upload(node, id, state, &scope.operation).await? else {
        return Ok(Prior::Fresh);
    };
    if record.scope != *scope || record.from != *from {
        return Err(failure("that upload operation ID was reused for another device, file or draft"));
    }
    if record.attachment.file != *file {
        return Err(failure(format!("{} changed since it was attached; attach it again", record.attachment.file.name)));
    }
    let updated = match &record.outcome {
        // The try before has not yet heard that its connection broke.
        Outcome::Uploading => {
            return Err(Failure::unanswered("composer: this upload is still arriving from an earlier try"));
        }
        Outcome::Failed { why } => {
            let why = why.clone();
            let mut next = cell.take().expect("loaded");
            if !mark(&mut next, scope, &record.attachment.id, None, &why) {
                return Err(Failure::new(why));
            }
            record.outcome = Outcome::Uploading;
            next.uploads.insert(scope.operation.clone(), record.clone());
            let guard = Guard { node: node.clone(), id: id.clone(), scope: scope.clone(), armed: true };
            storage::publish_then(node, id, &mut cell, next, clear_failure(node, id, &scope.operation)).await?;
            return Ok(Prior::Again(Box::new(record.attachment), guard));
        }
        Outcome::Attached => Ok(Updated { composer: state.view(), landed: None, delivery: None }),
        Outcome::Detached { why } => Err(Failure::new(why.clone())),
    };
    crate::attachments::resolve(node, id, std::slice::from_ref(&record.attachment)).await?;
    Ok(Prior::Ended(Box::new(fastctx_types::composer::UploadResult { attachment: record.attachment, updated })))
}

/// The selected destination of the exact draft receiving an upload.
pub(crate) async fn files_on(
    node: &Arc<Node>,
    id: &ResidentId,
    scope: &UploadScope,
) -> Result<Option<fastctx_types::access::Written>, Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    Ok(cell.as_mut().expect("loaded").document(&scope.target, scope.epoch)?.files_on.clone())
}

pub async fn begin(
    node: &Arc<Node>,
    id: &ResidentId,
    from: &MachineName,
    scope: &UploadScope,
    attachment: &Attachment,
) -> Result<Guard, Failure> {
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    node.residents.active(id)?;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    if let Some(mut kept) = receipts::upload(node, id, &next, &scope.operation).await? {
        let mut expected = attachment.clone();
        expected.path = kept.attachment.path.clone();
        if kept.from != *from || kept.scope != *scope || kept.attachment != expected {
            return Err(failure("that upload operation belongs to another file or device"));
        }
        if let Outcome::Failed { why } = &kept.outcome {
            if !mark(&mut next, scope, &kept.attachment.id, None, why) {
                return Err(failure("this attachment was removed while its upload was interrupted"));
            }
            kept.outcome = Outcome::Uploading;
            next.uploads.insert(scope.operation.clone(), kept);
            let guard = Guard { node: node.clone(), id: id.clone(), scope: scope.clone(), armed: true };
            storage::publish_then(node, id, &mut cell, next, clear_failure(node, id, &scope.operation)).await?;
            return Ok(guard);
        }
        return Ok(Guard { node: node.clone(), id: id.clone(), scope: scope.clone(), armed: false });
    }
    if scope.operation.is_empty()
        || receipts::receipt(node, id, &next, &scope.operation).await?.is_some()
        || receipts::upload(node, id, &next, &scope.operation).await?.is_some()
    {
        return Err(failure("a fresh upload operation ID is required"));
    }
    next.document(&scope.target, scope.epoch)?.uploads.push(Uploading {
        id: attachment.id.clone(),
        file: attachment.file.clone(),
        error: None,
        operation: Some(scope.operation.clone()),
    });
    next.uploads.insert(
        scope.operation.clone(),
        UploadRecord {
            from: from.clone(),
            scope: scope.clone(),
            attachment: attachment.clone(),
            outcome: Outcome::Uploading,
        },
    );
    let guard = Guard { node: node.clone(), id: id.clone(), scope: scope.clone(), armed: true };
    storage::publish_then(node, id, &mut cell, next, clear_failure(node, id, &scope.operation)).await?;
    Ok(guard)
}

pub async fn complete(
    node: &Arc<Node>,
    id: &ResidentId,
    scope: &UploadScope,
    attachment: &Attachment,
) -> Result<Updated, Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    let record = receipts::upload(node, id, &next, &scope.operation)
        .await?
        .ok_or_else(|| failure("the upload has no shared-input record"))?;
    if matches!(record.outcome, Outcome::Attached) && record.attachment == *attachment && record.scope == *scope {
        return Ok(Updated { composer: next.view(), landed: None, delivery: None });
    }
    let mut expected = record.attachment.clone();
    if expected.path.is_empty() {
        expected.path = attachment.path.clone();
    }
    if expected != *attachment || record.scope != *scope {
        return Err(failure("the upload receipt changed"));
    }
    if let Outcome::Detached { why } = &record.outcome {
        return Err(Failure::new(why.clone()));
    }
    let applied = attach(&mut next, scope, attachment, record);
    publish(node, id, &mut cell, next).await?;
    let composer = cell.as_ref().expect("loaded").view();
    drop(cell);
    schedule(node, id);
    applied.map(|()| Updated { composer, landed: None, delivery: None })
}

fn attach(
    state: &mut Stored,
    scope: &UploadScope,
    attachment: &Attachment,
    record: UploadRecord,
) -> Result<(), Failure> {
    state.uploads.insert(scope.operation.clone(), UploadRecord { attachment: attachment.clone(), ..record });
    let applied = match state.document(&scope.target, scope.epoch) {
        Ok(document) if document.uploads.iter().any(|upload| upload.id == attachment.id) => {
            document.uploads.retain(|upload| upload.id != attachment.id);
            document.attachments.push(attachment.clone());
            Ok(())
        }
        _ => Err(failure(
            "this attachment was removed or its draft was sent while it uploaded; the verified file is retained",
        )),
    };
    state.uploads.get_mut(&scope.operation).expect("known upload").outcome = match &applied {
        Ok(()) => Outcome::Attached,
        Err(error) => Outcome::Detached { why: error.message.clone() },
    };
    applied
}

pub async fn failed(node: &Arc<Node>, id: &ResidentId, scope: &UploadScope, why: &str) -> Result<(), Failure> {
    node.composers
        .upload_failures
        .lock()
        .expect("composer upload failures")
        .entry(id.clone())
        .or_default()
        .insert(scope.operation.clone(), why.into());
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    let Some(record) = next.uploads.get(&scope.operation) else {
        return Ok(());
    };
    if !matches!(record.outcome, Outcome::Uploading) {
        clear_failure(node, id, &scope.operation)();
        return Ok(());
    }
    let attachment = record.attachment.id.clone();
    next.uploads.get_mut(&scope.operation).expect("known upload").outcome = Outcome::Failed { why: why.into() };
    mark(&mut next, scope, &attachment, Some(why), "");
    storage::publish_then(node, id, &mut cell, next, clear_failure(node, id, &scope.operation)).await
}

/// A disconnected writer remains known even if persisting its failure failed.
/// Its intent is not published as a committed document or discarded by a reload.
pub(super) fn interrupted(node: &Node, id: &ResidentId, operation: &str, record: &mut UploadRecord) {
    if matches!(record.outcome, Outcome::Uploading)
        && let Some(why) = node
            .composers
            .upload_failures
            .lock()
            .expect("composer upload failures")
            .get(id)
            .and_then(|failures| failures.get(operation))
    {
        record.outcome = Outcome::Failed { why: why.clone() };
    }
}

fn clear_failure(node: &Node, id: &ResidentId, operation: &str) -> impl FnOnce() + Send + 'static {
    let failures = node.composers.upload_failures.clone();
    let (id, operation) = (id.clone(), operation.to_string());
    move || {
        let mut failures = failures.lock().expect("composer upload failures");
        if let Some(records) = failures.get_mut(&id) {
            records.remove(&operation);
            if records.is_empty() {
                failures.remove(&id);
            }
        }
    }
}

/// Says `why` on the upload of `attachment` listed in the words of `scope`,
/// and on the queued message it is in; `None` takes back the failure `was`
/// of one that goes on again. Whether the words list it.
fn mark(
    state: &mut Stored,
    scope: &UploadScope,
    attachment: &fastctx_types::NoticeId,
    why: Option<&str>,
    was: &str,
) -> bool {
    let Ok(document) = state.document(&scope.target, scope.epoch) else {
        return false;
    };
    let Some(upload) = document.uploads.iter_mut().find(|upload| &upload.id == attachment) else {
        return false;
    };
    upload.error = why.map(Into::into);
    if let Target::Queued { id } = &scope.target
        && let Ok(entry) = state.entry(id)
        && (why.is_some() || entry.error.as_deref() == Some(was))
    {
        entry.error = why.map(Into::into);
    }
    true
}

/// Uploads the words listed in `before` and no longer list in `after` that
/// are not arriving now: what arrived of them is left for nothing.
pub(super) fn listed(state: &Stored) -> HashSet<fastctx_types::NoticeId> {
    state.documents().flat_map(|document| document.uploads.iter().map(|upload| upload.id.clone())).collect()
}

pub(super) fn forsaken(was: HashSet<fastctx_types::NoticeId>, after: &Stored) -> Vec<fastctx_types::NoticeId> {
    let is = listed(after);
    let arriving: std::collections::HashSet<_> = after
        .uploads
        .values()
        .filter(|record| matches!(record.outcome, Outcome::Uploading))
        .map(|record| &record.attachment.id)
        .collect();
    was.into_iter().filter(|upload| !is.contains(upload) && !arriving.contains(upload)).collect()
}

/// Settles only uploads left by the stopped node, while the first loader owns
/// the composer cell. A new upload cannot begin until this state commits.
pub(super) async fn recover(node: &Arc<Node>, id: &ResidentId, state: &mut Stored) -> Result<bool, Failure> {
    let records: Vec<_> =
        state.uploads.values().filter(|record| matches!(record.outcome, Outcome::Uploading)).cloned().collect();
    let changed = !records.is_empty();
    for record in records {
        match crate::attachments::resolve(node, id, std::slice::from_ref(&record.attachment)).await {
            Ok(()) => {
                let scope = record.scope.clone();
                let attachment = record.attachment.clone();
                // A verified file whose placeholder was removed stays retained
                // with a Detached receipt, exactly as for a live completion.
                let _ = attach(state, &scope, &attachment, record);
            }
            Err(error) => {
                let why = format!("the node restarted before this upload was verified: {}", error.message);
                mark(state, &record.scope, &record.attachment.id, Some(&why), "");
                state.uploads.get_mut(&record.scope.operation).expect("recovering upload").outcome =
                    Outcome::Failed { why };
            }
        }
    }
    Ok(changed)
}

pub struct Guard {
    node: Arc<Node>,
    id: ResidentId,
    scope: UploadScope,
    armed: bool,
}

impl Guard {
    /// A resumable upload now lives on another node; its receipt completes it.
    pub(crate) fn release(&mut self) {
        self.armed = false;
    }

    pub async fn finish(&mut self, attachment: &Attachment) -> Result<Updated, Failure> {
        let result = complete(&self.node, &self.id, &self.scope, attachment).await;
        if let Err(error) = &result
            && let Err(failure) = failed(&self.node, &self.id, &self.scope, &error.message).await
        {
            self.node.log.line(format_args!("recording attachment failure: {failure}"));
        }
        self.armed = false;
        result
    }

    pub async fn fail(&mut self, error: &Failure) {
        if let Err(failure) = failed(&self.node, &self.id, &self.scope, &error.message).await {
            self.node.log.line(format_args!("recording attachment failure: {failure}"));
        }
        self.armed = false;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (node, id, scope) = (self.node.clone(), self.id.clone(), self.scope.clone());
        tokio::spawn(async move {
            if let Err(error) =
                failed(&node, &id, &scope, "the upload connection ended before a verified file was completed").await
            {
                node.log.line(format_args!("recording abandoned attachment: {error}"));
            }
        });
    }
}
