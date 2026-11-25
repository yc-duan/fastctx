//! Starting this computer's node anew from the application, as its user may
//! with no administrator (`fastctx service node restart`): for a node the
//! application cannot reach or that answers nothing.

/// Asks the node's service to start it anew; returns once asked, the new
/// node coming as the World's watch reconnects.
#[fastctx_gui_command::command]
pub async fn restart_local_service() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = fastctx_platform::fastctx_root().map_err(|error| format!("locating the World: {error}"))?;
        let service = fastctx_install::service::Service::new(
            fastctx_install::service::Kind::Node,
            fastctx_install::executable::installed_path(&root),
            &root,
        );
        match service.restart_node() {
            Ok(true) => Ok(()),
            Ok(false) => Err("the local service is not running".into()),
            Err(error) => Err(format!("{error:#}")),
        }
    })
    .await
    .map_err(|error| format!("asking the local service: {error}"))?
}
