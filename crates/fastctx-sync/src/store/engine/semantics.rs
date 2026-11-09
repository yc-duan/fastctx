//! Business deltas come from verified bodies, never the submitted patch paths.
use crate::{
    registry::handlers::KindHandler,
    store::{Error, authorize::FieldChange},
};

pub(super) fn derive(
    handler: &dyn KindHandler,
    schema: u16,
    before: &[u8],
    after: &[u8],
    max_bytes: usize,
) -> Result<(Vec<FieldChange>, usize), Error> {
    let mut fields = handler.differences(schema, before, after, max_bytes)?;
    let mut bytes = fields.capacity().checked_mul(size_of::<FieldChange>()).ok_or(Error::Busy)?;
    if fields.len() > 4096 {
        return Err(Error::Invalid("semantic field count exceeds its bound".into()));
    }
    for field in &fields {
        if field.path.is_empty() || field.path.len() > 64 {
            return Err(Error::Invalid("semantic fields require bounded nonempty paths".into()));
        }
        bytes = bytes
            .checked_add(field.path.capacity().checked_mul(size_of::<String>()).ok_or(Error::Busy)?)
            .ok_or(Error::Busy)?;
        for component in &field.path {
            bytes = bytes.checked_add(component.capacity()).ok_or(Error::Busy)?;
        }
    }
    if bytes > max_bytes {
        return Err(Error::Invalid("semantic field changes exceed their operation budget".into()));
    }
    fields.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    if fields.windows(2).any(|pair| pair[1].path.starts_with(&pair[0].path)) {
        return Err(Error::Invalid("semantic fields must not overlap".into()));
    }
    Ok((fields, bytes))
}
