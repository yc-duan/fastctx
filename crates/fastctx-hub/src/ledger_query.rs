//! Explicit World queries do not change which towns a member replicates.
//! A member asks of the towns its scope covers, and of the World's own
//! ledger: what is outside its scope it is not even shown exists.

use fastctx_net::ledger::TownId;
use fastctx_net::wire::{Question, Told};
use fastctx_types::MachineName;

use crate::Hub;

pub(crate) fn answer(hub: &Hub, name: &MachineName, question: Question) -> Result<Told, String> {
    let store = hub.store();
    let table = store.table();
    table.get(name).ok_or_else(|| format!("{name} is not a member"))?;
    let reads = |town: &TownId| table.readers(town).any(|reader| reader.name == *name);
    let ledgers = hub.ledgers.lock().expect("never poisoned");
    match question {
        Question::LedgerCatalog => Ok(Told::Towns {
            towns: ledgers
                .towns
                .iter()
                .filter(|(town, _)| reads(town))
                .map(|(town, ledger)| (town.clone(), ledger.next - 1))
                .collect(),
        }),
        Question::LedgerQuery { town, after } => {
            if !reads(&town) {
                return Err(format!("{name}'s scope does not include that project"));
            }
            let (entries, more) = ledgers.read_world(&town, after)?;
            Ok(Told::Entries { entries, more })
        }
        _ => unreachable!("only explicit ledger queries reach this handler"),
    }
}
