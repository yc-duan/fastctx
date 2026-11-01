//! Runs the node's record storage drill without starting hosts or a network.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 5 && args[1] == "--root" && args[3] == "--only" && args[4] == "migration" {
        return fastctx_node::record_migration_drill(std::path::Path::new(&args[2]));
    }
    if args.len() == 5 && args[1] == "--root" && args[3] == "--only" && args[4] == "transfer" {
        return fastctx_node::record_transfer_drill(std::path::Path::new(&args[2]));
    }
    if !matches!(args.len(), 3 | 5) || args[1] != "--root" || (args.len() == 5 && args[3] != "--family") {
        return Err("usage: record_drill --root ABSOLUTE_EMPTY_DIRECTORY \
                    [--only migration|transfer | --family handovers|executor|facts|records]"
            .into());
    }
    let family = args.get(4).map(|value| value.to_str().ok_or("invalid drill family")).transpose()?;
    fastctx_node::record_drill(std::path::Path::new(&args[2]), family)
}
