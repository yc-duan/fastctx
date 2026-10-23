//! Model-visible timer wording, kept together for editorial review.

/// Lateness is said once it reaches a minute: the machine was off or busy then.
pub(super) fn firing(timer: &fastctx_types::timers::Timer, text: &str) -> String {
    let late = timer.late_ms() / 60_000;
    let late = if late > 0 { format!(", {late} min late") } else { String::new() };
    format!("Your timer {} came due at {}{late}.\n{text}", timer.id, timer.due)
}
