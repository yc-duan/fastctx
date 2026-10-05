use std::sync::Arc;
use std::time::Duration;

use fastctx_client::World;

use super::bridge::Bridge;

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
    loop {
        let result = (|| {
            let now = fastctx_platform::awake::Sample::now().map_err(|error| error.to_string())?;
            if now.suspended_since(&clock) {
                super::reconnect();
            }
            clock = now;
            let state = world.device().expect("Android device").state();
            bridge.action("publishState", &serde_json::to_string(&state).map_err(|error| error.to_string())?)
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
