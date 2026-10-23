//! Model-visible timer wording, kept together for editorial review.

pub(super) fn firing(timer: &fastctx_types::timers::Timer, text: &str) -> String {
    format!("Timer {} was due at {}; delivered {} ms late.\n{}", timer.id, timer.due, timer.late_ms(), text)
}
