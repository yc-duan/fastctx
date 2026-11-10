use super::{api::*, state::Engine};
use crate::{conversation::Versioned, identity::ObjectId};
use fastctx_types::sync::conversation_timeline::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum JobKind {
    Wake { dependency: String },
    AppendWaiters { item: ObjectId },
    Children { parent: ObjectId },
    Anchors { anchor: ObjectId },
    Rebind { from: ObjectId, to: ObjectId },
    Aliases { target: ObjectId },
    Members { branch: ObjectId },
}
impl JobKind {
    pub fn class(&self) -> u8 {
        match self {
            Self::Wake { .. } | Self::AppendWaiters { .. } => 0,
            Self::Rebind { .. } => 1,
            Self::Children { .. } | Self::Anchors { .. } => 2,
            Self::Aliases { .. } => 3,
            Self::Members { .. } => 4,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Job {
    pub kind: JobKind,
    pub trigger: Evidence,
    pub phase: u8,
    pub after: Option<Vec<u8>>,
    pub version: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Relation {
    pub key: String,
    pub input: MergeInput,
}
fn decode<T: DeserializeOwned>(row: &crate::store::ingest_effects::SourceIndexRow) -> Result<T> {
    serde_json::from_slice(&row.value)
        .map_err(|e| MergeError::CorruptState { key: row.key.clone(), reason: e.to_string() })
}

impl Engine<'_> {
    pub async fn run_job(&mut self) -> Result<bool> {
        let Some(row) = self.first("work/", None).await? else {
            return Ok(false);
        };
        let key = self.rows.suffix(&row.key)?.to_owned();
        let mut job: Job = decode(&row)?;
        let prefix = match &job.kind {
            JobKind::Wake { dependency } => format!("pending/{dependency}/"),
            JobKind::AppendWaiters { item } => format!("pending/append/{}/", item.as_str()),
            JobKind::Children { parent } => format!("reverse/parent/{}/", parent.as_str()),
            JobKind::Anchors { anchor } => format!("reverse/anchor/{}/", anchor.as_str()),
            JobKind::Aliases { target } => format!("reverse/alias/{}/", target.as_str()),
            JobKind::Members { branch } => format!("branch/member/{}/", branch.as_str()),
            JobKind::Rebind { from, .. } => match job.phase {
                0 => format!("reverse/parent/{}/", from.as_str()),
                1 => format!("reverse/item-call/{}/", from.as_str()),
                2 => format!("reverse/call/{}/", from.as_str()),
                3 => format!("reverse/picture/{}/", from.as_str()),
                4 => format!("reverse/member/{}/", from.as_str()),
                5 => format!("pending/item/{}/", from.as_str()),
                6 => format!("pending/placement/{}/", from.as_str()),
                7 => format!("pending/parent/{}/", from.as_str()),
                8 => format!("pending/append/{}/", from.as_str()),
                9 => format!("reverse/anchor/{}/", from.as_str()),
                10 => format!("pending/anchor/{}/", from.as_str()),
                _ => {
                    self.delete(&key).await?;
                    return Ok(true);
                }
            },
        };
        let next = self.first(&prefix, job.after.as_deref()).await?;
        if let Some(relation) = &next {
            match &job.kind {
                JobKind::Wake { .. } | JobKind::AppendWaiters { .. } => {
                    let event: TimelineEvent = decode(relation)?;
                    let input = self.waiting_input(event).await?;
                    Box::pin(self.apply(&input, &job.trigger)).await?;
                }
                JobKind::Children { .. } | JobKind::Anchors { .. } => {
                    self.refresh(&decode::<ObjectId>(relation)?, &job.trigger).await?;
                }
                JobKind::Aliases { .. } => {
                    let from: ObjectId = decode(relation)?;
                    let alias: MergeInput = self.get(&format!("alias/{}", from.as_str())).await?.ok_or_else(|| {
                        MergeError::CorruptState {
                            key: relation.key.clone(),
                            reason: "alias relationship has no retained input".into(),
                        }
                    })?;
                    Box::pin(self.apply(&alias, &job.trigger)).await?;
                }
                JobKind::Members { .. } => {
                    let member: Versioned<BranchChange> = decode(relation)?;
                    let BranchChange::Membership { item, branch, .. } = member.value else {
                        return Err(MergeError::CorruptState {
                            key: relation.key.clone(),
                            reason: "membership index contains selection".into(),
                        });
                    };
                    let link: Relation = self
                        .get(&format!("reverse/member/{}/{}", item.as_str(), branch.as_str()))
                        .await?
                        .ok_or_else(|| MergeError::CorruptState {
                            key: relation.key.clone(),
                            reason: "membership has no retained relation".into(),
                        })?;
                    Box::pin(self.apply(&link.input, &job.trigger)).await?;
                }
                JobKind::Rebind { from, to } => {
                    let old_key = self.rows.suffix(&relation.key)?.to_owned();
                    match job.phase {
                        0 | 9 => {
                            let child: ObjectId = decode(relation)?;
                            self.refresh(&child, &job.trigger).await?;
                            self.delete(&old_key).await?;
                        }
                        1 => {
                            let call: ObjectId = decode(relation)?;
                            self.delete(&format!("reverse/call/{}/{}", call.as_str(), from.as_str())).await?;
                            self.put(&format!("reverse/call/{}/{}", call.as_str(), to.as_str()), to).await?;
                            self.put(&format!("reverse/item-call/{}/{}", to.as_str(), call.as_str()), &call).await?;
                            self.delete(&old_key).await?;
                        }
                        2 => {
                            let item: ObjectId = decode(relation)?;
                            self.put(&format!("reverse/call/{}/{}", to.as_str(), item.as_str()), &item).await?;
                            self.delete(&format!("reverse/item-call/{}/{}", item.as_str(), from.as_str())).await?;
                            self.put(&format!("reverse/item-call/{}/{}", item.as_str(), to.as_str()), to).await?;
                            self.delete(&old_key).await?;
                        }
                        3 | 4 => {
                            let link: Relation = decode(relation)?;
                            Box::pin(self.apply(&link.input, &job.trigger)).await?;
                            self.delete(&link.key).await?;
                            self.delete(&old_key).await?;
                        }
                        _ => {
                            let event: TimelineEvent = decode(relation)?;
                            let input = self.waiting_input(event).await?;
                            Box::pin(self.apply(&input, &job.trigger)).await?;
                        }
                    }
                }
            }
        }
        // A reduction may enqueue this same job with a new dependency revision.
        // Advancing an old cursor must not overwrite the new wake's start point.
        let latest: Option<Job> = self.get(&key).await?;
        if latest.as_ref().is_none_or(|latest| latest.version != job.version) {
            return Ok(true);
        }
        match next {
            Some(row) => {
                job.after = Some(row.key);
                self.put(&key, &job).await?;
            }
            None if matches!(job.kind, JobKind::Rebind { .. }) && job.phase < 10 => {
                job.phase += 1;
                job.after = None;
                self.put(&key, &job).await?;
            }
            None => {
                self.delete(&key).await?;
            }
        }
        Ok(true)
    }
}
