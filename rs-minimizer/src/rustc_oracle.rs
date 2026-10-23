//! Ready-made oracles for the things people want to reduce: a compiler crash (ICE, LLVM error, signal), a compile error, a hang,
//! an out-of-memory compilation, a miscompilation (two sets of flags give different output) or a program whose output or panic
//! has to stay. `detect` finds out which of them a file is, so that `reduce FILE` needs no script.

use crate::control;
use crate::oracle::{Cached, Oracle};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// the compiler crashes with this signature (see `crash_signature`)
    Crash { signature: String },
    /// compilation fails (not an ICE) and the compiler output contains `needle`
    Error { needle: String },
    /// compilation takes longer than the limit (seconds)
    Hang { secs: u64 },
    /// the compiler's resident memory exceeds the limit (MB)
    Oom { mb: u64 },
    /// both flag sets compile and run, but stdout or the exit code differ
    Differ { other_flags: Vec<String> },
    /// the program compiles and its stdout (`stdout`) or stderr contains `needle`
    Output { stdout: bool, needle: String },
}

#[derive(Clone, Debug)]
pub struct Spec {
    /// compiler command, e.g. `["rustc", "+nightly"]`
    pub rustc: Vec<String>,
    /// flags of the (first) build
    pub flags: Vec<String>,
    pub mode: Mode,
    pub compile_timeout: Duration,
    pub run_timeout: Duration,
}

static N: AtomicUsize = AtomicUsize::new(0);

pub struct Exec {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// the resident memory limit was exceeded and the process was killed
    pub over_memory: bool,
}

fn peak_rss_mb(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// Runs a command, kills it after `timeout` or when its resident memory passes `mem_mb`.
pub fn exec(mut cmd: Command, timeout: Duration, mem_mb: Option<u64>) -> std::io::Result<Exec> {
    use std::os::unix::process::ExitStatusExt;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if buf.len() < 4 << 20 {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let start = Instant::now();
    let (mut timed_out, mut over_memory) = (false, false);
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > timeout {
            timed_out = true;
        }
        if let Some(limit) = mem_mb {
            if peak_rss_mb(child.id()).is_some_and(|m| m > limit) {
                over_memory = true;
            }
        }
        if timed_out || over_memory || control::interrupted() {
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    Ok(Exec { code: status.code(), signal: status.signal(), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default(), timed_out, over_memory })
}

const CRASH_MARKERS: [&str; 5] = ["internal compiler error", "the compiler unexpectedly panicked", "rustc-LLVM ERROR", "LLVM ERROR", "error: rustc interrupted by"];

fn normalise(line: &str) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    for c in line.chars() {
        match c {
            '`' | '\'' => {
                in_quote = !in_quote;
                out.push('`');
            }
            _ if in_quote => {}
            c if c.is_ascii_digit() => {
                if !out.ends_with('N') {
                    out.push('N');
                }
            }
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The fingerprint of a crash: the marker line with numbers and quoted names removed, plus the compiler source file the panic names
/// (without line and column).
pub fn crash_signature(stderr: &str, signal: Option<i32>) -> Option<String> {
    let marker = stderr.lines().find(|l| CRASH_MARKERS.iter().any(|m| l.contains(m)));
    let panic = stderr.lines().find(|l| l.contains("panicked at"));
    let location = panic.and_then(|l| l.split("panicked at").nth(1)).map(|r| r.trim().split(':').next().unwrap_or("").to_string()).filter(|p| p.ends_with(".rs"));
    match (marker, signal) {
        (Some(m), _) => Some(format!("{}{}", normalise(m), location.map(|l| format!(" @ {l}")).unwrap_or_default())),
        (None, Some(sig)) if sig != 9 && sig != 15 => Some(format!("killed by signal {sig}")),
        _ => None,
    }
}

/// First error of a failed compilation: its code if it has one, else the message.
/// The needle has to be in the first error: a reduction that breaks the syntax and keeps the original error as a side effect is not what was asked for.
fn first_error_line(stderr: &str) -> Option<&str> {
    stderr.lines().find(|l| l.starts_with("error"))
}

fn first_error(stderr: &str) -> Option<String> {
    let line = stderr.lines().find(|l| l.starts_with("error"))?;
    if let Some(code) = line.strip_prefix("error[").and_then(|r| r.split(']').next()) {
        return Some(code.to_string());
    }
    Some(normalise(line.trim_start_matches("error: ")))
}

fn has_edition(flags: &[String]) -> bool {
    flags.iter().any(|f| f.starts_with("--edition"))
}

struct Built {
    compile: Exec,
    bin: PathBuf,
}

impl Spec {
    fn compile(&self, src_path: &Path, dir: &Path, flags: &[String], tag: &str, mem_mb: Option<u64>) -> std::io::Result<Built> {
        let bin = dir.join(format!("bin_{tag}"));
        let mut cmd = Command::new(&self.rustc[0]);
        cmd.args(&self.rustc[1..]).arg(src_path).arg("-o").arg(&bin).current_dir(dir);
        if !has_edition(flags) && !has_edition(&self.rustc) {
            cmd.args(["--edition", "2021"]);
        }
        let text = std::fs::read_to_string(src_path).unwrap_or_default();
        if !text.contains("fn main") && !flags.iter().any(|f| f.starts_with("--crate-type")) {
            cmd.args(["--crate-type", "lib"]);
        }
        cmd.args(flags);
        Ok(Built { compile: exec(cmd, self.compile_timeout, mem_mb)?, bin })
    }

    fn run(&self, bin: &Path) -> std::io::Result<Exec> {
        exec(Command::new(bin), self.run_timeout, None)
    }

    /// Does `src` still show the problem?
    pub fn check(&self, src: &str) -> bool {
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rs-minimizer-oracle-{}-{id}", std::process::id()));
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let path = dir.join("main.rs");
        let verdict = std::fs::write(&path, src).is_ok() && self.verdict(&path, &dir).unwrap_or(false);
        let _ = std::fs::remove_dir_all(&dir);
        verdict
    }

    fn verdict(&self, path: &Path, dir: &Path) -> std::io::Result<bool> {
        match &self.mode {
            Mode::Hang { .. } => Ok(self.compile(path, dir, &self.flags, "a", None)?.compile.timed_out),
            Mode::Oom { mb } => {
                let b = self.compile(path, dir, &self.flags, "a", Some(*mb))?;
                Ok(b.compile.over_memory || (b.compile.signal == Some(9) && !b.compile.timed_out))
            }
            Mode::Crash { signature } => {
                let b = self.compile(path, dir, &self.flags, "a", None)?;
                Ok(!b.compile.timed_out && crash_signature(&b.compile.stderr, b.compile.signal).as_deref() == Some(signature.as_str()))
            }
            Mode::Error { needle } => {
                let b = self.compile(path, dir, &self.flags, "a", None)?;
                let failed = b.compile.code != Some(0) && !b.compile.timed_out;
                Ok(failed && crash_signature(&b.compile.stderr, b.compile.signal).is_none() && first_error_line(&b.compile.stderr).is_some_and(|l| normalise(l).contains(needle.as_str()) || l.contains(needle.as_str())))
            }
            Mode::Output { stdout, needle } => {
                let b = self.compile(path, dir, &self.flags, "a", None)?;
                if b.compile.code != Some(0) {
                    return Ok(false);
                }
                let r = self.run(&b.bin)?;
                Ok(!r.timed_out && if *stdout { r.stdout.contains(needle.as_str()) } else { r.stderr.contains(needle.as_str()) })
            }
            Mode::Differ { other_flags } => {
                let a = self.compile(path, dir, &self.flags, "a", None)?;
                if a.compile.code != Some(0) || a.compile.timed_out {
                    return Ok(false);
                }
                let ra = self.run(&a.bin)?;
                if ra.timed_out {
                    return Ok(false);
                }
                let b = self.compile(path, dir, other_flags, "b", None)?;
                if b.compile.code != Some(0) || b.compile.timed_out {
                    return Ok(false);
                }
                let rb = self.run(&b.bin)?;
                Ok(!rb.timed_out && (ra.stdout != rb.stdout || ra.code != rb.code || ra.signal != rb.signal))
            }
        }
    }
}

/// What the file does, as a mode, and a sentence for the log. `None` when there is nothing to reduce (it compiles, runs and every
/// optimisation level agrees).
pub fn detect(spec: &Spec, src: &str) -> Option<(Mode, String)> {
    let id = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rs-minimizer-detect-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("main.rs");
    std::fs::write(&path, src).ok()?;
    let result = detect_in(spec, &path, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn detect_in(spec: &Spec, path: &Path, dir: &Path) -> Option<(Mode, String)> {
    let a = spec.compile(path, dir, &spec.flags, "a", None).ok()?;
    if a.compile.timed_out {
        let secs = spec.compile_timeout.as_secs();
        return Some((Mode::Hang { secs }, format!("the compilation does not finish in {secs}s")));
    }
    if let Some(sig) = crash_signature(&a.compile.stderr, a.compile.signal) {
        return Some((Mode::Crash { signature: sig.clone() }, format!("the compiler crashes: {sig}")));
    }
    if a.compile.code != Some(0) {
        let needle = first_error(&a.compile.stderr)?;
        return Some((Mode::Error { needle: needle.clone() }, format!("the compilation fails with `{needle}`")));
    }
    let ra = spec.run(&a.bin).ok()?;
    // does an optimisation level change what the program does?
    for level in ["1", "2", "3", "s", "z"] {
        let mut flags: Vec<String> = spec.flags.iter().filter(|f| !f.starts_with("opt-level") && !f.starts_with("-Copt-level")).cloned().collect();
        flags.extend(["-C".to_string(), format!("opt-level={level}")]);
        let mut base = spec.flags.clone();
        base.extend(["-C".to_string(), "opt-level=0".to_string()]);
        let (Ok(b0), Ok(b1)) = (spec.compile(path, dir, &base, "o0", None), spec.compile(path, dir, &flags, "o", None)) else { continue };
        if b0.compile.code != Some(0) || b1.compile.code != Some(0) {
            continue;
        }
        let (Ok(r0), Ok(r1)) = (spec.run(&b0.bin), spec.run(&b1.bin)) else { continue };
        if !r0.timed_out && !r1.timed_out && (r0.stdout != r1.stdout || r0.code != r1.code || r0.signal != r1.signal) {
            return Some((Mode::Differ { other_flags: flags }, format!("the program behaves differently at -C opt-level=0 and -C opt-level={level} (output or exit code)")));
        }
    }
    if ra.code != Some(0) || ra.signal.is_some() {
        let line = ra.stderr.lines().find(|l| l.contains("panicked") || !l.trim().is_empty()).unwrap_or("").to_string();
        let needle = line.split("panicked at").last().unwrap_or(&line).trim().split(':').last().unwrap_or("").trim().to_string();
        if !needle.is_empty() {
            return Some((Mode::Output { stdout: false, needle: needle.clone() }, format!("the program fails when run, its stderr says `{needle}`")));
        }
    }
    None
}

/// The oracle for the reducer.
pub fn oracle(spec: Spec, work: PathBuf) -> Cached<impl Fn(&str) -> bool + Sync> {
    let mut o = Cached::new(move |src: &str| spec.check(src));
    o.work = work;
    o
}

impl Oracle for Spec {
    fn check(&self, src: &str) -> bool {
        Spec::check(self, src)
    }
}
