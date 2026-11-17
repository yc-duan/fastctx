//! A negative business answer retains the coordinates needed to authorize its receipt.
use fastctx_types::ipc::Failure;
pub(super) fn settled<T>(result: Result<T, Failure>) -> Result<T, Failure> {
    result.map_err(|failure| {
        if failure.is_accepted() {
            Failure::new(format!("{}; whether the accepted work takes effect is not known yet", failure.message))
        } else {
            failure
        }
    })
}
