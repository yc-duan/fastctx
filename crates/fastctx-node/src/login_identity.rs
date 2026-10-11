//! Keep owned MCP installations able to identify their selected login.

use crate::Node;

pub(crate) fn refresh(node: &Node) {
    let result = (|| {
        let mut places = fastctx_install::apply::Places::here().map_err(|error| error.to_string())?;
        places.root = node.root.clone();
        places.world = node.world.path().to_path_buf();
        fastctx_install::apply::forward_login_identity(&places).map_err(|error| error.to_string())
    })();
    match result {
        Ok(report) => {
            for path in report.changed {
                node.log.line(format_args!("updated MCP login identity: {}", path.display()));
            }
            for notice in report.notices {
                node.log.line(format_args!("MCP login identity not updated, {}: {}", notice.location, notice.reason));
            }
            for leftover in report.leftovers {
                node.log.line(format_args!("MCP login identity update: {leftover}"));
            }
        }
        Err(error) => node.log.line(format_args!("updating MCP login identity: {error}")),
    }
}
