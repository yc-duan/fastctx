//! Backpressure belongs to this subscriber's forwarding task, never to the
//! conversation publisher. Ordered live events still fail fast on overflow.

use fastctx_net::watch_queue::{self, Lagged, Sender};
use fastctx_types::{ipc::Failure, session::Watched};

pub(super) async fn send(sender: &Sender<Result<Watched, Failure>>, piece: Watched) -> Result<(), Lagged> {
    // A complete state can exceed the ordinary queue allowance behind a small
    // initial History piece; waiting preserves both pieces.
    let initial = !matches!(piece, Watched::Live { .. } | Watched::Now { .. });
    if initial || watch_queue::replaceable(&piece) {
        sender.send_wait(Ok(piece), std::time::Duration::from_secs(30)).await
    } else {
        sender.send(Ok(piece))
    }
}
