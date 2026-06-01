//! Logging initialization for the `gitlore` workspace.
//!
//! [`init_logging`] wires `tracing` to two sinks:
//!
//! * **stderr** — compact human-readable format, level depends on
//!   [`LogLevel`].
//! * **file**   — full structured format with target, file, line, thread,
//!   level — appended to a size-rolling file under `log_dir`.
//!
//! Level matrix (subject to `RUST_LOG` override):
//!
//! | [`LogLevel`] | stderr  | file    |
//! |---|---|---|
//! | [`LogLevel::Quiet`]   | `ERROR` | `INFO`  |
//! | [`LogLevel::Normal`]  | `WARN`  | `INFO`  |
//! | [`LogLevel::Verbose`] | `INFO`  | `DEBUG` |
//!
//! `RUST_LOG`, when set, supersedes the default directives on both sinks.
//!
//! File rotation: each file caps at [`FILE_SIZE_LIMIT_BYTES`]
//! (10 MiB) and [`FILE_RETENTION`] historical rotations are kept
//! (`gitlore.log`, `gitlore.log.1`, `gitlore.log.2`, `gitlore.log.3`).
//!
//! XDG fallback: when `log_dir` is `None`, the file sink resolves via
//! [`directories::ProjectDirs`] — `state_dir()` when available (Linux/BSD),
//! `data_local_dir()` otherwise (macOS, Windows). The same root the index
//! storage uses (Q15b, ADR-029), so log files live next to the per-user
//! gitlore state by default. When the platform exposes no state path at
//! all, file logging is silently skipped and only the stderr sink is wired.
//!
//! Writes go through `tracing_appender::non_blocking`, so logging never
//! blocks the indexer or the TUI render loop. The returned [`LogGuard`]
//! owns the flush-on-drop worker handle; callers MUST keep it alive for
//! the lifetime of the program.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

/// Maximum bytes per individual log file before the rolling writer rotates
/// `gitlore.log` to `gitlore.log.1` (shifting older rotations one slot down).
pub const FILE_SIZE_LIMIT_BYTES: u64 = 10 * 1024 * 1024;

/// Number of rotated historical files retained on disk
/// (`gitlore.log.1` .. `gitlore.log.{FILE_RETENTION}`).
/// Beyond this, the oldest file is deleted on each rotation.
pub const FILE_RETENTION: usize = 3;

/// Active log file name. Rotated copies append `.1`, `.2`, ... `.{FILE_RETENTION}`.
pub const LOG_FILE_NAME: &str = "gitlore.log";

/// Sub-directory name used for the XDG state fallback path
/// (`<state-dir>/gitlore/`). Matches the index storage layout (Q15b).
const XDG_QUALIFIER: &str = "";
const XDG_ORGANIZATION: &str = "";
const XDG_APPLICATION: &str = "gitlore";

/// CLI-driven verbosity preset.
///
/// Maps the `--quiet` / `--verbose` flags from the gitlore CLI onto a
/// pair of `tracing` level directives (stderr + file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogLevel {
    /// `--quiet`: stderr at `ERROR`, file at `INFO`.
    Quiet,
    /// Default: stderr at `WARN`, file at `INFO`.
    #[default]
    Normal,
    /// `--verbose`: stderr at `INFO`, file at `DEBUG`.
    Verbose,
}

impl LogLevel {
    /// Level for the stderr layer (compact human-readable format).
    fn stderr_level(self) -> Level {
        match self {
            Self::Quiet => Level::ERROR,
            Self::Normal => Level::WARN,
            Self::Verbose => Level::INFO,
        }
    }

    /// Level for the rolling-file layer (full structured format).
    fn file_level(self) -> Level {
        match self {
            Self::Quiet | Self::Normal => Level::INFO,
            Self::Verbose => Level::DEBUG,
        }
    }
}

/// RAII guard returned by [`init_logging`].
///
/// Must be held for the lifetime of the program. Dropping it flushes the
/// non-blocking file writer; if it is dropped early, log records still in
/// the writer's queue may be lost.
#[must_use = "drop the LogGuard at end of `main`; dropping early flushes the file sink and silently truncates pending log records"]
pub struct LogGuard {
    /// `None` when no file sink could be opened (e.g. read-only filesystem,
    /// platform with no XDG state directory).
    _file_worker: Option<WorkerGuard>,
}

/// Initialize the global `tracing` subscriber.
///
/// Wires a compact stderr layer at the [`LogLevel`]-dependent stderr level
/// and (when `log_dir` resolves) a structured rolling-file layer at the
/// file level. `RUST_LOG`, when set, supersedes both default directives.
///
/// `log_dir`:
/// * `Some(path)` — write `gitlore.log` under `path` (created if missing).
/// * `None`       — resolve via XDG (`directories::ProjectDirs`); skip the
///   file sink if that yields nothing.
///
/// Returns the [`LogGuard`] owning the non-blocking writer's flush handle.
///
/// # Errors
///
/// Surfaces [`io::Error`] when the log directory cannot be created or the
/// initial log file cannot be opened. Failures from the global-subscriber
/// install are swallowed (a previously installed subscriber wins; the
/// returned guard is harmless in that case).
pub fn init_logging(level: LogLevel, log_dir: Option<&Path>) -> io::Result<LogGuard> {
    let resolved_dir: Option<PathBuf> = match log_dir {
        Some(p) => Some(p.to_path_buf()),
        None => xdg_log_dir(),
    };

    let stderr_layer = fmt::layer()
        .compact()
        .with_target(false)
        .with_writer(io::stderr)
        .with_filter(default_filter(level.stderr_level()));

    let (file_layer, file_worker) = match resolved_dir {
        Some(dir) => match build_file_layer(&dir, level.file_level()) {
            Ok((layer, worker)) => (Some(layer), Some(worker)),
            // Surface the error so callers can decide; we do not silently
            // fall back to stderr-only when the user explicitly asked for a
            // file sink. Returning here keeps the contract honest.
            Err(e) => return Err(e),
        },
        None => (None, None),
    };

    // Attach the boxed `Option<Box<dyn Layer<Registry>>>` *first* so the
    // type parameter `S` on the boxed layer stays bound to `Registry`.
    // Attaching `stderr_layer` first instead produces `Layered<..., Registry>`
    // which the boxed layer does not satisfy as `Layer<S>`.
    let _ = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    Ok(LogGuard {
        _file_worker: file_worker,
    })
}

/// Build the file layer (boxed so the `with`/`with` types match across
/// the `Some`/`None` arms).
fn build_file_layer(
    dir: &Path,
    file_level: Level,
) -> io::Result<(
    Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync + 'static>,
    WorkerGuard,
)> {
    fs::create_dir_all(dir)?;
    let writer = SizeRollingWriter::new(
        dir.to_path_buf(),
        LOG_FILE_NAME.to_string(),
        FILE_SIZE_LIMIT_BYTES,
        FILE_RETENTION,
    )?;
    let (non_blocking, worker) = tracing_appender::non_blocking(writer);
    let layer = fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .with_writer(non_blocking)
        .with_filter(default_filter(file_level))
        .boxed();
    Ok((layer, worker))
}

/// Build an `EnvFilter` whose default directive is `default_level` and
/// which honors `RUST_LOG` when set.
///
/// `from_env_lossy` swallows malformed `RUST_LOG` directives (logged on
/// stderr by the subscriber itself); the default directive remains the
/// safety net.
fn default_filter(default_level: Level) -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(default_level.into())
        .with_env_var("RUST_LOG")
        .from_env_lossy()
}

/// XDG fallback for the log directory. Mirrors the layout used by the
/// per-user index storage (Q15b, ADR-029) so a one-off `gitlore --version`
/// or `gitlore search ... --json` invocation outside any repo still has a
/// stable place to write structured logs.
///
/// Returns `None` only when `directories` cannot produce *any* user-scoped
/// path at all (extremely rare; missing `$HOME`).
fn xdg_log_dir() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from(XDG_QUALIFIER, XDG_ORGANIZATION, XDG_APPLICATION)?;
    // Prefer `state_dir` (Linux/BSD: `$XDG_STATE_HOME/gitlore`). macOS and
    // Windows expose `None` here, so fall back to `data_local_dir` which is
    // always present on every supported platform.
    let base = proj
        .state_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| proj.data_local_dir().to_path_buf());
    Some(base)
}

// ---------------------------------------------------------------------------
// SizeRollingWriter
//
// tracing-appender 0.2 only supports time-based rotation (`Rotation::DAILY`
// and friends), but the spec requires size-based rotation at 10 MiB with 3
// retained generations. The implementation below is the smallest writer that
// satisfies that contract: it owns one `File`, tracks bytes written into the
// current rotation, and renames `gitlore.log` -> `gitlore.log.1` (shifting
// older files down one slot) when a write would push it past the cap.
//
// The writer is fed into `tracing_appender::non_blocking`, which moves it
// onto a dedicated background thread, so the rotation cost (one rename per
// 10 MiB written) never blocks the foreground tasks.
// ---------------------------------------------------------------------------

/// Append-only file writer that rotates when the active file would exceed
/// `max_bytes`. Keeps `retention` historical rotations.
struct SizeRollingWriter {
    dir: PathBuf,
    base_name: String,
    max_bytes: u64,
    retention: usize,
    file: File,
    bytes_written: u64,
}

impl SizeRollingWriter {
    fn new(dir: PathBuf, base_name: String, max_bytes: u64, retention: usize) -> io::Result<Self> {
        let path = dir.join(&base_name);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let bytes_written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            dir,
            base_name,
            max_bytes,
            retention,
            file,
            bytes_written,
        })
    }

    /// Close the active file, shift `gitlore.log.{N-1}` -> `gitlore.log.{N}`
    /// for every retained slot (deleting the oldest), then rename the
    /// just-closed `gitlore.log` to `gitlore.log.1` and reopen a fresh
    /// `gitlore.log`. Bytes-written counter resets to zero.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush().ok();

        // Delete the oldest retained slot, if it exists. This bounds the
        // total on-disk footprint to (retention + 1) * max_bytes.
        let oldest = self
            .dir
            .join(format!("{}.{}", self.base_name, self.retention));
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }

        // Shift every middle slot down by one: .{N-1} -> .{N}.
        for i in (1..self.retention).rev() {
            let from = self.dir.join(format!("{}.{}", self.base_name, i));
            let to = self.dir.join(format!("{}.{}", self.base_name, i + 1));
            if from.exists() {
                fs::rename(&from, &to)?;
            }
        }

        // Promote the active file to slot .1, then open a fresh active file.
        let active = self.dir.join(&self.base_name);
        if active.exists() {
            let dot_one = self.dir.join(format!("{}.1", self.base_name));
            fs::rename(&active, &dot_one)?;
        }
        self.file = OpenOptions::new().create(true).append(true).open(&active)?;
        self.bytes_written = 0;
        Ok(())
    }
}

impl Write for SizeRollingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Rotate *before* a write would put us over the cap, never on the
        // first byte (`bytes_written > 0`) so the cap behaves as "max file
        // size", not "max single-record size".
        if self.bytes_written > 0
            && self.bytes_written.saturating_add(buf.len() as u64) > self.max_bytes
        {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.bytes_written = self.bytes_written.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ---- LogLevel mapping ---------------------------------------------------

    #[test]
    fn loglevel_quiet_maps_error_stderr_info_file() {
        assert_eq!(LogLevel::Quiet.stderr_level(), Level::ERROR);
        assert_eq!(LogLevel::Quiet.file_level(), Level::INFO);
    }

    #[test]
    fn loglevel_normal_maps_warn_stderr_info_file() {
        assert_eq!(LogLevel::Normal.stderr_level(), Level::WARN);
        assert_eq!(LogLevel::Normal.file_level(), Level::INFO);
    }

    #[test]
    fn loglevel_verbose_maps_info_stderr_debug_file() {
        assert_eq!(LogLevel::Verbose.stderr_level(), Level::INFO);
        assert_eq!(LogLevel::Verbose.file_level(), Level::DEBUG);
    }

    #[test]
    fn loglevel_default_is_normal() {
        assert_eq!(LogLevel::default(), LogLevel::Normal);
    }

    // ---- SizeRollingWriter --------------------------------------------------

    fn writer_for(dir: &Path, max_bytes: u64, retention: usize) -> SizeRollingWriter {
        SizeRollingWriter::new(
            dir.to_path_buf(),
            LOG_FILE_NAME.to_string(),
            max_bytes,
            retention,
        )
        .expect("open writer")
    }

    #[test]
    fn writes_below_threshold_stay_in_active_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = writer_for(tmp.path(), 1024, 3);
        w.write_all(&[b'a'; 100]).unwrap();
        w.flush().unwrap();

        let active = tmp.path().join(LOG_FILE_NAME);
        assert!(active.exists());
        assert_eq!(fs::metadata(&active).unwrap().len(), 100);
        assert!(!tmp.path().join(format!("{LOG_FILE_NAME}.1")).exists());
    }

    #[test]
    fn write_crossing_threshold_rotates_to_slot_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = writer_for(tmp.path(), 100, 3);

        // First write fills slot 0 to 80 bytes (< cap; no rotation yet).
        w.write_all(&[b'a'; 80]).unwrap();
        w.flush().unwrap();
        assert_eq!(
            fs::metadata(tmp.path().join(LOG_FILE_NAME)).unwrap().len(),
            80
        );
        assert!(!tmp.path().join(format!("{LOG_FILE_NAME}.1")).exists());

        // Second write would push past cap (80 + 80 > 100) -> rotates first,
        // then writes into the fresh active file.
        w.write_all(&[b'b'; 80]).unwrap();
        w.flush().unwrap();

        let dot_one = tmp.path().join(format!("{LOG_FILE_NAME}.1"));
        assert!(dot_one.exists(), "rotation must produce .1");
        assert_eq!(fs::metadata(&dot_one).unwrap().len(), 80);
        assert_eq!(
            fs::metadata(tmp.path().join(LOG_FILE_NAME)).unwrap().len(),
            80
        );
        assert_eq!(fs::read(&dot_one).unwrap(), vec![b'a'; 80]);
    }

    #[test]
    fn retention_caps_historical_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = writer_for(tmp.path(), 10, 3);

        // Each `write_all` for 10 bytes is exactly the cap; the *next*
        // write of any size triggers a rotation. We write 5 rounds of 10
        // bytes so we exercise 4 rotations -> .1/.2/.3 occupied and the
        // first batch gets evicted past the retention cap.
        for round in 0..5u8 {
            w.write_all(&[b'0' + round; 10]).unwrap();
            w.flush().unwrap();
        }

        // Active + .1 + .2 + .3 exist; nothing else.
        for slot in 1..=3 {
            let p = tmp.path().join(format!("{LOG_FILE_NAME}.{slot}"));
            assert!(p.exists(), "slot .{slot} must exist after 4 rotations");
        }
        assert!(!tmp.path().join(format!("{LOG_FILE_NAME}.4")).exists());

        // Oldest content (round 0 = b'0') has been evicted; .3 holds round 1.
        let dot_three = tmp.path().join(format!("{LOG_FILE_NAME}.3"));
        assert_eq!(fs::read(&dot_three).unwrap(), vec![b'1'; 10]);
    }

    #[test]
    fn reopen_preserves_existing_bytes_written_offset() {
        let tmp = tempfile::tempdir().unwrap();

        // Pre-existing content in the active file.
        {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(tmp.path().join(LOG_FILE_NAME))
                .unwrap();
            f.write_all(&[b'x'; 60]).unwrap();
        }

        let mut w = writer_for(tmp.path(), 100, 3);
        // 60 bytes already on disk; writing 50 more crosses the 100-byte cap.
        w.write_all(&[b'y'; 50]).unwrap();
        w.flush().unwrap();

        let dot_one = tmp.path().join(format!("{LOG_FILE_NAME}.1"));
        assert!(dot_one.exists(), "rotation must see pre-existing bytes");
        assert_eq!(fs::read(&dot_one).unwrap(), vec![b'x'; 60]);
        assert_eq!(
            fs::read(tmp.path().join(LOG_FILE_NAME)).unwrap(),
            vec![b'y'; 50]
        );
    }

    #[test]
    fn first_write_never_rotates_even_when_oversized() {
        // A single record larger than `max_bytes` must not rotate an empty
        // active file (otherwise we would create an empty .1 ad infinitum).
        let tmp = tempfile::tempdir().unwrap();
        let mut w = writer_for(tmp.path(), 10, 3);
        w.write_all(&[b'z'; 200]).unwrap();
        w.flush().unwrap();

        assert!(!tmp.path().join(format!("{LOG_FILE_NAME}.1")).exists());
        assert_eq!(
            fs::metadata(tmp.path().join(LOG_FILE_NAME)).unwrap().len(),
            200
        );
    }

    // ---- init_logging end-to-end -------------------------------------------

    #[test]
    fn init_logging_creates_log_dir_and_returns_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested").join("logs");
        assert!(
            !dir.exists(),
            "test precondition: dir must be created by init"
        );

        let _guard = init_logging(LogLevel::Normal, Some(&dir)).expect("init_logging");

        assert!(dir.exists(), "init_logging must create the log dir");
        assert!(
            dir.join(LOG_FILE_NAME).exists(),
            "init_logging must open the active log file"
        );
    }

    #[test]
    fn xdg_log_dir_resolves_when_home_present() {
        // `directories` resolves to *something* on every platform CI runs on
        // (Linux state_dir, macOS Application Support, Windows AppData) as
        // long as `$HOME` (or equivalent) is set, which it always is in
        // `cargo test`. Treat any `Some` result as success; the concrete
        // path is platform-specific and not worth pinning here.
        let dir = xdg_log_dir().expect("XDG fallback must resolve under cargo test");
        assert!(
            dir.ends_with("gitlore"),
            "XDG fallback must terminate in `gitlore/`, got: {}",
            dir.display()
        );
    }
}
