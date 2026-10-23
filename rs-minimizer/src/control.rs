//! Process-wide state of a reduction run: Ctrl+C, parallelism, the on-disk checkpoint of the best program and the progress
//! counters. One reduction runs at a time per process.

use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static DEADLINE_HIT: AtomicBool = AtomicBool::new(false);

/// Set by Ctrl+C (see `install_ctrlc`): every pass stops at its next deadline check.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

static EFFORT: AtomicUsize = AtomicUsize::new(0);

/// How hard the passes try: 0 is the normal run, every further level adds slower and less obvious variants (random subsets instead
/// of neighbours, bigger token windows, pairs of statements, more replacement values). A caller with time left raises it instead of
/// giving up.
pub fn set_effort(level: usize) {
    EFFORT.store(level, Ordering::Relaxed);
}

pub fn effort() -> usize {
    EFFORT.load(Ordering::Relaxed)
}

/// Deterministic shuffle (the same level always gives the same order): xorshift on the seed.
pub fn shuffled<T>(items: &mut [T], seed: u64) {
    let mut x = seed.wrapping_mul(0x9e3779b97f4a7c15) | 1;
    for i in (1..items.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        items.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

static PARALLEL: AtomicUsize = AtomicUsize::new(0);

/// Parallel oracle calls of the passes (and Shrink Ray's workers).
pub fn set_parallel(n: usize) {
    PARALLEL.store(n.max(1), Ordering::Relaxed);
}

pub fn par() -> usize {
    match PARALLEL.load(Ordering::Relaxed) {
        // never set: all the cores there are
        0 => cores(),
        n => n,
    }
}

pub fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

fn meminfo_gb(key: &str) -> Option<f64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: f64 = text.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0 / 1024.0)
}

/// What the run has to work with: cores, memory, load, and how many oracle calls and external workers run at once.
pub fn describe_system() -> String {
    let load = std::fs::read_to_string("/proc/loadavg").ok().and_then(|t| t.split_whitespace().next().map(String::from)).unwrap_or_else(|| "?".into());
    format!(
        "{} cores, {:.1} GB RAM ({:.1} GB free), load {load}; {} oracle calls at once (each starts rustc, which is multi-threaded itself), {} workers for Shrink Ray, {} for treereduce-rust",
        cores(),
        meminfo_gb("MemTotal:").unwrap_or(0.0),
        meminfo_gb("MemAvailable:").unwrap_or(0.0),
        par_checks(),
        par(),
        par()
    )
}

/// Candidates evaluated at once by the built-in passes: most oracle calls stop after one compilation (a candidate that fails there is
/// not compiled a second time), and rustc itself is multi-threaded, so a bit more than half of the jobs.
pub fn par_checks() -> usize {
    (par() * 3 / 4).max(1)
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

pub fn expired(deadline: Instant) -> bool {
    let e = interrupted() || Instant::now() >= deadline;
    if e {
        DEADLINE_HIT.store(true, Ordering::Relaxed);
    }
    e
}

/// Called by the external reducers that stop at a deadline.
pub fn mark_deadline_hit() {
    DEADLINE_HIT.store(true, Ordering::Relaxed);
}

/// Forgets an earlier deadline hit: call before a new reduction.
pub fn reset_deadline_flag() {
    DEADLINE_HIT.store(false, Ordering::Relaxed);
}

/// True when the last reduction finished every stage before its deadlines (so its result is a local minimum of all passes).
pub fn last_run_complete() -> bool {
    !DEADLINE_HIT.load(Ordering::Relaxed)
}

/// First Ctrl+C: stop reducing and write everything up; second Ctrl+C: leave at once (the checkpoint is on disk).
pub fn install_ctrlc() {
    let res = ctrlc::set_handler(|| {
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            warn!("Ctrl+C again - leaving at once (the checkpoint holds the best program found so far)");
            std::process::exit(130);
        }
        warn!("Ctrl+C caught - finishing the running oracle calls, then writing up (press Ctrl+C again to leave at once)");
    });
    if let Err(e) = res {
        error!("cannot install the Ctrl+C handler: {e}");
    }
}

/// The smallest program seen so far that still shows the bug, and where to keep a copy of it on disk.
static BEST: Mutex<Option<String>> = Mutex::new(None);
static CHECKPOINT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Every time the oracle accepts a program that is smaller than the best one so far it is written to `path` (atomically), so
/// that a killed run loses nothing. `None` switches this off.
pub fn set_checkpoint(path: Option<PathBuf>) {
    *BEST.lock().unwrap() = None;
    *CHECKPOINT.lock().unwrap() = path;
}

/// The best (smallest) interesting program recorded since `set_checkpoint`.
pub fn best_so_far() -> Option<String> {
    BEST.lock().unwrap().clone()
}

fn write_atomically(path: &std::path::Path, src: &str) {
    let tmp = path.with_extension("rs.tmp");
    if std::fs::write(&tmp, src).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// An accepted program that is bigger than the best one (macros expanded): it becomes the baseline and is written to disk, so
/// that the file always holds the form every later step works on.
pub fn rebase_checkpoint(src: &str) {
    let Some(path) = CHECKPOINT.lock().unwrap().clone() else { return };
    write_atomically(&path, src);
    *BEST.lock().unwrap() = Some(src.to_string());
    info!("[{}] saved as the new baseline (a bigger but simpler program): {} lines, {} bytes ({})", stage(), src.lines().count(), src.len(), path.display());
}

static LAST_LOG: Mutex<Option<Instant>> = Mutex::new(None);
static CLOCK: Mutex<Option<Instant>> = Mutex::new(None);

/// Starts the clock that the attempt rate in the improvement lines is measured against.
pub fn start_clock() {
    *CLOCK.lock().unwrap() = Some(Instant::now());
}

/// `attempt 125, 3.4/s`: how many oracle calls were made so far and how fast they go.
fn attempts() -> String {
    let runs = ORACLE_RUNS.load(Ordering::Relaxed);
    let secs = CLOCK.lock().unwrap().map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0).max(0.001);
    format!("attempt {runs}, {:.1}/s", runs as f64 / secs)
}

static STAGE: Mutex<String> = Mutex::new(String::new());

/// Name of the pass that is running, shown in front of every improvement it finds.
pub fn set_stage(name: &str) {
    *STAGE.lock().unwrap() = name.to_string();
}

fn stage() -> String {
    STAGE.lock().unwrap().clone()
}

fn note_interesting(src: &str) {
    let Some(path) = CHECKPOINT.lock().unwrap().clone() else { return };
    let mut best = BEST.lock().unwrap();
    if best.as_ref().map(|b| src.len() < b.len()).unwrap_or(true) {
        // a pass that finds many tiny improvements would flood the log: a line when it matters (1% of the program) or every 3 seconds
        let mut last = LAST_LOG.lock().unwrap();
        let big = best.as_ref().is_none_or(|b| (b.len() - src.len()) * 100 >= b.len());
        if big || last.is_none_or(|t| t.elapsed() >= Duration::from_secs(3)) {
            *last = Some(Instant::now());
            match best.as_ref() {
                Some(b) => info!("[{}] smaller program: {} -> {} lines, {} -> {} bytes ({}; saved to {})", stage(), b.lines().count(), src.lines().count(), b.len(), src.len(), attempts(), path.display()),
                None => info!("[{}] first interesting program: {} lines, {} bytes ({}; saved to {})", stage(), src.lines().count(), src.len(), attempts(), path.display()),
            }
        }
        write_atomically(&path, src);
        *best = Some(src.to_string());
    }
}

/// Used by external reducers that find smaller programs on their own (Shrink Ray rewrites a file we poll).
pub fn note_external_result(src: &str) {
    note_interesting(src);
}

static ORACLE_RUNS: AtomicUsize = AtomicUsize::new(0);
static ORACLE_HITS: AtomicUsize = AtomicUsize::new(0);
static ORACLE_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static EXTERNAL_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn set_external_running(on: bool) {
    EXTERNAL_RUNNING.store(on, Ordering::Relaxed);
}

/// Oracle calls that really compiled something so far.
pub fn oracle_runs() -> usize {
    ORACLE_RUNS.load(Ordering::Relaxed)
}

/// Bracket every uncached oracle call with these two (the `Oracle` implementations do).
pub fn oracle_started() {
    ORACLE_ACTIVE.fetch_add(1, Ordering::Relaxed);
}

pub fn oracle_finished(src: &str, interesting: bool) {
    ORACLE_ACTIVE.fetch_sub(1, Ordering::Relaxed);
    ORACLE_RUNS.fetch_add(1, Ordering::Relaxed);
    if interesting {
        ORACLE_HITS.fetch_add(1, Ordering::Relaxed);
        note_interesting(src);
    }
}

/// Logs a one-line sign of life every `every` until the process ends, so a long silent pass is visibly working.
pub fn start_heartbeat(every: Duration) {
    let start = Instant::now();
    thread::spawn(move || {
        let mut last_runs = 0;
        loop {
            thread::sleep(every);
            let runs = ORACLE_RUNS.load(Ordering::Relaxed);
            let best = BEST.lock().unwrap().as_ref().map(|b| b.lines().count());
            info!(
                "alive {}s: {} oracle runs (+{} in the last {}s), {} interesting, {} running now{}, best so far {}",
                start.elapsed().as_secs(),
                runs,
                runs - last_runs,
                every.as_secs(),
                ORACLE_HITS.load(Ordering::Relaxed),
                ORACLE_ACTIVE.load(Ordering::Relaxed),
                if EXTERNAL_RUNNING.load(Ordering::Relaxed) { " (+ the external reducer's own workers, not counted)" } else { "" },
                best.map(|l| format!("{l} lines")).unwrap_or_else(|| "(nothing yet)".into()),
            );
            last_runs = runs;
        }
    });
}
