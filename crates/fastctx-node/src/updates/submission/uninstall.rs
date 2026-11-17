//! Preparation finishes host cleanup before the external uninstaller may proceed.
use super::*;

const FAMILY: Family = Family {
    name: "call/uninstall-prepare",
    at_once: 1,
    within: Duration::from_secs(30 * 60),
    stays: Duration::from_secs(240),
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Prepare;

pub(super) fn register(node: &Arc<Node>) {
    submitted::register(
        node,
        FAMILY,
        |node: Arc<Node>, _: Prepare| async move { crate::uninstall::prepare(&node).await },
    );
}

pub(crate) async fn prepare(node: &Arc<Node>) -> Result<(), Failure> {
    if !super::started(node) {
        return crate::uninstall::prepare(node).await;
    }
    submitted::submit(node, &FAMILY, "this-machine", &Prepare).await
}
