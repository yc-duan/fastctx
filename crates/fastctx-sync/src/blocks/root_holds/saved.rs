//! Compare every receipt field with its immutable persisted acquisition.
use super::*;

pub(super) struct Encoded {
    object: String,
    kind: String,
    hash: String,
    form: String,
    length: Option<u32>,
    operation: String,
    generation: u64,
    pub holding: Vec<u8>,
    pub purpose: Vec<u8>,
    pub usage: Vec<u8>,
    expires: Option<i64>,
}
pub(super) struct Saved {
    pub hash: String,
    pub form: String,
    pub released: bool,
    fields: Encoded,
}
impl Saved {
    pub fn holder(&self) -> Result<crate::cursor::Holder> {
        let period: crate::cursor::HoldingPeriod =
            serde_json::from_slice(&self.fields.holding).map_err(|error| sql::invalid(error.to_string()))?;
        Ok(period.holder)
    }
    pub fn check(&self, expected: &Encoded) -> Result<()> {
        self.check_fields(expected)?;
        if self.fields.purpose != expected.purpose {
            return Err(sql::invalid("root usage differs from its persisted purpose"));
        }
        if self.fields.holding != expected.holding {
            let before: crate::cursor::HoldingPeriod =
                serde_json::from_slice(&self.fields.holding).map_err(|e| sql::invalid(e.to_string()))?;
            let after: crate::cursor::HoldingPeriod =
                serde_json::from_slice(&expected.holding).map_err(|e| sql::invalid(e.to_string()))?;
            if before.holder != after.holder || before.scope != after.scope {
                return Err(sql::invalid("root use belongs to another holder or scope"));
            }
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "root use is awaiting holding-period reconciliation",
            ));
        }
        Ok(())
    }
    pub fn check_release(&self, expected: &Encoded, holder: &crate::cursor::Holder) -> Result<()> {
        self.check_fields(expected)?;
        let period: crate::cursor::HoldingPeriod =
            serde_json::from_slice(&self.fields.holding).map_err(|error| sql::invalid(error.to_string()))?;
        if &period.holder != holder {
            return Err(sql::invalid("root usage is owned by another holder"));
        }
        Ok(())
    }
    fn check_fields(&self, expected: &Encoded) -> Result<()> {
        let found = &self.fields;
        if found.object != expected.object
            || found.kind != expected.kind
            || found.hash != expected.hash
            || found.form != expected.form
            || found.length != expected.length
            || found.operation != expected.operation
            || found.generation != expected.generation
            || found.usage != expected.usage
            || found.expires != expected.expires
        {
            return Err(sql::invalid("root usage differs from its persisted acquisition"));
        }
        Ok(())
    }
}
pub(super) fn encode(lease: &RootLease, purpose: &HoldingUse) -> Result<Encoded> {
    let encode = |value: serde_json::Result<Vec<u8>>| value.map_err(|error| sql::invalid(error.to_string()));
    let (form, length) = pins::shape(&lease.root);
    Ok(Encoded {
        object: lease.object.object.as_str().into(),
        kind: lease.kind.clone(),
        hash: lease.root.id().digest.as_str().into(),
        form: form.into(),
        length,
        operation: lease.operation.as_str().into(),
        generation: lease.generation.0,
        holding: encode(serde_json::to_vec(&lease.holding))?,
        purpose: encode(serde_json::to_vec(purpose))?,
        usage: encode(serde_json::to_vec(&lease.usage))?,
        expires: lease.expires_at_millis,
    })
}
pub(super) fn get(db: &Connection, scope: &ScopeKey, id: &RootLeaseId) -> Result<Option<Saved>> {
    db.query_row(
        "SELECT object,kind,hash,root_kind,decoded_bytes,operation,generation,usage,holding,purpose,expires,released FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3",
        params![scope.hub.to_string(),scope.scope.as_str(),id.as_str()],
        |row| {
            let fields = Encoded {
                object: row.get(0)?, kind: row.get(1)?, hash: row.get(2)?, form: row.get(3)?,
                length: row.get(4)?, operation: row.get(5)?, generation: sql::counter(row,6)?,
                usage: row.get(7)?, holding: row.get(8)?, purpose: row.get(9)?, expires: row.get(10)?,
            };
            Ok(Saved { hash: fields.hash.clone(), form: fields.form.clone(), released: row.get(11)?, fields })
        },
    ).optional().map_err(Into::into)
}
