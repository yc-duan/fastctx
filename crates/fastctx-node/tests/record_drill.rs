//! Runs the node's record storage drill without starting hosts or a network.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 || args[1] != "--root" {
        return Err("usage: record_drill --root ABSOLUTE_EMPTY_DIRECTORY".into());
    }
    fastctx_node::record_drill(std::path::Path::new(&args[2]))
}
