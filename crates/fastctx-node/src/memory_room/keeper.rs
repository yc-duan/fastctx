//! Proactive anonymous reclaim is optional; admission remains available on
//! kernels or service layouts where reclaim cannot be performed correctly.

use crate::Node;
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux;

pub(crate) fn start(node: &Arc<Node>) {
    #[cfg(target_os = "linux")]
    {
        let node = node.clone();
        tokio::spawn(async move { keep(node).await });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = node;
}

#[cfg(target_os = "linux")]
async fn keep(node: Arc<Node>) {
    use std::time::Duration;
    let mut keeper = match linux::Keeper::new() {
        Ok(keeper) => keeper,
        Err(error) => {
            node.log.line(format_args!("agent memory reclaim unavailable: {error}; turn admission remains active"));
            return;
        }
    };
    node.log.line(format_args!("agent memory keeper: {}", keeper.description()));
    let mut previous = String::new();
    loop {
        let room = node.memory_room.bytes();
        let round = tokio::task::spawn_blocking(move || {
            let result = keeper.tick(room);
            (keeper, result)
        })
        .await;
        let (returned, result) = match round {
            Ok(round) => round,
            Err(error) => {
                node.log.line(format_args!("agent memory keeper ended: {error}; turn admission remains active"));
                return;
            }
        };
        keeper = returned;
        match result {
            Ok(observation) => {
                if observation.state != previous {
                    node.log.line(format_args!("agent memory reclaim: {}", observation.state));
                    previous = observation.state.to_owned();
                }
                if observation.requested > 0 {
                    node.log.line(format_args!(
                        "agent memory reclaim requested {}: swap charge grew {}, available memory {}",
                        super::size(observation.requested),
                        super::size(observation.swapped),
                        super::size(observation.available)
                    ));
                }
            }
            Err(error) => {
                let reason = format!("{error}; reclaim will retry, turn admission remains active");
                if reason != previous {
                    node.log.line(format_args!("agent memory reclaim: {reason}"));
                    previous = reason;
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
