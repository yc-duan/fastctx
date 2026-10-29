use std::sync::Arc;
use std::time::Duration;

use fastctx_client::World;

use super::bridge::Bridge;

/// How long an unchanged state goes without being handed over again.
const SETTLE: Duration = Duration::from_secs(30);

pub(super) async fn run(world: World, bridge: Arc<Bridge>) {
    let mut clock = match fastctx_platform::awake::Sample::now() {
        Ok(clock) => clock,
        Err(error) => {
            if let Err(native) =
                bridge.action("reportConnectionError", &format!("reading Android sleep clocks: {error}"))
            {
                eprintln!("{native}");
            }
            return;
        }
    };
    let mut last_error = None;
    // What was last handed to the service, and when: the same state again
    // crosses only now and then, for the service to settle against it (start
    // again where it stopped), while waking from sleep is looked for every
    // round.
    let mut published: Option<(String, std::time::Instant)> = None;
    loop {
        let result = (|| -> Result<(), String> {
            let now = fastctx_platform::awake::Sample::now().map_err(|error| error.to_string())?;
            if now.suspended_since(&clock) {
                super::reconnect();
            }
            clock = now;
            let state = world.device().expect("Android device").state();
            let state = serde_json::to_string(&state).map_err(|error| error.to_string())?;
            if published.as_ref().is_some_and(|(said, at)| *said == state && at.elapsed() < SETTLE) {
                return Ok(());
            }
            bridge.action("publishState", &state)?;
            published = Some((state, std::time::Instant::now()));
            Ok(())
        })();
        let error = result.err();
        if error != last_error {
            if let Err(native) = bridge.action("reportConnectionError", error.as_deref().unwrap_or("")) {
                eprintln!("{native}");
            }
            last_error = error;
        }
        // Connection and sleep observations cannot wait behind a remote
        // machine answering the notification observer's World query.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
