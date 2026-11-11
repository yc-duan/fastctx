//! Shape limits for the synchronous list and diagnostic requests.

use fastctx_types::Call;
use fastctx_types::ipc::Failure;

pub(super) fn check(call: &Call) -> Result<(), Failure> {
    match call {
        Call::Look { ids } => strings("detail IDs", ids, 64, 256, fastctx_types::contract::LOOK_REQUEST_BYTES),
        Call::WorldNow { known } | Call::WatchWorld { known } => {
            strings("known machines", known, 16_384, 512, 4 * 1024 * 1024)
        }
        Call::Stats { seen: Some(seen), .. } => seen.check().map_err(Failure::not_run),

        Call::Machines { thread, login, .. } => {
            if thread.as_ref().is_some_and(|thread| thread.len() > 256)
                || login
                    .as_ref()
                    .and_then(|login| login.home.as_ref())
                    .is_some_and(|home| home.as_os_str().len() > 4096)
            {
                return Err(Failure::not_run("machine request thread or login path exceeds its byte limit"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn strings(what: &str, values: &[String], count: usize, each: usize, bytes: usize) -> Result<(), Failure> {
    if values.len() > count
        || values.iter().any(|value| value.len() > each)
        || values.iter().map(String::len).sum::<usize>() > bytes
    {
        return Err(Failure::not_run(format!(
            "{what} exceed their request limit: at most {count} values, {each} bytes each and {bytes} bytes in total"
        )));
    }
    Ok(())
}
