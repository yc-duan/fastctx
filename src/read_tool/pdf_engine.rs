//! Validation, atomic extraction, and lazy binding for the embedded Pdfium library.

use fs2::FileExt;
use pdfium_render::prelude::Pdfium;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/pdfium_embedded.rs"));
}

#[derive(Clone, Copy)]
struct EngineArtifact<'a> {
    bytes: &'a [u8],
    filename: &'a str,
    sha256: &'a str,
    release_tag: &'a str,
}

const BUNDLED_ARTIFACT: EngineArtifact<'static> = EngineArtifact {
    bytes: bundled::PDFIUM_BYTES,
    filename: bundled::PDFIUM_FILENAME,
    sha256: bundled::PDFIUM_SHA256,
    release_tag: bundled::PDFIUM_RELEASE_TAG,
};

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();
static PDFIUM_INITIALIZATION: Mutex<()> = Mutex::new(());
static PDFIUM_OPERATIONS: Mutex<()> = Mutex::new(());
static PDF_RUNTIME: PdfRuntime = PdfRuntime::new();
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const PDF_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Eq, PartialEq)]
pub(super) enum PdfOperationError {
    TimedOut,
    Unavailable(String),
}

struct PdfRuntime {
    degraded_reason: OnceLock<String>,
}

impl PdfRuntime {
    const fn new() -> Self {
        Self {
            degraded_reason: OnceLock::new(),
        }
    }

    fn run<T: Send + 'static>(
        &self,
        timeout: Duration,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PdfOperationError> {
        if let Some(reason) = self.degraded_reason() {
            return Err(PdfOperationError::Unavailable(reason));
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("fastctx-pdf-operation".to_string())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(operation));
                let _ = sender.send(outcome);
            })
            .map_err(|error| {
                PdfOperationError::Unavailable(format!(
                    "could not start the isolated PDF operation: {error}"
                ))
            })?;

        match receiver.recv_timeout(timeout) {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(PdfOperationError::Unavailable(self.degrade(
                "the PDF engine panicked while processing a document; PDF support is disabled until the server restarts",
            ))),
            Err(RecvTimeoutError::Timeout) => {
                self.degrade(
                    "a PDF operation timed out; PDF support is disabled until the server restarts",
                );
                Err(PdfOperationError::TimedOut)
            }
            Err(RecvTimeoutError::Disconnected) => Err(PdfOperationError::Unavailable(
                self.degrade(
                    "the isolated PDF operation ended without a result; PDF support is disabled until the server restarts",
                ),
            )),
        }
    }

    fn degraded_reason(&self) -> Option<String> {
        self.degraded_reason.get().cloned()
    }

    fn degrade(&self, reason: &str) -> String {
        let _ = self.degraded_reason.set(reason.to_string());
        self.degraded_reason
            .get()
            .cloned()
            .unwrap_or_else(|| reason.to_string())
    }
}

pub(super) fn run_pdf_operation<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<T, PdfOperationError> {
    PDF_RUNTIME.run(PDF_OPERATION_TIMEOUT, operation)
}

pub(super) fn pdfium_session() -> Result<(MutexGuard<'static, ()>, &'static Pdfium), String> {
    if let Some(reason) = PDF_RUNTIME.degraded_reason() {
        return Err(reason);
    }
    // The Pdfium C API uses process-global state, so overlapping document operations contaminate one another.
    let operation = PDFIUM_OPERATIONS
        .lock()
        .map_err(|_| "the PDF engine operation lock was poisoned".to_string())?;
    if let Some(reason) = PDF_RUNTIME.degraded_reason() {
        return Err(reason);
    }
    let pdfium = pdfium()?;
    Ok((operation, pdfium))
}

fn pdfium() -> Result<&'static Pdfium, String> {
    if let Some(pdfium) = PDFIUM.get() {
        return Ok(pdfium);
    }
    let _initialization = PDFIUM_INITIALIZATION
        .lock()
        .map_err(|_| "the PDF engine initialization lock was poisoned".to_string())?;
    if let Some(pdfium) = PDFIUM.get() {
        return Ok(pdfium);
    }
    let initialized = initialize_pdfium()?;
    PDFIUM
        .set(initialized)
        .map_err(|_| "the PDF engine was initialized concurrently".to_string())?;
    PDFIUM
        .get()
        .ok_or_else(|| "the PDF engine disappeared after initialization".to_string())
}

fn initialize_pdfium() -> Result<Pdfium, String> {
    verify_embedded_bytes(BUNDLED_ARTIFACT)?;
    let primary = cache_dir().ok_or_else(|| "no user cache location is available".to_string());
    let fallback = fallback_dir();
    let path = release_with_fallback(primary, &fallback, BUNDLED_ARTIFACT)?;
    if !file_hash_matches(&path, BUNDLED_ARTIFACT.sha256)? {
        return Err(format!(
            "released engine changed before loading at {}",
            path.display()
        ));
    }
    let bindings = Pdfium::bind_to_library(&path)
        .map_err(|error| format!("dynamic binding failed: {error}"))?;
    Ok(Pdfium::new(bindings))
}

fn verify_embedded_bytes(artifact: EngineArtifact<'_>) -> Result<(), String> {
    let actual = hex::encode(Sha256::digest(artifact.bytes));
    if actual == artifact.sha256 {
        Ok(())
    } else {
        Err(format!(
            "embedded engine hash mismatch: expected {}, got {actual}",
            artifact.sha256
        ))
    }
}

fn cache_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("fastctx"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("Library/Caches/fastctx"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".cache"))
            })
            .map(|path| path.join("fastctx"))
    }
}

fn fallback_dir() -> PathBuf {
    let identity = cache_dir()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("unknown-user"));
    let digest = hex::encode(Sha256::digest(identity.to_string_lossy().as_bytes()));
    std::env::temp_dir().join(format!("fastctx-{}", &digest[..16]))
}

fn release_with_fallback(
    primary: Result<PathBuf, String>,
    fallback: &Path,
    artifact: EngineArtifact<'_>,
) -> Result<PathBuf, String> {
    match primary.and_then(|directory| ensure_library_in(&directory, artifact)) {
        Ok(path) => Ok(path),
        Err(primary_error) => ensure_library_in(fallback, artifact).map_err(|fallback_error| {
            format!(
                "cache extraction failed: {primary_error}; temporary extraction failed: {fallback_error}"
            )
        }),
    }
}

fn ensure_library_in(directory: &Path, artifact: EngineArtifact<'_>) -> Result<PathBuf, String> {
    prepare_directory(directory)?;
    let destination = destination_path(directory, artifact);
    if file_hash_matches(&destination, artifact.sha256).unwrap_or(false) {
        secure_file_permissions(&destination)?;
        return Ok(destination);
    }
    let lock_path = directory.join(format!(
        ".{}.{}.lock",
        artifact.filename,
        &artifact.sha256[..16]
    ));
    let _release_lock = acquire_release_lock(&lock_path)?;
    if file_hash_matches(&destination, artifact.sha256).unwrap_or(false) {
        secure_file_permissions(&destination)?;
        return Ok(destination);
    }
    remove_stale_destination(&destination)?;

    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        ".{}.{}.{}.tmp",
        artifact.filename,
        std::process::id(),
        sequence
    ));
    let write_result = write_temporary(&temporary, artifact.bytes).and_then(|()| {
        fs::rename(&temporary, &destination).map_err(|error| {
            format!(
                "cannot atomically move {} to {}: {error}",
                temporary.display(),
                destination.display()
            )
        })
    });
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    secure_file_permissions(&destination)?;
    if file_hash_matches(&destination, artifact.sha256)? {
        Ok(destination)
    } else {
        Err(format!(
            "released engine failed hash verification at {}",
            destination.display()
        ))
    }
}

fn acquire_release_lock(lock_path: &Path) -> Result<File, String> {
    match fs::symlink_metadata(lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "release lock path is not a regular file: {}",
                lock_path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect release lock {}: {error}",
                lock_path.display()
            ));
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(lock_path)
        .map_err(|error| format!("cannot open release lock {}: {error}", lock_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(lock_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            format!(
                "cannot secure release lock {}: {error}",
                lock_path.display()
            )
        })?;
    }
    file.lock_exclusive()
        .map_err(|error| format!("cannot lock release file {}: {error}", lock_path.display()))?;
    file.set_len(0)
        .map_err(|error| format!("cannot reset release lock {}: {error}", lock_path.display()))?;
    writeln!(file, "{}", std::process::id())
        .map_err(|error| format!("cannot write release lock {}: {error}", lock_path.display()))?;
    Ok(file)
}

fn destination_path(directory: &Path, artifact: EngineArtifact<'_>) -> PathBuf {
    let release = artifact.release_tag.replace('/', "-");
    let original = Path::new(artifact.filename);
    let stem = original
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("pdfium");
    let extension = original
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    directory.join(format!(
        "{stem}-{release}-{}.{}",
        &artifact.sha256[..16],
        extension
    ))
}

fn remove_stale_destination(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot replace corrupted cached engine {}: {error}",
            path.display()
        )),
    }
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

fn file_hash_matches(path: &Path, expected: &str) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "cannot inspect cached engine {}: {error}",
                path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "cached engine path is not a regular file: {}",
            path.display()
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot verify cached engine {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot verify cached engine {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()) == expected)
}

fn prepare_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "cache location is not a regular directory: {}",
                    path.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure {}: {error}", path.display()))?;
    }
    Ok(())
}

fn secure_file_permissions(_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o500))
            .map_err(|error| format!("cannot secure {}: {error}", _path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        EngineArtifact, PdfOperationError, PdfRuntime, destination_path, ensure_library_in,
        release_with_fallback,
    };
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    const TEST_ARTIFACT: EngineArtifact<'static> = EngineArtifact {
        bytes: b"test-engine",
        filename: "pdfium.test",
        sha256: "f3d95bb7396a6919739c96947848496a1dd3473dc6e0061e51a7092bbd26a3d3",
        release_tag: "test/1",
    };

    #[test]
    fn corrupted_cache_is_replaced_and_then_reused() {
        let temp = tempfile::tempdir().unwrap();
        let first = ensure_library_in(temp.path(), TEST_ARTIFACT).unwrap();
        assert_eq!(fs::read(&first).unwrap(), TEST_ARTIFACT.bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&first, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(&first, b"corrupt").unwrap();
        let repaired = ensure_library_in(temp.path(), TEST_ARTIFACT).unwrap();
        assert_eq!(first, repaired);
        assert_eq!(fs::read(repaired).unwrap(), TEST_ARTIFACT.bytes);
    }

    #[test]
    fn unwritable_primary_shape_falls_back_to_the_temporary_location() {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("not-a-directory");
        fs::write(&primary, b"occupied").unwrap();
        let fallback = temp.path().join("fallback");
        let released = release_with_fallback(Ok(primary), &fallback, TEST_ARTIFACT).unwrap();
        assert_eq!(released, destination_path(&fallback, TEST_ARTIFACT));
        assert_eq!(fs::read(released).unwrap(), TEST_ARTIFACT.bytes);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_cache_directory_falls_back_to_temporary_storage() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let read_only_parent = temp.path().join("read-only");
        fs::create_dir(&read_only_parent).unwrap();
        fs::set_permissions(&read_only_parent, fs::Permissions::from_mode(0o555)).unwrap();
        let primary = read_only_parent.join("cache");
        let fallback = temp.path().join("fallback");
        let result = release_with_fallback(Ok(primary), &fallback, TEST_ARTIFACT);
        fs::set_permissions(&read_only_parent, fs::Permissions::from_mode(0o755)).unwrap();
        let released = result.unwrap();
        assert_eq!(released, destination_path(&fallback, TEST_ARTIFACT));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cache_directory_is_rejected_and_falls_back() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let primary = temp.path().join("linked-cache");
        symlink(&actual, &primary).unwrap();
        let fallback = temp.path().join("fallback");
        let released = release_with_fallback(Ok(primary), &fallback, TEST_ARTIFACT).unwrap();
        assert_eq!(released, destination_path(&fallback, TEST_ARTIFACT));
        assert_eq!(fs::read_dir(actual).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_release_lock_cannot_redirect_writes() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("primary");
        fs::create_dir(&primary).unwrap();
        let victim = temp.path().join("victim.txt");
        fs::write(&victim, b"do not touch").unwrap();
        let lock = primary.join(format!(
            ".{}.{}.lock",
            TEST_ARTIFACT.filename,
            &TEST_ARTIFACT.sha256[..16]
        ));
        symlink(&victim, lock).unwrap();
        let fallback = temp.path().join("fallback");
        let released = release_with_fallback(Ok(primary), &fallback, TEST_ARTIFACT).unwrap();
        assert_eq!(released, destination_path(&fallback, TEST_ARTIFACT));
        assert_eq!(fs::read(victim).unwrap(), b"do not touch");
    }

    #[test]
    fn concurrent_first_release_converges_on_one_valid_file() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let directory = directory.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ensure_library_in(&directory, TEST_ARTIFACT).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let paths = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert!(paths.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(fs::read(&paths[0]).unwrap(), TEST_ARTIFACT.bytes);
        let files = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            files.iter().filter(|name| !name.ends_with(".lock")).count(),
            1
        );
        assert!(files.iter().any(|name| name.ends_with(".lock")));
    }

    #[test]
    fn pdf_runtime_catches_panics_and_disables_future_operations() {
        let runtime = PdfRuntime::new();
        let error = runtime
            .run(Duration::from_secs(1), || -> usize {
                panic!("synthetic PDF panic")
            })
            .unwrap_err();
        assert!(matches!(
            error,
            PdfOperationError::Unavailable(ref reason)
                if reason.contains("panicked") && reason.contains("server restarts")
        ));

        let executed = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&executed);
        let second = runtime.run(Duration::from_secs(1), move || {
            marker.store(true, Ordering::SeqCst);
            7
        });
        assert!(matches!(second, Err(PdfOperationError::Unavailable(_))));
        assert!(!executed.load(Ordering::SeqCst));
    }

    #[test]
    fn pdf_runtime_times_out_without_blocking_the_caller_and_then_degrades() {
        let runtime = PdfRuntime::new();
        let started = Instant::now();
        let result = runtime.run(Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_millis(250));
            7
        });
        assert_eq!(result, Err(PdfOperationError::TimedOut));
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(matches!(
            runtime.run(Duration::from_secs(1), || 9),
            Err(PdfOperationError::Unavailable(ref reason)) if reason.contains("timed out")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn released_engine_and_cache_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("secure-cache");
        let released = ensure_library_in(&directory, TEST_ARTIFACT).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(released).unwrap().permissions().mode() & 0o777,
            0o500
        );
    }

    #[cfg(unix)]
    #[test]
    fn valid_cached_engine_permissions_are_repaired_on_reuse() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let released = ensure_library_in(temp.path(), TEST_ARTIFACT).unwrap();
        fs::set_permissions(&released, fs::Permissions::from_mode(0o600)).unwrap();
        let reused = ensure_library_in(temp.path(), TEST_ARTIFACT).unwrap();
        assert_eq!(released, reused);
        assert_eq!(
            fs::metadata(reused).unwrap().permissions().mode() & 0o777,
            0o500
        );
    }
}
