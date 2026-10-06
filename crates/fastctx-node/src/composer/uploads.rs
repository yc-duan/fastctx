//! Upload placeholders and verified receipts belong to the same durable input.

use fastctx_types::attachments::{Attachment, UploadFile};
use fastctx_types::composer::{Target, UploadScope, Uploading};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadRecord {
    from: MachineName,
    scope: UploadScope,
    attachment: Attachment,
    outcome: Outcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Outcome {
    Uploading,
    Attached,
    Failed { why: String },
    Detached { why: String },
}

pub async fn cached(
    node: &Arc<Node>,
    id: &ResidentId,
    from: &MachineName,
    scope: &UploadScope,
    file: &UploadFile,
) -> Result<Option<fastctx_types::composer::UploadResult>, Failure> {
    node.ready().await;
    node.residents.active(id)?;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    let state = cell.as_ref().expect("loaded");
    let Some(record) = state.uploads.get(&scope.operation) else { return Ok(None) };
    if record.scope != *scope || record.from != *from || record.attachment.file != *file {
        return Err(failure("that upload operation ID was reused for another device, file or draft"));
    }
    match &record.outcome {
        Outcome::Uploading => Err(failure("this upload operation is already in progress")),
        outcome => {
            crate::attachments::resolve(node, id, std::slice::from_ref(&record.attachment)).await?;
            let updated = match outcome {
                Outcome::Attached => Ok(Updated { composer: state.view(), landed: None }),
                Outcome::Failed { why } | Outcome::Detached { why } => Err(Failure::new(why.clone())),
                Outcome::Uploading => unreachable!(),
            };
            Ok(Some(fastctx_types::composer::UploadResult { attachment: record.attachment.clone(), updated }))
        }
    }
}

pub async fn begin(
    node: &Arc<Node>,
    id: &ResidentId,
    from: &MachineName,
    scope: &UploadScope,
    attachment: &Attachment,
) -> Result<Guard, Failure> {
    let gate = node.residents.management(id)?;
    let _reading = gate.read().await;
    node.residents.active(id)?;
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    let mut next = cell.as_ref().expect("loaded").clone();
    if scope.operation.is_empty()
        || next.receipts.contains_key(&scope.operation)
        || next.uploads.contains_key(&scope.operation)
    {
        return Err(failure("a fresh upload operation ID is required"));
    }
    next.document(&scope.target, scope.epoch)?.uploads.push(Uploading {
        id: attachment.id.clone(),
        file: attachment.file.clone(),
        error: None,
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
    publish(node, id, &mut cell, next)?;
    Ok(Guard { node: node.clone(), id: id.clone(), scope: scope.clone(), armed: true })
}

pub async fn complete(
    node: &Arc<Node>,
    id: &ResidentId,
    scope: &UploadScope,
    attachment: &Attachment,
) -> Result<Updated, Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    let mut next = cell.as_ref().expect("loaded").clone();
    let record = next.uploads.get(&scope.operation).ok_or_else(|| failure("the upload has no shared-input record"))?;
    if record.attachment != *attachment || record.scope != *scope {
        return Err(failure("the upload receipt changed"));
    }
    let applied = match next.document(&scope.target, scope.epoch) {
        Ok(document) if document.uploads.iter().any(|upload| upload.id == attachment.id) => {
            document.uploads.retain(|upload| upload.id != attachment.id);
            document.attachments.push(attachment.clone());
            Ok(())
        }
        _ => Err(failure(
            "this attachment was removed or its draft was sent while it uploaded; the verified file is retained",
        )),
    };
    next.uploads.get_mut(&scope.operation).expect("known upload").outcome = match &applied {
        Ok(()) => Outcome::Attached,
        Err(error) => Outcome::Detached { why: error.message.clone() },
    };
    publish(node, id, &mut cell, next)?;
    let composer = cell.as_ref().expect("loaded").view();
    drop(cell);
    schedule(node, id);
    applied.map(|()| Updated { composer, landed: None })
}

pub async fn failed(node: &Arc<Node>, id: &ResidentId, scope: &UploadScope, why: &str) -> Result<(), Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    let mut next = cell.as_ref().expect("loaded").clone();
    let Some(record) = next.uploads.get(&scope.operation) else { return Ok(()) };
    if !matches!(record.outcome, Outcome::Uploading) {
        return Ok(());
    }
    let attachment = record.attachment.id.clone();
    next.uploads.get_mut(&scope.operation).expect("known upload").outcome = Outcome::Failed { why: why.into() };
    mark_failed(&mut next, scope, &attachment, why);
    if let Err(error) = publish(node, id, &mut cell, next) {
        if let Some(state) = cell.as_mut() {
            if let Some(record) = state.uploads.get_mut(&scope.operation) {
                record.outcome = Outcome::Failed { why: why.into() };
            }
            mark_failed(state, scope, &attachment, why);
        }
        return Err(error);
    }
    Ok(())
}

fn mark_failed(state: &mut Stored, scope: &UploadScope, attachment: &fastctx_types::NoticeId, why: &str) {
    if let Ok(document) = state.document(&scope.target, scope.epoch)
        && let Some(upload) = document.uploads.iter_mut().find(|upload| &upload.id == attachment)
    {
        upload.error = Some(why.into());
    }
    if let Target::Queued { id } = &scope.target
        && let Ok(entry) = state.entry(id)
    {
        entry.error = Some(why.into());
    }
}

pub async fn recover(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let cell = node.composers.cell(id);
    let mut cell = cell.lock().await;
    load(node, id, &mut cell)?;
    let records: Vec<_> = cell
        .as_ref()
        .expect("loaded")
        .uploads
        .values()
        .filter(|record| matches!(record.outcome, Outcome::Uploading))
        .cloned()
        .collect();
    drop(cell);
    for record in records {
        match crate::attachments::resolve(node, id, std::slice::from_ref(&record.attachment)).await {
            Ok(()) => {
                if let Err(error) = complete(node, id, &record.scope, &record.attachment).await {
                    let cell = node.composers.cell(id);
                    let cell = cell.lock().await;
                    if !cell
                        .as_ref()
                        .expect("loaded")
                        .uploads
                        .get(&record.scope.operation)
                        .is_some_and(|record| matches!(record.outcome, Outcome::Detached { .. }))
                    {
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                failed(
                    node,
                    id,
                    &record.scope,
                    &format!("the node restarted before this upload was verified: {}", error.message),
                )
                .await?;
            }
        }
    }
    Ok(())
}

pub struct Guard {
    node: Arc<Node>,
    id: ResidentId,
    scope: UploadScope,
    armed: bool,
}

impl Guard {
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
