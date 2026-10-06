//! Explicit World queries do not change which towns a member replicates.
//! Any member may read any town's items: what is sealed for others it
//! cannot open.

use fastctx_net::wire::{Question, Told};
use fastctx_types::MachineName;

use crate::Hub;

pub(crate) fn answer(hub: &Hub, name: &MachineName, question: Question) -> Result<Told, String> {
    let store = hub.store();
    store.table().get(name).ok_or_else(|| format!("{name} is not a member"))?;
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
