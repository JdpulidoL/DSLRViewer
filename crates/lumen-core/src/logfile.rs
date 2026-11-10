//! Plain-text diagnostic log (`logs/lumen.log`) that a developer - or an AI agent - can tail while the app runs.
//!
//! Everything the engine reports plus the raw camera event stream goes here. The previous run is kept as
//! `lumen.prev.log`. Override the location with `LUMEN_LOG=<file>`.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Sink {
    file: File,
    started: Instant,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Repo-root `logs/lumen.log` (this crate sits two levels below the workspace root).
fn default_path() -> PathBuf {
    std::env::var_os("LUMEN_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../logs").join("lumen.log"))
}

/// Open the log (rotating the previous one). Safe to call more than once; failures just disable logging.
pub fn init() {
    init_at(default_path());
}

/// Same as [`init`] with an explicit file; Android points it at the app's private files dir.
pub fn init_at(path: PathBuf) {
    SINK.get_or_init(|| {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::rename(&path, path.with_extension("prev.log"));
        let file = File::create(&path).or_else(|_| File::create(std::env::temp_dir().join("lumen.log"))).expect("no writable location for the log");
        Mutex::new(Sink { file, started: Instant::now() })
    });
    line("app", format!("Lumen {} started", env!("CARGO_PKG_VERSION")));
}

/// Append `[+seconds] tag: message`.
pub fn line(tag: &str, msg: impl AsRef<str>) {
    let Some(sink) = SINK.get() else { return };
    if let Ok(mut s) = sink.lock() {
        let t = s.started.elapsed().as_secs_f32();
        let _ = writeln!(s.file, "[{t:9.3}] {tag:<7} {}", msg.as_ref());
        let _ = s.file.flush();
    }
}
