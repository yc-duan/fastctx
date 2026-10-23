//! Match recorded addresses through the machine names known at observation time.

use fastctx_types::Address;
use fastctx_types::layout::Here;

pub(super) fn same(names: &impl Here, recorded: &Address, current: &Address) -> bool {
    recorded.resident == current.resident
        && (recorded.machine == current.machine || names.names_of(&recorded.machine).contains(&current.machine))
}
