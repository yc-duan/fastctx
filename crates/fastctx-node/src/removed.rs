//! A hub took this machine off and asked it to uninstall Fastctx
//! (`world-network.md`, 移出网络与卸载). The node hands that to the removal
//! the machine's own settings make, run by the stable World program in a
//! process of its own: the removal stops the node. What cannot start here
//! is told the hub at once.

use std::path::Path;
use std::sync::Arc;

use fastctx_types::uninstall::Then;

use crate::Node;

/// Uninstalls Fastctx here as the hub whose membership is kept in
/// `membership` asked when it took this machine off.
pub(crate) async fn uninstall(node: &Arc<Node>, membership: &Path, then: Then) {
    if let Some(app) = crate::updates::application(node) {
        let left = format!("Fastctx {app} here is an application: whoever sits at this computer uninstalls it there");
        tell(node, membership, &left).await;
        return;
    }
    let program = fastctx_install::renewal::installed(&node.root);
    let mut args = vec![
        std::ffi::OsString::from("uninstall"),
        "run".into(),
        "--after-removal".into(),
        membership.as_os_str().to_owned(),
    ];
    if then == Then::Everything {
        args.push("--everything".into());
    }
    let started = fastctx_platform::world_process_environment().and_then(|mut environment| {
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("FASTCTX_HOME"));
        environment.push(("FASTCTX_HOME".into(), node.root.clone().into()));
        let args: Vec<&std::ffi::OsStr> = args.iter().map(|arg| arg.as_os_str()).collect();
        fastctx_platform::spawn_detached(&program, &args, &node.root, &environment)
    });
    match started {
        Ok(_) => node.log.line(format_args!(
            "uninstalling Fastctx here{}, as the hub that took this machine off asked",
            if then == Then::Everything { " with everything it wrote" } else { ", keeping its data" }
        )),
        Err(error) => tell(node, membership, &format!("starting the uninstall: {error}")).await,
    }
}

async fn tell(node: &Node, membership: &Path, left: &str) {
    node.log.line(format_args!("not uninstalling Fastctx here as asked: {left}"));
    let world = fastctx_types::layout::WorldDir::under(&node.root);
    if let Err(error) = fastctx_uninstall::tell_left(&world, membership, left).await {
        node.log.line(format_args!("telling the hub what was left of the uninstall: {error}"));
    }
}
