use super::{api::*, keys, state::Engine};
use crate::{
    catalog::RecordRef,
    conversation::Dependency,
    identity::{ObjectId, OperationId},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Waiting {
    pub dependencies: Vec<String>,
    pub causes: Vec<RecordRef>,
    pub ancestors: Vec<ObjectId>,
}
impl Engine<'_> {
    pub async fn clear_waiting(&mut self, operation: &OperationId) -> Result<()> {
        let key = format!("pending-input/{}", operation.as_str());
        if let Some(waiting) = self.get::<Waiting>(&key).await? {
            for dependency in waiting.dependencies {
                self.delete(&format!("pending/{dependency}/{}", operation.as_str())).await?;
            }
            self.delete(&key).await?;
        }
        Ok(())
    }
    pub async fn set_waiting(&mut self, input: &MergeInput, dependencies: Vec<Dependency>) -> Result<()> {
        self.clear_waiting(&input.event.evidence.operation).await?;
        let mut dependencies = dependencies.iter().map(keys::dependency).collect::<Vec<_>>();
        dependencies.sort();
        dependencies.dedup();
        if dependencies.is_empty() {
            return Ok(());
        }
        let operation = &input.event.evidence.operation;
        for dependency in &dependencies {
            self.put(&format!("pending/{dependency}/{}", operation.as_str()), &input.event).await?;
        }
        self.put(
            &format!("pending-input/{}", operation.as_str()),
            &Waiting { dependencies, causes: input.causes.clone(), ancestors: input.agent_ancestors.clone() },
        )
        .await?;
        Ok(())
    }
    pub async fn waiting_input(
        &mut self,
        event: fastctx_types::sync::conversation_timeline::TimelineEvent,
    ) -> Result<MergeInput> {
        let key = format!("pending-input/{}", event.evidence.operation.as_str());
        let waiting = self.get::<Waiting>(&key).await?.ok_or_else(|| MergeError::CorruptState {
            key: self.rows.key(&key),
            reason: "pending event has no input context".into(),
        })?;
        Ok(MergeInput { event, causes: waiting.causes, agent_ancestors: waiting.ancestors })
    }
}
