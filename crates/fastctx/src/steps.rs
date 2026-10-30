//! What one of the World's own commands did, told to this machine's node
//! from inside the job whose script gave it (`run_facts::RunStep`), so the
//! conversation draws it. Telling never changes what the command does or
//! says: a node too old to take it, or a command run outside a `run`, is
//! passed by in silence.

use std::time::Duration;

use fastctx_client::{Client, command_peer};
use fastctx_types::layout::WorldDir;
use fastctx_types::run_facts::StepDid;
use fastctx_types::{Call, JobId, MachineName, TownName};

/// The longest a command waits for its node to take a step.
const TELLING: Duration = Duration::from_secs(5);

/// A command begun inside a run's job, the step it will tell.
pub(crate) struct Began {
    job: JobId,
    town: Option<TownName>,
    at: jiff::Timestamp,
}

/// The step of a command begun now; none outside a run.
pub(crate) fn began() -> Option<Began> {
    let job = std::env::var("FASTCTX_JOB").ok().filter(|job| !job.is_empty())?.parse().ok()?;
    std::env::var_os("FASTCTX_RESIDENT")?;
    let town = std::env::var("FASTCTX_TOWN").ok().and_then(|town| TownName::try_from(town).ok());
    Some(Began { job, town, at: jiff::Timestamp::now() })
}

/// The machine the run's job is on, as the World names it.
pub(crate) fn here() -> Option<MachineName> {
    std::env::var("FASTCTX_MACHINE").ok()?.parse().ok()
}

impl Began {
    /// Tells what the command did, now that it is done.
    pub(crate) fn tell(self, did: StepDid) {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        let _ = runtime.block_on(async {
            tokio::time::timeout(TELLING, async {
                let world = WorldDir::under(&fastctx_platform::fastctx_root().ok()?);
                let mut client = Client::connect(&world, command_peer().ok()?).await.ok()?;
                let call = Call::Stepped {
                    job: self.job,
                    town: self.town,
                    id: id()?,
                    began: self.at,
                    ended: Some(jiff::Timestamp::now()),
                    did,
                };
                client.call(call).await.ok()
            })
            .await
        });
    }
}

fn id() -> Option<String> {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).ok()?;
    Some(format!("step-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
}
