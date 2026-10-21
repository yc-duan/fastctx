//! Persistent ledger damage and current IO failures exposed in machine health.
use crate::Node;
use fastctx_types::session::{Health, HealthKind};

pub(crate) fn health(node: &Node) -> Vec<Health> {
    let mut issues: Vec<String> = node
        .usage
        .failures
        .lock()
        .expect("never poisoned")
        .iter()
        .map(|(path, error)| format!("usage ledger {} cannot be read or written: {error}", path.display()))
        .collect();
    let mut scan = || -> std::io::Result<()> {
        let entries = match std::fs::read_dir(super::root(node)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let count = super::quarantine::count(&path)?;
            if count > 0 {
                issues.push(format!(
                    "usage ledger {} has {count} unreadable rows preserved in quarantine; totals omit these rows",
                    path.display()
                ));
            }
        }
        Ok(())
    };
    if let Err(error) = scan() {
        issues.push(format!("reading usage ledger health: {error}"));
    }
    issues.sort();
    issues.into_iter().map(|text| Health { kind: HealthKind::Warning, text }).collect()
}
