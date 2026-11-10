//! Response-path budgets: observation only, without cancelling or timing out work.

use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fastctx_types::contract::Response;

tokio::task_local! {
    /// When the call the task handles recorded its work, once it has.
    static RECORDED: Arc<OnceLock<Instant>>;
}

/// Says that the call the running task handles has recorded the work it
/// was asked for: a submission's interval ends here, however long its
/// caller then stays for what comes of the work.
pub fn recorded() {
    let _ = RECORDED.try_with(|recorded| {
        let _ = recorded.set(Instant::now());
    });
}

/// Bounded answers taking longer than this are recorded as violations.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(1);
/// A submission must acknowledge recorded work rather than wait for execution.
pub const SUBMIT_WITHIN: Duration = Duration::from_secs(2);

/// The handling interval of a call that is expected to answer or acknowledge.
/// Dropping it without an answer records nothing; streams have no such interval.
pub struct ResponseTime {
    began: Instant,
    response: Response,
    recorded: Arc<OnceLock<Instant>>,
}

impl ResponseTime {
    /// Starts when a recognized call begins handling, after processing admission.
    pub fn start(response: Response) -> Option<Self> {
        match response {
            Response::Answer | Response::Submit => {
                Some(Self { began: Instant::now(), response, recorded: Arc::default() })
            }
            Response::Stream => None,
        }
    }

    /// Runs the call's handling so that the work it records (`recorded`)
    /// is told to this interval.
    pub fn handling<F: Future>(&self, handling: F) -> impl Future<Output = F::Output> + use<F> {
        RECORDED.scope(self.recorded.clone(), handling)
    }

    /// Records a response written to its caller, including preparation and
    /// output. A submission that recorded its work is timed until then.
    pub fn answered(self, origin: &str, kind: &str) {
        let until = match (self.response, self.recorded.get()) {
            (Response::Submit, Some(recorded)) => *recorded,
            _ => Instant::now(),
        };
        observe(self.response, origin, kind, until.saturating_duration_since(self.began));
    }
}

/// Counts an over-budget response under its origin and call kind. It never
/// panics in strict-role mode or changes the call's result or lifetime.
pub fn observe(response: Response, origin: &str, kind: &str, elapsed: Duration) {
    let family = match response {
        Response::Answer if elapsed > ANSWER_WITHIN => "calls.slow_answer",
        Response::Submit if elapsed > SUBMIT_WITHIN => "calls.slow_submit",
        Response::Answer | Response::Submit | Response::Stream => return,
    };
    let name = format!("{origin}.{kind}");
    crate::readings::counter_of(family, &name).add(1);
    match response {
        Response::Answer => crate::conduct::slow_answer(&name, elapsed),
        Response::Submit => crate::conduct::slow_submit(&name, elapsed),
        Response::Stream => unreachable!("streams have no response budget"),
    }
}
