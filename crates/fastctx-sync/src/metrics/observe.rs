//! Opt-in, bounded experiment evidence beside the ordinary fixed-name metrics.
//! No observer is constructed unless an absolute output path is configured.
use super::{Cause, Counter, Latency, Metrics, PlatformMetrics};
use crate::{
    conversation::producer::MergeUsage,
    cursor::ByteCount,
    identity::{HostGeneration, OperationId, SourceId},
    store::ingest::{IngestReceipt, SourceCheckpoint},
};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

/// The parent directory must already exist. The file is experimental output,
/// never a checkpoint, asset, or recovery input.
pub const ENVIRONMENT: &str = "FASTCTX_BENCH_OBSERVE";
pub const MAX_OBSERVATION_BYTES: usize = 256 * 1024;
static OBSERVER: OnceLock<Option<Observer>> = OnceLock::new();

/// Resolves explicit configuration once. An unconfigured process creates no file,
/// allocates no event, reads no clock, and registers no observation metrics.
pub fn current() -> Option<&'static Observer> {
    OBSERVER
        .get_or_init(|| {
            let path = std::env::var_os(ENVIRONMENT)?;
            match Observer::open(Path::new(&path)) {
                Ok(observer) => Some(observer),
                Err(_) => {
                    dropped();
                    None
                }
            }
        })
        .as_ref()
}
fn dropped() {
    PlatformMetrics.add(Counter::ObservationDropped, Cause::Ordinary, 1);
}

/// Actual IO increments, including unsuccessful steps and anchor checks.
/// Emit each increment once; these are not file lengths or checkpoint deltas.
#[derive(Clone, Debug)]
pub struct SourceIo {
    pub source: SourceId,
    pub generation: HostGeneration,
    pub operation: Option<OperationId>,
    pub cause: Cause,
    pub open_calls: u64,
    pub read_calls: u64,
    pub logical_read_bytes: u64,
    pub rpc_calls: u64,
}

pub struct Observer {
    file: Mutex<File>,
    started: Instant,
    sequence: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicBool,
}
impl Observer {
    fn open(path: &Path) -> std::io::Result<Self> {
        if !path.is_absolute() || path.parent().is_none_or(|parent| !parent.is_dir()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an existing absolute parent is required",
            ));
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Mutex::new(file),
            started: Instant::now(),
            sequence: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        })
    }

    /// Call after ingest returns its receipt, with the actual durable staged
    /// boundary. Parser roots and lease lists are deliberately not serialized.
    pub fn committed(
        &self,
        checkpoint: &SourceCheckpoint,
        staged_offset: ByteCount,
        receipt: &IngestReceipt,
        merge: Option<&MergeUsage>,
    ) {
        #[derive(Serialize)]
        struct MergeWork {
            effects: usize,
            effect_bytes: usize,
            records: usize,
            payload_bytes: u64,
            commit_bytes: usize,
            work_bytes: usize,
            work_units: u32,
            content_reads: u32,
        }
        let merge = merge.map(|usage| MergeWork {
            effects: usage.effects,
            effect_bytes: usage.effect_bytes,
            records: usage.records,
            payload_bytes: usage.payload_bytes,
            commit_bytes: usage.commit_bytes,
            work_bytes: usage.work_bytes,
            work_units: usage.work_units,
            content_reads: usage.content_reads,
        });
        #[derive(Serialize)]
        struct Committed<'a> {
            event: &'static str,
            source: &'a SourceId,
            generation: &'a HostGeneration,
            revision: crate::cursor::Revision,
            committed_offset: ByteCount,
            staged_offset: ByteCount,
            parser_version: u16,
            operation: &'a OperationId,
            local: crate::cursor::LocalSequence,
            destinations: &'a [crate::store::ingest::SourceReceipt],
            merge: Option<MergeWork>,
        }
        self.emit(&Committed {
            event: "committed",
            source: &checkpoint.source,
            generation: &checkpoint.generation,
            revision: checkpoint.revision,
            committed_offset: checkpoint.committed_offset,
            staged_offset,
            parser_version: checkpoint.parser_version,
            operation: &receipt.operation,
            local: receipt.local,
            destinations: &receipt.sources,
            merge,
        });
    }

    pub fn source_io(&self, sample: &SourceIo) {
        let metrics = PlatformMetrics;
        metrics.add(Counter::SourceOpenCalls, sample.cause, sample.open_calls);
        metrics.add(Counter::SourceReadCalls, sample.cause, sample.read_calls);
        metrics.add(Counter::IngestBytes, sample.cause, sample.logical_read_bytes);
        metrics.add(Counter::SourceRpcCalls, sample.cause, sample.rpc_calls);
        #[derive(Serialize)]
        struct Io<'a> {
            event: &'static str,
            source: &'a SourceId,
            generation: &'a HostGeneration,
            operation: &'a Option<OperationId>,
            cause: &'static str,
            open_calls: u64,
            read_calls: u64,
            logical_read_bytes: u64,
            rpc_calls: u64,
        }
        self.emit(&Io {
            event: "source_io",
            source: &sample.source,
            generation: &sample.generation,
            operation: &sample.operation,
            cause: sample.cause.label(),
            open_calls: sample.open_calls,
            read_calls: sample.read_calls,
            logical_read_bytes: sample.logical_read_bytes,
            rpc_calls: sample.rpc_calls,
        });
    }

    /// CPU must cover the same measured boundary; use None when unavailable.
    /// Clock values are relative to this observer, and cannot align processes.
    pub fn elapsed(&self, operation: &OperationId, stage: Latency, wall_nanos: u64, cpu_nanos: Option<u64>) {
        PlatformMetrics.elapsed(stage, wall_nanos);
        #[derive(Serialize)]
        struct Elapsed<'a> {
            event: &'static str,
            operation: &'a OperationId,
            stage: &'static str,
            wall_nanos: u64,
            cpu_nanos: Option<u64>,
        }
        self.emit(&Elapsed { event: "elapsed", operation, stage: stage.label(), wall_nanos, cpu_nanos });
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    fn discard(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        dropped();
    }
    fn emit(&self, event: &impl Serialize) {
        if self.failed.load(Ordering::Relaxed) {
            self.discard();
            return;
        }
        let Ok(mut file) = self.file.try_lock() else {
            self.discard();
            return;
        };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        #[derive(Serialize)]
        struct Envelope<'a, T> {
            format: u8,
            pid: u32,
            sequence: u64,
            monotonic_nanos: u128,
            dropped: u64,
            #[serde(flatten)]
            event: &'a T,
        }
        let envelope = Envelope {
            format: 1,
            pid: std::process::id(),
            sequence,
            monotonic_nanos: self.started.elapsed().as_nanos(),
            dropped: self.dropped(),
            event,
        };
        let Ok(mut bytes) = fastctx_types::sync::transport_codec::bounded_json(&envelope, MAX_OBSERVATION_BYTES - 1)
        else {
            self.discard();
            return;
        };
        bytes.push(b'\n');
        if file.write_all(&bytes).is_err() {
            // A partial final line is left for the collector to reject. Never
            // append another line behind it and make a false complete record.
            self.failed.store(true, Ordering::Relaxed);
            self.discard();
        }
    }
}
