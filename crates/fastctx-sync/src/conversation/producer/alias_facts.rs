use super::{api::*, state::Engine};
use crate::conversation::*;
use fastctx_types::sync::conversation_identity::AliasSource;
use fastctx_types::sync::conversation_timeline::*;

impl Engine<'_> {
    pub(super) async fn reduce_alias(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        binding: &fastctx_types::sync::conversation_identity::AliasBinding,
        receipt: &crate::catalog::RecordRef,
        mut causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let from = match binding.source() {
                AliasSource::Provisional(k) => k.id(),
                AliasSource::Legacy(k) => k.id(),
            };
            let to = binding.target().id();
            let mut missing = Vec::new();
            let source = self.item(&from).await?;
            let target = self.item(&to).await?;
            if source.is_none() {
                missing.push(Dependency::Item(from.clone()));
            }
            if target.is_none() {
                missing.push(Dependency::Item(to.clone()));
            }
            let (Some(source), Some(target)) = (source, target) else {
                return Ok(missing);
            };
            let mut selected = target.placement.clone();
            if let Some(placed) = &source.placement {
                self.reduced(evidence, crate::conversation::placement::choose(&mut selected, placed.clone()))?;
            }
            let neighbors = self.neighbors(selected.as_ref()).await?;
            causes.extend(neighbors.causes());
            causes.push(receipt.clone());
            causes.extend(source.causes());
            causes.extend(target.causes());
            let reduction = self.reduced(evidence, bind_alias(binding, &source, &target, neighbors.as_refs()))?;
            let alias_key = format!("alias/{}", from.as_str());
            if self.get::<MergeInput>(&alias_key).await?.is_none() {
                self.put(&alias_key, input).await?;
            }
            self.put(&format!("reverse/alias/{}/{}", to.as_str(), from.as_str()), &from).await?;
            Box::pin(self.save_item(Some(&source), &reduction.source, trigger)).await?;
            Box::pin(self.save_item(Some(&target), &reduction.target, trigger)).await?;
            if !reduction.source.visible {
                self.delete(&format!("reverse/alias/{}/{}", to.as_str(), from.as_str())).await?;
            }
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
}
