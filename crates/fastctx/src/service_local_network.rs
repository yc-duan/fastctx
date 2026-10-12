//! The GUI's single elevated phase when it creates a network here.

use std::path::PathBuf;

use clap::Args;

#[derive(Args)]
pub struct LocalNetworkArgs {
    #[arg(long, value_parser = crate::paths::parser())]
    dir: PathBuf,
    #[arg(long, value_parser = crate::paths::parser())]
    hub_dir: PathBuf,
    #[arg(long, value_parser = crate::paths::parser())]
    executable: PathBuf,
    #[arg(long)]
    user_sid: String,
}

pub fn run(args: LocalNetworkArgs) -> anyhow::Result<()> {
    fastctx_install::service::install_local_network(&args.dir, &args.hub_dir, &args.executable, &args.user_sid)
}
