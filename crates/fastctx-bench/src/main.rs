//! The numbers of the sync design (§18), each measured on its fixed input
//! with fixed seeds, the storage core apart from the whole path: one
//! subcommand a row, each printing what it measured as JSON lines — the
//! row, the measure, p50, p99 and the most, how many samples and how many
//! failed, the target and whether it was met. Run built for release, on a
//! machine doing nothing else, or under the reference machine's limits.

mod advance;
mod fixture;
mod keys;
mod stats;
mod storage;

use std::path::PathBuf;

const USAGE: &str = "usage: fastctx-bench <row|storage> --dir <scratch directory> [--small]
rows: first-screen hub-frames replica-catch-up object-loop durable-word durable-attachment commit-to-readable
follower rows: advance-small advance-large
diagnostic rows: replica-catch-up-parts first-screen-cold (Linux, as root)";

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(row) = args.next() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let (mut dir, mut small) = (None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "--small" => small = true,
            other => {
                eprintln!("{other}?\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let scale = if small { storage::Scale::Small } else { storage::Scale::Full };
    let rows: Vec<&str> = if row == "storage" { storage::ROWS.to_vec() } else { vec![row.as_str()] };
    let mut failed = false;
    for row in rows {
        let at = dir.join(row);
        let _ = std::fs::remove_dir_all(&at);
        if let Err(error) = std::fs::create_dir_all(&at) {
            eprintln!("{row}: {error}");
            std::process::exit(1);
        }
        match storage::run(row, &at, scale) {
            Ok(met) => failed |= !met,
            Err(error) => {
                println!("{}", serde_json::json!({ "row": row, "error": error }));
                failed = true;
            }
        }
        let _ = std::fs::remove_dir_all(&at);
    }
    std::process::exit(i32::from(failed));
}
