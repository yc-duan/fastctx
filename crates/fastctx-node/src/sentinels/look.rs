//! A sentinel's looks at the account's machines: the World's read-only
//! looks (`crate::inspect`), taken with the account's authority as it
//! stands now. A goal's judge may read the evidence the World keeps for the
//! conversation whose goal it judges.
use crate::Node;
use fastctx_agent::FutureResult;
use fastctx_sentinel::ports::{Look, ReadResult};
use fastctx_types::{
    MachineName,
    access::HubAccount,
    inspect::Inspect,
    sentinel::{Definition, Kind},
};
use std::sync::Arc;

pub(crate) struct MachineLook {
    pub node: Arc<Node>,
    pub scope: HubAccount,
}
impl Look for MachineLook {
    fn look<'a>(
        &'a self,
        definition: &'a Definition,
        machine: &'a MachineName,
        inspect: &'a Inspect,
    ) -> FutureResult<'a, ReadResult> {
        Box::pin(async move {
            if definition.kind == Kind::Web {
                return Err("machine looks are not available to this sentinel".into());
            }
            let evidence_of = (definition.kind == Kind::Goal).then(|| definition.owner.clone());
            let inspected = crate::inspect::look(&self.node, &self.scope, machine, inspect.clone(), evidence_of)
                .await
                .map_err(|failure| failure.message)?;
            Ok(ReadResult { text: inspected.text, full_path: None, note: inspected.note })
        })
    }
}
