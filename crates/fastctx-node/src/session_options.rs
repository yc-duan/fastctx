//! User selections survive a node restart, kept with the conversation's books.

use fastctx_types::ResidentId;
use fastctx_types::agents::SessionOptions;
use fastctx_types::ipc::Failure;

use crate::Node;

fn path(node: &Node, id: &ResidentId) -> std::path::PathBuf {
    node.world.conversation(id).session_options()
}

pub fn keep(node: &Node, id: &ResidentId, options: &SessionOptions) -> Result<(), Failure> {
    let path = path(node, id);
    fastctx_platform::ensure_private_dir(path.parent().expect("options directory"))
        .and_then(|()| {
            fastctx_platform::publish_new(
                &path,
                &serde_json::to_vec(options).expect("options serialize"),
                fastctx_platform::Permissions::Private,
            )
        })
        .map_err(|error| Failure::new(format!("keeping session options in {}: {error}", path.display())))
}

pub fn read(node: &Node, id: &ResidentId) -> Result<SessionOptions, Failure> {
    let path = path(node, id);
    let mut options = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<SessionOptions>(&bytes)
            .map_err(|error| Failure::new(format!("reading {}: {error}", path.display())))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SessionOptions::default(),
        Err(error) => return Err(Failure::new(format!("reading {}: {error}", path.display()))),
    };
    if let Some((about, _)) = node.residents.about(id) {
        options.mcp_off = about.mcp_off;
    }
    Ok(options)
}

pub fn configure(config: &mut serde_json::Value, options: SessionOptions) {
    if let Some(model) = options.model {
        config["model"] = model.into();
    }
    if let Some(effort) = options.reasoning_effort {
        config["model_reasoning_effort"] = effort.into();
    }
}
