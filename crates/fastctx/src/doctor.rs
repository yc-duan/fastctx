//! Local operational diagnostics of the serving node.

#[derive(clap::Subcommand)]
pub enum Doctor {
    /// Calls being answered, waiting for admission, or still finishing.
    Calls {
        /// Print structured diagnostics.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: Doctor) -> anyhow::Result<()> {
    let Doctor::Calls { json } = command;
    let world = fastctx_types::layout::WorldDir::under(&fastctx_platform::fastctx_root()?);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let reply = runtime.block_on(async {
        let mut client = fastctx_client::Client::connect(&world, fastctx_client::command_peer()?).await?;
        Ok::<_, anyhow::Error>(client.call(fastctx_types::Call::DoctorCalls).await?)
    })?;
    let fastctx_types::Reply::DoctorCalls { calls } = reply else {
        anyhow::bail!("the node returned no call diagnostics")
    };
    if json {
        println!("{}", serde_json::to_string(&calls)?);
    } else if calls.is_empty() {
        println!("No calls in progress.");
    } else {
        for call in calls {
            println!(
                "{} {} for {} from {}: {}, {} s, {}{}",
                call.id,
                call.kind,
                call.account,
                call.machine,
                call.phase,
                call.seconds,
                call.waiting,
                call.conversation.map(|id| format!(" ({id})")).unwrap_or_default()
            );
        }
    }
    Ok(())
}
