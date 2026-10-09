//! Avoid mallocng's shared allocation lock in the static Linux executable.

#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Two arenas keep metadata and fragmentation independent of the host's CPU
// count. Thread caches serve short allocations; freed arena pages are returned
// without a decay delay or a background thread, including after large bursts.
// Keep virtual extents for reuse while purging their physical pages.
// jemalloc reads this C symbol before the first allocation. The pointer refers
// to immutable static bytes and is never changed by fastctx.
#[used]
#[unsafe(export_name = "_rjem_malloc_conf")]
static mut MALLOC_CONF: *const std::ffi::c_char =
    c"background_thread:false,narenas:2,dirty_decay_ms:0,muzzy_decay_ms:0,retain:true".as_ptr();
