//! Running rustc and the produced binaries with timeouts, and comparing outcomes.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    pub name: String,
    pub flags: Vec<String>,
    /// Takes much longer to compile than the rest (fat LTO, one codegen unit...); left out by `--profile fast`.
    pub slow: bool,
}

impl Config {
    pub fn new(name: &str, flags: &str) -> Config {
        Config { name: name.to_string(), flags: flags.split_whitespace().map(String::from).collect(), slow: false }
    }

    pub fn slow(mut self) -> Config {
        self.slow = true;
        self
    }
}

/// Flags pinned identically for every configuration, so a difference can only come from the
/// flags deliberately varied by `Config`. (Later `-C` flags override earlier ones.)
pub const COMMON_FLAGS: &[&str] =
    &["--edition", "2021", "--cap-lints", "allow", "-C", "debug-assertions=off", "-C", "overflow-checks=off"];

static COMMON_OVERRIDE: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// Replace the pinned flags (used by `triage` after it found out which of them a finding needs);
/// `None` restores `COMMON_FLAGS`.
pub fn set_common_flags(flags: Option<Vec<String>>) {
    *COMMON_OVERRIDE.lock().unwrap() = flags;
}

pub fn common_flags() -> Vec<String> {
    COMMON_OVERRIDE.lock().unwrap().clone().unwrap_or_else(|| COMMON_FLAGS.iter().map(|s| s.to_string()).collect())
}

pub struct Exec {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub timed_out: bool,
}

fn drain<R: Read + Send + 'static>(mut r: R, limit: usize) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.len() < limit {
                        out.extend_from_slice(&buf[..n.min(limit - out.len())]);
                    }
                }
            }
        }
        out
    })
}

pub fn exec(mut cmd: Command, timeout: Duration) -> std::io::Result<Exec> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let so = drain(child.stdout.take().unwrap(), 32 << 20);
    let se = drain(child.stderr.take().unwrap(), 256 << 10);
    let start = Instant::now();
    let (status, timed_out) = loop {
        if let Some(st) = child.try_wait()? {
            break (Some(st), false);
        }
        if start.elapsed() > timeout {
            // kill the whole process group (rustc spawns linkers etc.)
            let _ = Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).status();
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = so.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&se.join().unwrap_or_default()).into_owned();
    use std::os::unix::process::ExitStatusExt;
    Ok(Exec {
        code: status.and_then(|s| s.code()),
        signal: status.and_then(|s| s.signal()),
        stdout,
        stderr,
        timed_out,
    })
}

#[derive(Clone, Debug)]
pub enum Outcome {
    CompileFail(String),
    CompileTimeout,
    RunTimeout,
    Ran { code: Option<i32>, signal: Option<i32>, stdout: String, stderr: String },
}

impl Outcome {
    /// What counts as "the same behaviour" between two configurations.
    pub fn key(&self) -> String {
        match self {
            Outcome::CompileFail(_) => "compile-fail".into(),
            Outcome::CompileTimeout => "compile-timeout".into(),
            Outcome::RunTimeout => "run-timeout".into(),
            Outcome::Ran { code, signal, stdout, .. } => format!("exit={code:?} signal={signal:?}\n{stdout}"),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Outcome::CompileFail(_) => "compilation failed".into(),
            Outcome::CompileTimeout => "compilation timed out".into(),
            Outcome::RunTimeout => "execution timed out".into(),
            Outcome::Ran { code, signal, stdout, .. } => {
                format!("exit code {code:?}, signal {signal:?}, {} output lines", stdout.lines().count())
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub compile: Duration,
    pub run: Duration,
}

pub fn rustc_cmd(rustc: &[String]) -> Command {
    let mut c = Command::new(&rustc[0]);
    c.args(&rustc[1..]);
    c
}

pub fn compile_args(src: &Path, bin: &Path, cfg: &Config) -> Vec<String> {
    let mut a: Vec<String> = vec![src.display().to_string(), "-o".into(), bin.display().to_string()];
    a.extend(common_flags());
    a.extend(cfg.flags.iter().cloned());
    a
}

pub fn build_and_run(rustc: &[String], src: &Path, bin: &Path, cfg: &Config, lim: &Limits) -> Outcome {
    let mut c = rustc_cmd(rustc);
    c.args(compile_args(src, bin, cfg));
    let r = match exec(c, lim.compile) {
        Ok(r) => r,
        Err(e) => return Outcome::CompileFail(format!("cannot spawn rustc: {e}")),
    };
    if r.timed_out {
        return Outcome::CompileTimeout;
    }
    if r.code != Some(0) {
        return Outcome::CompileFail(r.stderr);
    }
    let run = match exec(Command::new(bin), lim.run) {
        Ok(r) => r,
        Err(e) => return Outcome::CompileFail(format!("cannot run binary: {e}")),
    };
    let _ = std::fs::remove_file(bin);
    if run.timed_out {
        return Outcome::RunTimeout;
    }
    Outcome::Ran {
        code: run.code,
        signal: run.signal,
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        stderr: run.stderr,
    }
}

/// Type/borrow-check only (cheap): returns rustc's stderr on failure.
pub fn check_only(rustc: &[String], src: &Path, out_dir: &Path, timeout: Duration) -> Result<(), String> {
    let mut c = rustc_cmd(rustc);
    c.args([src.display().to_string(), "--emit=metadata".into(), "--crate-type=bin".into(), "--crate-name=fuzz_check".into()]);
    c.arg("--out-dir").arg(out_dir);
    c.args(COMMON_FLAGS);
    match exec(c, timeout) {
        Ok(r) if r.timed_out => Err("check timed out".into()),
        Ok(r) if r.code == Some(0) => Ok(()),
        Ok(r) => Err(r.stderr),
        Err(e) => Err(format!("cannot spawn rustc: {e}")),
    }
}

/// Run `f` over `items` with at most `jobs` threads, results in input order.
pub fn par_map<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    use std::sync::Mutex;
    let next = Mutex::new(0usize);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    thread::scope(|s| {
        for _ in 0..jobs.max(1).min(items.len().max(1)) {
            s.spawn(|| loop {
                let i = {
                    let mut n = next.lock().unwrap();
                    let i = *n;
                    *n += 1;
                    i
                };
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                out.lock().unwrap()[i] = Some(r);
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

/// Stable fingerprint of a compiler crash, so the same crash found again (other batch, other
/// run) is recognised: the first line naming an LLVM error / ICE / signal, digits normalised.
pub fn crash_signature(o: &Outcome) -> Option<String> {
    let Outcome::CompileFail(e) = o else { return None };
    let line = e
        .lines()
        .find(|l| l.contains("LLVM ERROR") || l.contains("internal compiler error") || l.contains("interrupted by SIG") || l.contains("panicked at") || l.contains("Assertion"))
        .or_else(|| e.lines().find(|l| l.starts_with("error")))?;
    let norm: String = line.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    Some(norm.chars().take(160).collect())
}

