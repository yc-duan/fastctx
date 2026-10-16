use clap::{Args, Subcommand};
use fastctx_install::uninstall::Installation;
use std::io::Write as _;
use std::path::PathBuf;

#[derive(Args)]
pub struct Options {
    #[arg(long)]
    everything: bool,
    #[arg(long)]
    json: bool,
    /// Package-owned data directories, passed only by the graphical shell.
    #[arg(long, hide = true)]
    application_dir: Vec<PathBuf>,
    #[arg(long, hide = true)]
    application: Option<PathBuf>,
    #[arg(long, hide = true)]
    application_image: Option<PathBuf>,
    #[arg(long, hide = true)]
    application_process: Option<String>,
    #[arg(long, hide = true)]
    application_control: Option<PathBuf>,
    #[arg(long, hide = true)]
    graphical: bool,
    /// The membership whose hub took this machine off and asked for this
    /// removal, passed only by the node: what is left undone is told it.
    #[arg(long, hide = true)]
    after_removal: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Uninstall {
    /// Read the installation and report every intended step; changes nothing.
    Plan(Options),
    /// Remove the local World's programs and connections, retaining data unless requested.
    Run(Options),
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        request: PathBuf,
    },
    #[command(hide = true)]
    System {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        control: PathBuf,
        #[arg(long)]
        controller: String,
    },
    #[cfg(target_os = "linux")]
    #[command(hide = true)]
    Package,
    #[cfg(target_os = "linux")]
    #[command(hide = true)]
    Application {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        process: String,
        #[arg(long)]
        temporary: PathBuf,
        #[arg(long)]
        data: Vec<PathBuf>,
    },
    /// Removes the macOS application bundle once the application exited.
    #[cfg(target_os = "macos")]
    #[command(hide = true)]
    Bundle {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        process: String,
        #[arg(long)]
        temporary: PathBuf,
        #[arg(long)]
        data: Vec<PathBuf>,
    },
    #[command(hide = true)]
    ApplicationFinish,
}

pub fn run(command: Uninstall) -> anyhow::Result<()> {
    match command {
        Uninstall::ApplicationFinish => fastctx_uninstall::finish_application(&fastctx_platform::fastctx_root()?),
        #[cfg(target_os = "linux")]
        Uninstall::Package => fastctx_uninstall::package::before_remove(),
        #[cfg(target_os = "linux")]
        Uninstall::Application { image, process, temporary, data } => tokio::runtime::Runtime::new()?.block_on(
            fastctx_uninstall::package::remove_image(&image, &serde_json::from_str(&process)?, &temporary, data),
        ),
        #[cfg(target_os = "macos")]
        Uninstall::Bundle { bundle, process, temporary, data } => tokio::runtime::Runtime::new()?.block_on(
            fastctx_uninstall::bundle::remove_bundle(&bundle, &serde_json::from_str(&process)?, &temporary, data),
        ),
        Uninstall::System { request, output, control, controller } => {
            let request: fastctx_uninstall::Request = serde_json::from_slice(&std::fs::read(request)?)?;
            let controller = serde_json::from_str(&controller)?;
            tokio::runtime::Runtime::new()?.block_on(fastctx_uninstall::system_worker(
                &request,
                &output,
                &control,
                &controller,
            ))
        }
        Uninstall::Plan(options) => {
            let request = request(&options)?;
            if options.json {
                println!("{}", serde_json::to_string(&request.plan)?);
            } else {
                for step in request.plan.steps {
                    println!(
                        "{}: {} ({}){}",
                        step.id,
                        step.what,
                        step.location.unwrap_or_default(),
                        if step.admin { " [administrator]" } else { "" }
                    );
                }
                for item in request.plan.keeps {
                    println!("Keep: {} ({})", item.what, item.location.unwrap_or_default());
                }
            }
            Ok(())
        }
        Uninstall::Run(options) => {
            let request = request(&options)?;
            match &options.after_removal {
                Some(membership) => fastctx_uninstall::after_removal(&request, membership),
                None => fastctx_uninstall::run_outside(&request),
            }
        }
        Uninstall::Worker { request } => {
            let request = fastctx_uninstall::read_request(&request)?;
            tokio::runtime::Runtime::new()?.block_on(fastctx_uninstall::run(request, |event| {
                println!("{}", serde_json::to_string(&event).expect("uninstall events serialize"));
                if let Err(error) = std::io::stdout().flush() {
                    eprintln!("uninstall progress could not be displayed: {error}");
                }
            }))
        }
    }
}

fn request(options: &Options) -> anyhow::Result<fastctx_uninstall::Request> {
    let mut installation = Installation::here()?;
    installation.application_dirs.extend(options.application_dir.clone());
    installation.application_dirs.sort();
    installation.application_dirs.dedup();
    installation.application = options.application.clone();
    installation.application_image = options.application_image.clone();
    installation.application_process = options.application_process.as_deref().map(serde_json::from_str).transpose()?;
    installation.application_control = options.application_control.clone();
    installation.graphical = options.graphical;
    fastctx_uninstall::preview(installation, options.everything)
}
