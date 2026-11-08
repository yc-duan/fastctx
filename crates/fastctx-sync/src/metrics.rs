//! Fixed metric names; account IDs and object paths belong in bounded scenes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counter {
    IngestBytes,
    NewSourceBytes,
    ReconcileBytes,
    DuplicateRecords,
    SentBytes,
    UsefulBytes,
    SnapshotResets,
    RejectedWrites,
    Revocations,
    LegacyIncomplete,
    ThumbnailHits,
    ResolveScanBytes,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gauge {
    MissingRecords,
    MissingBytes,
    OldestPendingMillis,
    ProjectionLag,
    WriterQueue,
    TransferBytes,
    TemporaryBytes,
    GarbageCandidates,
    LegacyPinnedRecords,
    LegacyPinnedBytes,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Latency {
    Ingest,
    ReadPage,
    Apply,
    Commit,
    LockWait,
    LockHold,
    Thumbnail,
    Send,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Ordinary,
    InitialImport,
    Rewrite,
    Recovery,
    Legacy,
    Integrity,
}

pub trait Metrics: Send + Sync {
    fn add(&self, counter: Counter, cause: Cause, amount: u64);
    fn set(&self, gauge: Gauge, value: u64);
    fn elapsed(&self, latency: Latency, nanos: u64);
}
