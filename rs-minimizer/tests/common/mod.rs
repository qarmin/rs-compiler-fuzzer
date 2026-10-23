#![allow(dead_code)]

use rs_minimizer::{Cached, Oracle};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static N: AtomicUsize = AtomicUsize::new(0);

pub fn far() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

/// `Some(stdout)` of the program when it compiles and exits successfully.
pub fn run_program(src: &str) -> Option<String> {
    let dir = std::env::temp_dir().join(format!("rsmin-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join("main.rs");
    std::fs::write(&file, src).ok()?;
    let bin = dir.join("bin");
    let compiled = Command::new("rustc").args(["--edition", "2021", "-C", "opt-level=0", "-A", "warnings", "-o"]).arg(&bin).arg(&file).output().ok()?;
    let out = if compiled.status.success() { Command::new(&bin).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string()) } else { None };
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// "The program compiles and prints exactly `expected`": a behaviour that has nothing to do with compiler bugs, so the tests
/// stay valid after the bugs are fixed.
pub fn prints(expected: &'static str) -> Cached<impl Fn(&str) -> bool + Sync> {
    let mut o = Cached::new(move |src: &str| run_program(src).as_deref() == Some(expected));
    o.work = scratch();
    o
}

pub fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("rsmin-work-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn contains(needle: &'static str) -> Cached<impl Fn(&str) -> bool + Sync> {
    Cached::new(move |src: &str| src.contains(needle))
}

pub fn assert_prints(src: &str, expected: &str) {
    assert_eq!(run_program(src).as_deref(), Some(expected), "program does not print {expected:?}:\n{src}");
}

pub fn check_oracle(o: &dyn Oracle, src: &str) {
    assert!(o.check(src), "the oracle must accept the starting program:\n{src}");
}
