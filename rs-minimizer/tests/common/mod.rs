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
    let out = if compiled.status.success() { run_with_timeout(&bin, Duration::from_secs(10)) } else { None };
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

/// Stdout of the program when it exits successfully within `limit`.
fn run_with_timeout(bin: &std::path::Path, limit: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = Command::new(bin).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(st) => break st,
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let text = reader.join().ok()?;
    status.success().then_some(text)
}
