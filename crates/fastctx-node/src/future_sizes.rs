//! The futures of the node's deepest chains stay small. A future holds the
//! whole of every future it awaits by value, and a poll of an unoptimized
//! build keeps copies of what it builds on its thread's stack: when the
//! steps that cross to a thread that may block held their work by value,
//! telling a resident something grew to nine hundred kilobytes, and nodes
//! overflowed their threads' stacks as they started and in a goal's turn.
//! The work is boxed where it crosses now; this keeps it so.
#![allow(unreachable_code, unused_variables, clippy::diverging_sub_expression)]

use std::future::Future;

/// The most any of these futures may take, in any build: an eighth of
/// what a thread of the runtime has for its stack, on which a poll may keep
/// a few copies of the future it builds.
const MOST: usize = 256 * 1024;

/// The size of the future `make` would return, without calling it.
fn size<F: Future>(_make: impl FnOnce() -> F) -> usize {
    std::mem::size_of::<F>()
}

#[test]
fn deep_futures_stay_small() {
    let sizes = [
        ("run_prepared", size(|| crate::run_prepared(todo!(), todo!(), todo!(), todo!(), todo!()))),
        ("inbox::settle_jobs", size(|| crate::inbox::settle_jobs(todo!()))),
        ("inbox::tell_if_ended", size(|| crate::inbox::tell_if_ended(todo!(), todo!(), todo!()))),
        ("speech::tell", size(|| crate::speech::tell(todo!(), todo!(), todo!(), todo!(), todo!(), todo!()))),
        ("speech::tell_here", size(|| crate::speech::tell_here(todo!(), todo!(), todo!(), todo!()))),
        ("speech::tell_completion", size(|| crate::speech::tell_completion(todo!(), todo!(), todo!()))),
        ("speech::hand_over", size(|| crate::speech::hand_over(todo!(), todo!(), todo!(), todo!(), todo!()))),
        ("speech::recover", size(|| crate::speech::recover(todo!(), todo!()))),
        ("turns::apply", size(|| crate::turns::apply(todo!(), todo!(), todo!()))),
        ("turns::ended", size(|| crate::turns::ended(todo!(), todo!(), todo!(), todo!()))),
        ("turns::unsettled", size(|| crate::turns::unsettled(todo!(), todo!(), todo!(), todo!(), todo!(), todo!()))),
        ("control_queue::run_pending", size(|| crate::control_queue::run_pending(todo!(), todo!()))),
        ("control::control_here", size(|| crate::control::control_here(todo!(), todo!(), todo!()))),
        ("control::execute", size(|| crate::control::execute(todo!(), todo!(), todo!()))),
        (
            "colleagues::ask",
            size(|| crate::colleagues::ask(todo!(), todo!(), todo!(), todo!(), todo!(), todo!(), todo!())),
        ),
        (
            "colleagues::message",
            size(|| crate::colleagues::message(todo!(), todo!(), todo!(), todo!(), todo!(), todo!())),
        ),
    ];
    let large: Vec<String> =
        sizes.iter().filter(|(_, bytes)| *bytes > MOST).map(|(name, bytes)| format!("{name}: {bytes} bytes")).collect();
    assert!(large.is_empty(), "futures larger than {MOST} bytes: {}", large.join(", "));
}
