//! Audited capture ceremony for the frozen FastCtx v0.1.1 compatibility corpus.

mod capture;
mod fixture;
mod mcp;
mod model;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "compat-v011-capture", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run one platform/oracle arm of the audited 32-process stability ceremony.
    Capture {
        /// Absolute path to the exact v0.1.1 binary under audit.
        #[arg(long)]
        binary: PathBuf,
        /// Static compatibility asset directory.
        #[arg(long)]
        assets: PathBuf,
        /// Dedicated ASCII-only fixture root; it must not already exist.
        #[arg(long)]
        fixture_root: PathBuf,
        /// Stable platform identifier recorded in ledgers.
        #[arg(long)]
        platform: String,
        /// Whether the binary is the exact-commit source build or official release asset.
        #[arg(long, value_enum)]
        oracle: OracleKind,
        /// Fresh server processes per case.
        #[arg(long, default_value_t = 32)]
        runs: usize,
        /// Fixed case-order shuffle seed.
        #[arg(long, default_value_t = 0x0FAC_C011)]
        seed: u64,
        /// Per-frame and process-exit deadline in seconds.
        #[arg(long, default_value_t = 20)]
        timeout_seconds: u64,
    },
    /// Reconcile completed platform ledgers and write the single common expected corpus.
    Finalize {
        /// Static compatibility asset directory.
        #[arg(long)]
        assets: PathBuf,
        /// Required platform identifiers; repeat for every audited target.
        #[arg(long = "platform", required = true)]
        platforms: Vec<String>,
    },
    /// Materialize and independently re-read the deterministic fixture without running FastCtx.
    CertifyFixture {
        /// Static compatibility asset directory.
        #[arg(long)]
        assets: PathBuf,
        /// Empty destination used for readback verification.
        #[arg(long)]
        fixture_root: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OracleKind {
    SourceBuilt,
    Release,
}

impl OracleKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SourceBuilt => "source-built",
            Self::Release => "release",
        }
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("compat-v011-capture: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Capture {
            binary,
            assets,
            fixture_root,
            platform,
            oracle,
            runs,
            seed,
            timeout_seconds,
        } => capture::run(capture::CaptureOptions {
            binary,
            assets,
            fixture_root,
            platform,
            oracle: oracle.as_str().to_string(),
            runs,
            seed,
            timeout: std::time::Duration::from_secs(timeout_seconds),
        }),
        Command::Finalize { assets, platforms } => capture::finalize(&assets, &platforms),
        Command::CertifyFixture {
            assets,
            fixture_root,
        } => {
            let spec = model::read_json(&assets.join("fixture-spec.json"))?;
            let guard = fixture::FixtureGuard::materialize(&fixture_root, &spec)?;
            let readback = guard.verify_immutable()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&readback)
                    .map_err(|error| format!("cannot render fixture certificate: {error}"))?
            );
            guard.finish()?;
            Ok(())
        }
    }
}
