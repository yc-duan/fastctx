//! Explicit World queries do not change which towns a machine replicates.

use fastctx_net::wire::{Question, Told};
use fastctx_types::MachineName;

use crate::Hub;

pub(crate) fn answer(hub: &Hub, name: &MachineName, question: Question) -> Result<Told, String> {
    let store = hub.store();
    let member = store.table().get(name).ok_or_else(|| format!("{name} is not a member"))?;
    if !member.kind.is_machine() {
        return Err(format!("{name} is a device; a machine queries the World's ledgers for it"));
    }
    let ledgers = hub.ledgers.lock().expect("never poisoned");
    match question {
        Question::LedgerCatalog => Ok(Told::Towns {
            towns: ledgers.towns.iter().map(|(town, ledger)| (town.clone(), ledger.next - 1)).collect(),
        }),
        Question::LedgerQuery { town, after } => {
            let (entries, more) = ledgers.read_world(&town, after)?;
            Ok(Told::Entries { entries, more })
        }
        _ => unreachable!("only explicit ledger queries reach this handler"),
    }
}
