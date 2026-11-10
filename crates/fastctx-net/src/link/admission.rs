//! Scheduling metadata, independent of the authorization that precedes it.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::contract::Pool;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName};

/// A call's identity for resource accounting; never evidence of permission.
#[derive(Clone)]
pub struct Request {
    pub machine: MachineName,
    pub account: AccountName,
    pub pool: Pool,
    pub conversation: Option<String>,
    pub kind: String,
    pub class: fastctx_types::call_diagnostics::Class,
    pub received: tokio::time::Instant,
    pub within: Option<Duration>,
    pub finish: bool,
    /// Retained control bytes. Large payloads have their own reassembly budget.
    pub bytes: usize,
}

impl Request {
    pub fn receiving(machine: MachineName, account: AccountName, received: tokio::time::Instant) -> Self {
        Self {
            machine,
            account,
            pool: Pool::Receiving,
            conversation: None,
            kind: "receiving".into(),
            class: fastctx_types::call_diagnostics::Class::Unknown,
            received,
            within: Some(fastctx_types::contract::OPENED_WITHIN),
            finish: false,
            bytes: crate::wire::MAX_CONTROL_FRAME * 2,
        }
    }

    pub fn call(
        machine: MachineName,
        account: AccountName,
        received: tokio::time::Instant,
        call: &RemoteCall,
        here: &MachineName,
    ) -> Self {
        let contract = call.contract();
        let conversation = call.scheduling_subject(&machine, here);
        Self {
            machine,
            account,
            pool: contract.pool,
            conversation,
            kind: call.kind(),
            class: call.diagnostic_class(),
            received,
            within: contract.within,
            finish: !contract.cancellable && contract.within.is_some(),
            bytes: control_size(call),
        }
    }
}

fn control_size(call: &RemoteCall) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 > crate::wire::MAX_CONTROL_FRAME {
                return Err(std::io::Error::other("control size known"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, call);
    count.0.min(crate::wire::MAX_CONTROL_FRAME).saturating_add(1024) * 2
}

/// A reserved call, held through the answer. A change retains a bounded
/// settlement record until its work finishes, even after its caller leaves.
pub trait Permit: Send + Sync {
    fn started(&self) {}
    /// Returns processing capacity without abandoning work already begun.
    fn settling(&self) {}
    /// Returns processing capacity for good: the call has handed its work
    /// to what does it apart from the call, and only stays for the outcome.
    fn stays(&self) {}
}

tokio::task_local! {
    static PLACE: Admitted;
}

/// Runs `answering` as the call that holds `place`.
pub(super) async fn held<F: std::future::Future>(place: Admitted, answering: F) -> F::Output {
    PLACE.scope(place, answering).await
}

/// Says that the call the running task answers has submitted its work and
/// from here on only stays for what comes of it: the place it took among
/// the calls of its kind goes to the next one. What it holds in memory stays
/// counted until it answers. It says nothing outside an admitted call.
pub fn stays() {
    let _ = PLACE.try_with(|place| place.stays());
}

/// Compatibility for handlers that do not schedule incoming work.
pub(super) struct Held {
    pub _guard: std::sync::Mutex<Box<dyn Send>>,
}
impl Permit for Held {}

pub type Admitted = Arc<dyn Permit>;
