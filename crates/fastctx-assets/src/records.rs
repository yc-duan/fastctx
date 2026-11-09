//! Pure record schemas. Available schemas are not installed source authorizers,
//! reader policies, mutation services or proof that a kind is ready to advertise.
use crate::{
    Error,
    activity::Activity,
    device::Preferences,
    diagnostics::{alert::Alert, log::Log, mcp::McpLog},
    json,
    machine::{audit::Audit, connection::Connection, settings::Settings},
    read_position::Position,
};
use fastctx_sync::{
    catalog::ObjectHead,
    manifest::ContentRef,
    registry::handlers::{DeletePlan, ExportPlan, KindHandler, MergeBodies, MergeOutcome},
    store::{
        authorize::FieldChange,
        mutation::{FieldEdit, ProposedObject, TextEdit},
    },
};
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schema {
    Usage,
    Activity,
    ReadPosition,
    MachineSettings,
    Audit,
    Connection,
    Preferences,
    Alert,
    Log,
    McpLog,
}
impl Schema {
    pub fn validate(self, body: &[u8]) -> Result<(), Error> {
        match self {
            Self::Usage => crate::usage_shape::decode(body)?.validate(),
            Self::Activity => decode::<Activity>(body)?.validate(),
            Self::ReadPosition => decode::<Position>(body).map(|_| ()),
            Self::MachineSettings => decode::<Settings>(body)?.validate(),
            Self::Audit => decode::<Audit>(body)?.validate(),
            Self::Connection => decode::<Connection>(body).map(|_| ()),
            Self::Preferences => decode::<Preferences>(body)?.validate(),
            Self::Alert => decode::<Alert>(body).map(|_| ()),
            Self::Log => decode::<Log>(body)?.validate(),
            Self::McpLog => decode::<McpLog>(body)?.validate(),
        }
    }
    pub fn differences(self, before: &[u8], after: &[u8], max_bytes: usize) -> Result<Vec<FieldChange>, Error> {
        let parse = |body: &[u8]| {
            if body.is_empty() {
                return Ok(Value::Object(Default::default()));
            }
            self.validate(body)?;
            Ok::<_, Error>(Value::Object(json::object(body)?))
        };
        json::differences(&parse(before)?, &parse(after)?, max_bytes)
    }
    pub fn references(self, body: &[u8]) -> Result<Vec<ContentRef>, Error> {
        self.validate(body)?;
        Ok(match self {
            Self::Log => vec![decode::<Log>(body)?.content],
            Self::McpLog => vec![decode::<McpLog>(body)?.content],
            _ => Vec::new(),
        })
    }
}
pub(crate) fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    json::typed(Value::Object(json::object(body)?))
}

/// Only names already present in the foundation registry. Diagnostic log and
/// MachineWork schemas remain pure values until their registrations are provided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Usage,
    Activity,
    ReadPosition,
    MachineSettings,
    Audit,
    Connection,
    Preferences,
    Alert,
}
impl Kind {
    pub const ALL: [Self; 8] = [
        Self::Usage,
        Self::Activity,
        Self::ReadPosition,
        Self::MachineSettings,
        Self::Audit,
        Self::Connection,
        Self::Preferences,
        Self::Alert,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Usage => "account.usage",
            Self::Activity => "account.activity",
            Self::ReadPosition => "account.read-position",
            Self::MachineSettings => "machine.settings",
            Self::Audit => "machine.audit",
            Self::Connection => "machine.connection",
            Self::Preferences => "device.preferences",
            Self::Alert => "account.notice",
        }
    }
    pub fn schema(self) -> Schema {
        match self {
            Self::Usage => Schema::Usage,
            Self::Activity => Schema::Activity,
            Self::ReadPosition => Schema::ReadPosition,
            Self::MachineSettings => Schema::MachineSettings,
            Self::Audit => Schema::Audit,
            Self::Connection => Schema::Connection,
            Self::Preferences => Schema::Preferences,
            Self::Alert => Schema::Alert,
        }
    }
}
/// Used by schema/import preparation. Production installation remains gated by
/// the service readiness list, including the final object reader policy.
pub struct Handler(pub Kind);
impl KindHandler for Handler {
    fn kind(&self) -> &'static str {
        self.0.id()
    }
    fn validate(&self, schema: u16, body: &[u8]) -> Result<(), Error> {
        json::schema(schema)?;
        self.0.schema().validate(body)
    }
    fn differences(
        &self,
        schema: u16,
        before: &[u8],
        after: &[u8],
        max_bytes: usize,
    ) -> Result<Vec<FieldChange>, Error> {
        json::schema(schema)?;
        self.0.schema().differences(before, after, max_bytes)
    }
    fn references(&self, schema: u16, body: &[u8]) -> Result<Vec<ContentRef>, Error> {
        json::schema(schema)?;
        self.0.schema().references(body)
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
    Error::Unavailable(
        "record operations require their registered source, trusted projection or semantic submission adapter".into(),
    )
}
