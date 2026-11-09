//! Account schemas and pure compatibility conversions. No service is activated here.
mod authorize;
pub mod collaboration;
pub mod directory;
pub mod profile;
pub mod settings;
pub mod skill;
use crate::{Error, json};
pub use authorize::Authorizer;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    Profile,
    Collaboration,
    Settings,
    Skill,
}
impl Kind {
    pub const ALL: [Self; 5] = [Self::Directory, Self::Profile, Self::Collaboration, Self::Settings, Self::Skill];
    pub fn id(self) -> &'static str {
        match self {
            Self::Directory => "account.directory",
            Self::Profile => "account.profile",
            Self::Collaboration => "account.collaboration",
            Self::Settings => "account.settings",
            Self::Skill => "account.skill",
        }
    }
    fn semantic(self, body: &[u8]) -> Result<Value, Error> {
        match self {
            Self::Profile => profile::semantic(body),
            Self::Collaboration => collaboration::semantic(body),
            _ if body.is_empty() => Ok(Value::Object(Default::default())),
            _ => Ok(Value::Object(json::object(body)?)),
        }
    }
}

/// A stateless schema adapter, not proof that a kind's service ports are ready.
#[derive(Clone, Copy, Debug)]
pub struct Handler(pub Kind);
impl KindHandler for Handler {
    fn kind(&self) -> &'static str {
        self.0.id()
    }
    fn validate(&self, schema: u16, body: &[u8]) -> Result<(), Error> {
        json::schema(schema)?;
        match self.0 {
            Kind::Directory => directory::decode(body).map(|_| ()),
            Kind::Profile => profile::decode(body).map(|_| ()),
            Kind::Collaboration => collaboration::decode(body).map(|_| ()),
            Kind::Settings => settings::decode(body).map(|_| ()),
            Kind::Skill => skill::decode(body).map(|_| ()),
        }
    }
    fn references(&self, schema: u16, body: &[u8]) -> Result<Vec<ContentRef>, Error> {
        self.validate(schema, body)?;
        if self.0 == Kind::Skill { Ok(vec![skill::decode(body)?.directory]) } else { Ok(Vec::new()) }
    }
    fn differences(
        &self,
        schema: u16,
        before: &[u8],
        after: &[u8],
        max_bytes: usize,
    ) -> Result<Vec<FieldChange>, Error> {
        json::schema(schema)?;
        for body in [before, after].into_iter().filter(|body| !body.is_empty()) {
            self.validate(schema, body)?;
        }
        json::differences(&self.0.semantic(before)?, &self.0.semantic(after)?, max_bytes)
    }
    fn import_legacy(&self, shape: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
        match (self.0, shape) {
            (Kind::Profile, "profile/v1") => profile::import(body),
            (Kind::Collaboration, "collaboration/v1") => collaboration::import(body),
            (Kind::Settings, _) => settings::import(shape, body),
            _ => Err(Error::Unavailable("this import needs verified directory or materialization context".into())),
        }
    }
    fn project_legacy(&self, _: &ObjectHead, _: &[u8], _: u16) -> Result<Vec<u8>, Error> {
        Err(Error::Unavailable(
            "legacy projection requires an authenticated ProjectionContext and version mapping".into(),
        ))
    }
    fn project_local(&self, _: &ObjectHead, body: &[u8]) -> Result<Vec<u8>, Error> {
        self.validate(1, body)?;
        // Only the authenticated raw-object reader consumes this projection.
        Ok(body.to_vec())
    }
    fn merge(&self, _: &ProposedObject, _: &ObjectHead, bodies: MergeBodies<'_>) -> Result<MergeOutcome, Error> {
        if matches!(self.0, Kind::Directory | Kind::Skill) {
            return Err(Error::Unavailable(
                "this kind requires its source or prepared-directory publication port".into(),
            ));
        }
        self.validate(1, bodies.candidate)?;
        if bodies.base == bodies.current { Ok(MergeOutcome::UnchangedBase) } else { Ok(MergeOutcome::Conflict) }
    }
    fn apply_edit(&self, _: u16, _: &[u8], _: &[TextEdit], _: usize) -> Result<Vec<u8>, Error> {
        Err(json::invalid("JSON account objects do not accept raw text edits"))
    }
    fn patch_fields(&self, schema: u16, current: &[u8], edits: &[FieldEdit]) -> Result<Vec<u8>, Error> {
        if matches!(self.0, Kind::Directory | Kind::Skill) {
            return Err(Error::Unavailable(
                "this kind requires its source or prepared-directory publication port".into(),
            ));
        }
        let body = json::patch(current, edits)?;
        self.validate(schema, &body)?;
        Ok(body)
    }
    fn export(&self, _: &ObjectHead) -> Result<ExportPlan, Error> {
        Ok(ExportPlan::GenericRootClosure)
    }
    fn delete(&self, _: &ObjectHead) -> Result<DeletePlan, Error> {
        Err(Error::Denied("account removal and skill deletion require their authorized lifecycle transaction".into()))
    }
}
