use crate::control;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Decides whether a candidate program still shows the problem being reduced (miscompilation, ICE, a lint, ...).
pub trait Oracle: Sync {
    fn check(&self, src: &str) -> bool;

    /// The verdict without any cache: for checking whether the program is interesting every time (a flaky bug is not).
    fn recheck(&self, src: &str) -> bool {
        self.check(src)
    }

    /// One line on what the oracle sees in `src` (what each configuration did), for the log when a verdict is a surprise.
    fn explain(&self, _src: &str) -> String {
        String::new()
    }

    /// Scratch directory for the rustc runs of the passes.
    fn work_dir(&self) -> PathBuf {
        std::env::temp_dir().join("rs-minimizer")
    }

}

pub(crate) fn hash_text(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Wraps a plain predicate: caches results (the passes regenerate many identical candidates), counts runs for the progress
/// line and feeds the checkpoint.
pub struct Cached<F: Fn(&str) -> bool + Sync> {
    pub predicate: F,
    pub work: PathBuf,
    cache: Mutex<HashMap<u64, bool>>,
}

impl<F: Fn(&str) -> bool + Sync> Cached<F> {
    pub fn new(predicate: F) -> Self {
        Cached { predicate, work: std::env::temp_dir().join("rs-minimizer"), cache: Mutex::new(HashMap::new()) }
    }
}

impl<F: Fn(&str) -> bool + Sync> Oracle for Cached<F> {
    fn check(&self, src: &str) -> bool {
        let key = hash_text(src);
        if let Some(&r) = self.cache.lock().unwrap().get(&key) {
            return r;
        }
        control::oracle_started();
        let r = (self.predicate)(src);
        control::oracle_finished(src, r);
        self.cache.lock().unwrap().insert(key, r);
        r
    }

    fn work_dir(&self) -> PathBuf {
        self.work.clone()
    }

}

static SCRIPT_RUNS: AtomicUsize = AtomicUsize::new(0);

/// The usual reducer contract: a command that is given the path of a candidate file as its last argument and exits with 0 when
/// the candidate is interesting.
pub fn script_predicate(command: Vec<String>, work: PathBuf) -> impl Fn(&str) -> bool + Sync {
    move |src: &str| {
        let n = SCRIPT_RUNS.fetch_add(1, Ordering::Relaxed);
        let dir = work.join(format!("script{}_{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("main.rs");
        let ok = std::fs::write(&file, src).is_ok()
            && Command::new(&command[0]).args(&command[1..]).arg(&file).current_dir(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        let _ = std::fs::remove_dir_all(&dir);
        ok
    }
}
