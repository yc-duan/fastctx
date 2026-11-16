//! The agents' processes ranked for the kernel's OOM killer, where the
//! system has a way: the node itself last.

use crate::Node;
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux;

pub(crate) fn start(node: &Arc<Node>) {
    #[cfg(target_os = "linux")]
    {
        let node = node.clone();
        fastctx_platform::task::spawn(async move { keep(node).await });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = node;
}

#[cfg(target_os = "linux")]
async fn keep(node: Arc<Node>) {
    use std::time::Duration;
    let opened = fastctx_platform::task::spawn_blocking(linux::Keeper::new).await.map_err(std::io::Error::other);
    let mut keeper = match opened.and_then(|opened| opened) {
        Ok(keeper) => keeper,
        Err(error) => {
            node.log.line(format_args!("agent OOM ranking unavailable: {error}"));
            return;
        }
    };
    node.log.line(format_args!("agent OOM ranking: {}", keeper.description()));
    if !keeper.standing() {
        return;
    }
    let mut previous = String::new();
    loop {
        let round = fastctx_platform::task::spawn_blocking(move || {
            let result = keeper.rank();
            (keeper, result)
        })
        .await;
        let (returned, result) = match round {
            Ok(round) => round,
            Err(error) => {
                node.log.line(format_args!("agent OOM ranking ended: {error}"));
                return;
            }
        };
        keeper = returned;
        if let Err(error) = result {
            let reason = format!("{error}; ranking will retry");
            if reason != previous {
                node.log.line(format_args!("agent OOM ranking: {reason}"));
                previous = reason;
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
