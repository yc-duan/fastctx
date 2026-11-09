//! A persisted use, not a caller's old receipt, supplies the current holding period.
use super::*;

pub(in crate::blocks) fn retained(db: &Connection, custody: &Custody, receipt: &RootLease) -> Result<RootLease> {
    if receipt.object != custody.object
        || receipt.kind != custody.kind.registration().id
        || receipt.holding.holder != custody.holding.holder
        || receipt.holding.scope != custody.holding.scope
    {
        return Err(sql::invalid("root use belongs to another object, kind or holder"));
    }
    let previous =
        saved::get(db, &custody.object.scope, &receipt.id)?.ok_or_else(|| sql::invalid("root use no longer exists"))?;
    if previous.released {
        return Err(sql::invalid("root use has been retired"));
    }
    let mut current = receipt.clone();
    current.holding = custody.holding.clone();
    previous.check(&saved::encode(&current, &custody.purpose)?)?;
    Ok(current)
}
