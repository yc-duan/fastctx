//! The repair helper may deliver an official package without granting the
//! receiving account access to arbitrary files on the helper's computer.

use std::sync::Arc;
use fastctx_types::{access::{Acting, Written}, update::Manifest};
use crate::Node;

pub(crate) async fn send_package(node: &Arc<Node>, acting: &Acting, manifest: &Manifest, target: &str, machine: &Written, destination: &str) -> Result<(), String> {
    node.authorize_machine(acting, machine).map_err(|e| e.to_string())?;
    let cache = fastctx_update::Cache::new(node.root.join("updates/packages"));
    let path = cache.get(&manifest.version, target).ok_or("repair package is not cached on the helper")?;
    let package = manifest.package(target).ok_or("repair manifest has no such package")?;
    fastctx_update::verify::file(&path, &package.signature, &manifest.version)?;
    let _busy = node.handoff.call().ok_or("repair helper is handing over its node")?;
    let report = super::send(node, acting, path.to_str().ok_or("repair package path is not UTF-8")?, machine, destination, &super::Began::default()).await.map_err(|e| e.to_string())?;
    if !report.problems.is_empty() { return Err(format!("delivering repair package to {destination}: {}", report.problems.join("; "))); }
    Ok(())
}
