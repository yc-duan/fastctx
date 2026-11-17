//! A sampled projection must not replace a previously measured capture extent.

use fastctx_types::run_facts::OutputRef;

pub(super) fn preserve(output: &mut OutputRef, kept: &OutputRef) -> bool {
    if output.lines == 0
        && kept.lines > 0
        && output.bytes == kept.bytes
        && output.machine == kept.machine
        && output.path == kept.path
    {
        output.lines = kept.lines;
        output.cut = kept.cut;
        true
    } else {
        false
    }
}
