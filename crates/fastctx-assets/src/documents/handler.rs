//! Description-only adapter. Referenced-body operations require the real bounded
//! semantic preparation port, not JSON containing an extra inline text field.
use super::{Kind, decode};
use crate::{Error, json};
use fastctx_sync::{
    catalog::ObjectHead,
    manifest::ContentRef,
    registry::handlers::{DeletePlan, ExportPlan, KindHandler, MergeBodies, MergeOutcome},
    store::{
        authorize::FieldChange,
        mutation::{FieldEdit, ProposedObject, TextEdit},
    },
};
use serde_json::Value;
pub struct Handler(pub Kind);
impl KindHandler for Handler {
    fn kind(&self) -> &'static str {
        self.0.id()
    }
    fn validate(&self, schema: u16, body: &[u8]) -> Result<(), Error> {
        json::schema(schema)?;
        if decode(body)?.kind()? != self.0 {
            return Err(json::invalid("document description belongs to another kind"));
        }
        Ok(())
    }
    fn references(&self, schema: u16, body: &[u8]) -> Result<Vec<ContentRef>, Error> {
        self.validate(schema, body)?;
        Ok(vec![decode(body)?.text.root])
    }
    fn differences(
        &self,
        schema: u16,
        before: &[u8],
        after: &[u8],
        max_bytes: usize,
    ) -> Result<Vec<FieldChange>, Error> {
        let parse = |body: &[u8]| -> Result<Value, Error> {
            if body.is_empty() {
                return Ok(Value::Object(Default::default()));
            }
            self.validate(schema, body)?;
            Ok(Value::Object(json::object(body)?))
        };
        json::schema(schema)?;
        json::differences(&parse(before)?, &parse(after)?, max_bytes)
    }
    fn import_legacy(&self, _: &str, _: &[u8]) -> Result<Vec<u8>, Error> {
        Err(unavailable())
    }
    fn project_legacy(&self, _: &ObjectHead, _: &[u8], _: u16) -> Result<Vec<u8>, Error> {
        Err(unavailable())
    }
    fn project_local(&self, _: &ObjectHead, body: &[u8]) -> Result<Vec<u8>, Error> {
        self.validate(1, body)?;
        Ok(body.to_vec())
    }
    fn merge(&self, _: &ProposedObject, _: &ObjectHead, _: MergeBodies<'_>) -> Result<MergeOutcome, Error> {
        Err(unavailable())
    }
    fn apply_edit(&self, _: u16, _: &[u8], _: &[TextEdit], _: usize) -> Result<Vec<u8>, Error> {
        Err(unavailable())
    }
    fn patch_fields(&self, _: u16, _: &[u8], _: &[FieldEdit]) -> Result<Vec<u8>, Error> {
        Err(unavailable())
    }
    fn export(&self, _: &ObjectHead) -> Result<ExportPlan, Error> {
        Ok(ExportPlan::GenericRootClosure)
    }
    fn delete(&self, _: &ObjectHead) -> Result<DeletePlan, Error> {
        Err(unavailable())
    }
}
fn unavailable() -> Error {
    Error::Unavailable("document operations require bounded referenced text, exact version mappings and the authorized lifecycle adapter".into())
}
