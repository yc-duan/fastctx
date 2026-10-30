//! Runtime settings and update arrangements. The small JSON file only names
//! the hub and its listening addresses; a move supplies those at its new home.
use super::{
    DueFor, HubFile, PackageTargets, RepairFor,
    rows::{Changes, Listed, ListedNext, Mapped, MappedNext},
    shelf::Shelf,
};
use fastctx_net::sign::PublicKey;
use std::{collections::BTreeMap, io};

pub(super) struct HubRows {
    due: Listed<DueFor>,
    repairs: Listed<RepairFor>,
    targets: Mapped<PublicKey, PackageTargets>,
    settings: BTreeMap<&'static str, Vec<u8>>,
}
pub(super) struct Next {
    due: ListedNext<DueFor>,
    repairs: ListedNext<RepairFor>,
    targets: MappedNext<PublicKey, PackageTargets>,
    settings: BTreeMap<&'static str, Vec<u8>>,
}

impl HubRows {
    pub(super) fn load(shelf: &Shelf, mut hub: HubFile) -> io::Result<(HubFile, Self)> {
        hub.self_registration = shelf.meta("hub.self_registration")?.unwrap_or(hub.self_registration);
        hub.updates = shelf.meta("hub.updates")?.unwrap_or(hub.updates);
        hub.dissolved = shelf.meta("hub.dissolved")?.unwrap_or(hub.dissolved);
        let (due_entries, due) = super::book_rows::list(shelf, "due")?;
        let (repair_entries, repairs) = super::book_rows::list(shelf, "repairs")?;
        hub.due = due_entries;
        hub.repairs = repair_entries;
        hub.package_targets.clear();
        for (key, _, value) in shelf.rows("package_targets")? {
            hub.package_targets.insert(key.parse().map_err(io::Error::other)?, value);
        }
        let targets = Mapped::read("package_targets", hub.package_targets.clone());
        let settings = settings(&hub)?;
        Ok((hub, Self { due, repairs, targets, settings }))
    }
    pub(super) fn save_target(&mut self, shelf: &Shelf, hub: &HubFile, key: &PublicKey) -> io::Result<()> {
        let mut changes = Changes::default();
        let next = self.targets.change_one(&hub.package_targets, key, |key| key.to_string(), &mut changes)?;
        if let Err(error) = shelf.write(changes) {
            self.targets.lost();
            return Err(error);
        }
        self.targets.kept(next);
        Ok(())
    }

    pub(super) fn save(&mut self, shelf: &Shelf, hub: &HubFile) -> io::Result<()> {
        let mut changes = Changes::default();
        let next = self.stage(hub, &mut changes)?;
        if let Err(error) = shelf.write(changes) {
            self.lost();
            return Err(error);
        }
        self.committed(next);
        Ok(())
    }

    pub(super) fn stage(&self, hub: &HubFile, changes: &mut Changes) -> io::Result<Next> {
        let due = self.due.changes(&hub.due, |row| super::book_rows::key("due", row), changes)?;
        let repairs = self.repairs.changes(&hub.repairs, |row| super::book_rows::key("repairs", row), changes)?;
        let targets = self.targets.changes(&hub.package_targets, |key| key.to_string(), changes)?;
        let settings = settings(hub)?;
        for (name, value) in &settings {
            if self.settings.get(name) != Some(value) {
                changes.meta(name, value.clone());
            }
        }
        Ok(Next { due, repairs, targets, settings })
    }

    pub(super) fn lost(&mut self) {
        self.due.lost();
        self.repairs.lost();
        self.targets.lost();
        self.settings.clear();
    }

    pub(super) fn committed(&mut self, next: Next) {
        self.due.kept(next.due);
        self.repairs.kept(next.repairs);
        self.targets.kept(next.targets);
        self.settings = next.settings;
    }
}
fn settings(hub: &HubFile) -> io::Result<BTreeMap<&'static str, Vec<u8>>> {
    Ok([
        ("hub.self_registration", super::rows::body(&hub.self_registration)?),
        ("hub.updates", super::rows::body(&hub.updates)?),
        ("hub.dissolved", super::rows::body(&hub.dissolved)?),
    ]
    .into_iter()
    .collect())
}
