//! Fixed metric names; account IDs and object paths belong in bounded scenes.

mod platform;
pub use platform::PlatformMetrics;

macro_rules! measures {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl $name {
            const COUNT: usize = [$(stringify!($variant)),+].len();
            fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}
measures!(Counter {
    IngestBytes => "sync.ingest_bytes",
    NewSourceBytes => "sync.new_source_bytes",
    ReconcileBytes => "sync.reconcile_bytes",
    DuplicateRecords => "sync.duplicate_records",
    SentBytes => "sync.sent_bytes",
    UsefulBytes => "sync.useful_bytes",
    SnapshotResets => "sync.snapshot_resets",
    RejectedWrites => "sync.rejected_writes",
    Revocations => "sync.revocations",
    LegacyIncomplete => "sync.legacy_incomplete",
    ThumbnailHits => "sync.thumbnail_hits",
    ResolveScanBytes => "sync.resolve_scan_bytes",
});
measures!(Gauge {
    MissingRecords => "sync.missing_records",
    MissingBytes => "sync.missing_bytes",
    OldestPendingMillis => "sync.oldest_pending_millis",
    ProjectionLag => "sync.projection_lag",
    WriterQueue => "sync.writer_queue",
    TransferBytes => "sync.transfer_bytes",
    TemporaryBytes => "sync.temporary_bytes",
    GarbageCandidates => "sync.garbage_candidates",
    LegacyPinnedRecords => "sync.legacy_pins.rows",
    LegacyPinnedBytes => "sync.legacy_pins.bytes",
});
measures!(Latency {
    Ingest => "sync.ingest",
    ReadPage => "sync.read_page",
    Apply => "sync.apply",
    Commit => "sync.commit",
    LockWait => "sync.lock_wait",
    LockHold => "sync.lock_hold",
    Thumbnail => "sync.thumbnail",
    Send => "sync.send",
    Sign => "sync.sign",
    Verify => "sync.verify",
});
measures!(Cause {
    Ordinary => "ordinary",
    InitialImport => "initial_import",
    Rewrite => "rewrite",
    Recovery => "recovery",
    Legacy => "legacy",
    Integrity => "integrity",
});

pub trait Metrics: Send + Sync {
    fn add(&self, counter: Counter, cause: Cause, amount: u64);
    fn set(&self, gauge: Gauge, value: u64);
    fn elapsed(&self, latency: Latency, nanos: u64);
}
