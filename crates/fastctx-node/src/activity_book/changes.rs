//! Notify activity readers after the writer committed, including when the
//! caller stopped waiting before the transaction finished.
use std::sync::Arc;

use crate::topics::Topics;

pub(super) fn committed<T>(topics: &Arc<Topics>) -> impl FnOnce(&T) + Send + 'static {
    let topics = topics.clone();
    move |_| topics.changed(fastctx_types::topics::BOOK)
}
