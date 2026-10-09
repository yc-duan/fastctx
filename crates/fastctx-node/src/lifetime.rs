//! The node records its own containment before accepting any work. The
//! starter's breakaway attempt alone cannot establish this: Windows can
//! leave a child inside a nonbreakaway ancestor job after a successful
//! breakaway from an inner job.

use std::io;

use fastctx_types::node_lifetime::{NodeLifetime, NodeStarter};

pub(crate) fn inspect(supervised: bool) -> io::Result<NodeLifetime> {
    let starter = std::env::var_os("FASTCTX_NODE_STARTER")
        .map(|value| {
            let value = value.into_string().map_err(|_| io::Error::other("FASTCTX_NODE_STARTER is not UTF-8"))?;
            serde_json::from_str::<NodeStarter>(&value).map_err(io::Error::other)
        })
        .transpose()?;
    Ok(NodeLifetime {
        process: fastctx_platform::current_identity()?,
        starter,
        supervised,
        windows_job: fastctx_platform::job_membership::current_job()?,
    })
}
