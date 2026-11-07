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
    /// The program is interpreted by Miri instead of being compiled (see `miri::run_program`); the name starts with `miri`.
    pub miri: bool,
}

impl Config {
    pub fn new(name: &str, flags: &str) -> Config {
        Config {
            name: name.to_string(),
            flags: flags.split_whitespace().map(String::from).collect(),
            slow: false,
            miri: name.starts_with("miri"),
        }
    }

    pub fn slow(mut self) -> Config {
        self.slow = true;
        self
    }
}

/// Flags pinned identically for every configuration, so a difference can only come from the
/// flags deliberately varied by `Config`. (Later `-C` flags override earlier ones.)
pub const COMMON_FLAGS: &[&str] = &[
    "--edition",
    "2021",
    "--cap-lints",
    "allow",
    "--force-warn",
    "function_casts_as_integer",
    "-C",
    "debug-assertions=off",
    "-C",
    "overflow-checks=off",
];

/// The address of a function differs on every run (ASLR), so a program that turns one into an integer is no test case. The lint is
/// forced to warn (`--cap-lints` cannot hide it) and a compilation that shows it counts as failed.
pub fn casts_function_address(stderr: &str) -> bool {
    stderr.contains("direct cast of function item")
        || stderr.contains("function_casts_as_integer") && stderr.contains("warning")
}

/// The pinned flags as they are shown in reports: without the lint that only guards the test cases.
pub fn shown_flags(flags: &[String]) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut it = flags.iter().map(String::as_str).peekable();
    while let Some(f) = it.next() {
        if f == "--force-warn" && it.peek() == Some(&"function_casts_as_integer") {
            it.next();
        } else {
            out.push(f);
        }
    }
    out.join(" ")
}

/// Every command in a report names the nightly toolchain: `-Z` options need it and the others behave the same, so one form fits all.
pub fn compiler_for(_flags: &str) -> &'static str {
    "rustc +nightly"
}

pub(crate) const FN_ADDRESS_MESSAGE: &str =
    "the program casts a function item into an integer: the address differs on every run";

static COMMON_OVERRIDE: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// Replace the pinned flags (used by `triage` after it found out which of them a finding needs);
/// `None` restores `COMMON_FLAGS`.
pub fn set_common_flags(flags: Option<Vec<String>>) {
    *COMMON_OVERRIDE.lock().unwrap() = flags;
}

pub fn common_flags() -> Vec<String> {
    COMMON_OVERRIDE
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| COMMON_FLAGS.iter().map(|s| s.to_string()).collect())
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

/// Resident memory (MB) one compiler or test process may use before it is killed; `RS_MEM_MB` overrides it. A reduction runs one
/// oracle per core, so a candidate that makes rustc or the program grow without bound must not take the machine down.
fn mem_limit_mb() -> u64 {
    static LIMIT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("RS_MEM_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4096)
    })
}

fn rss_mb(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kb: u64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1024)
}

unsafe extern "C" {
    fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
    fn setrlimit(resource: i32, rlim: *const [u64; 2]) -> i32;
}

pub fn exec(mut cmd: Command, timeout: Duration) -> std::io::Result<Exec> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // a test binary must not outlive the process that started it (a killed reducer left them spinning for the rest of the job):
    // PR_SET_PDEATHSIG = 1, SIGKILL = 9; RLIMIT_CORE = 4: crashing compilers must not leave core.* files behind
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            prctl(1, 9, 0, 0, 0);
            setrlimit(4, &[0, 0]);
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let so = drain(child.stdout.take().unwrap(), 32 << 20);
    let se = drain(child.stderr.take().unwrap(), 4 << 20);
    let start = Instant::now();
    let (status, timed_out) = loop {
        if let Some(st) = child.try_wait()? {
            break (Some(st), false);
        }
        if start.elapsed() > timeout || rss_mb(pid).is_some_and(|m| m > mem_limit_mb()) {
            // kill the whole process group (rustc spawns linkers etc.)
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .status();
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
    /// Miri refuses to run the program (undefined behaviour, an unsupported operation); the text is its stderr.
    MiriError(String),
    Ran {
        code: Option<i32>,
        signal: Option<i32>,
        stdout: String,
        stderr: String,
    },
}

impl Outcome {
    /// What counts as "the same behaviour" between two configurations.
    pub fn key(&self) -> String {
        match self {
            Outcome::CompileFail(_) => "compile-fail".into(),
            Outcome::CompileTimeout => "compile-timeout".into(),
            Outcome::RunTimeout => "run-timeout".into(),
            Outcome::MiriError(_) => "miri-error".into(),
            Outcome::Ran {
                code,
                signal,
                stdout,
                ..
            } => format!("exit={code:?} signal={signal:?}\n{stdout}"),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Outcome::CompileFail(_) => "compilation failed".into(),
            Outcome::CompileTimeout => "compilation timed out".into(),
            Outcome::RunTimeout => "execution timed out".into(),
            Outcome::MiriError(e) => format!("Miri reports: {}", crate::miri::headline(e)),
            Outcome::Ran {
                code,
                signal,
                stdout,
                ..
            } => {
                format!(
                    "exit code {code:?}, signal {signal:?}, {} output lines",
                    stdout.lines().count()
                )
            }
        }
    }
}

/// (overflow-checks, debug-assertions) as the configuration builds with them (the common flags, then the configuration's own).
fn checks_of(c: &Config) -> (bool, bool) {
    let (mut overflow, mut debug): (Option<bool>, bool) = (None, false);
    let flags: Vec<String> = common_flags()
        .into_iter()
        .chain(c.flags.iter().cloned())
        .collect();
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i].as_str();
        let arg = if f == "-C" {
            i += 1;
            flags.get(i).map(String::as_str).unwrap_or("")
        } else {
            f.strip_prefix("-C").unwrap_or("")
        };
        let on = |v: &str| matches!(v, "on" | "yes" | "y" | "true" | "");
        if let Some(v) = arg.strip_prefix("overflow-checks=") {
            overflow = Some(on(v));
        } else if let Some(v) = arg.strip_prefix("debug-assertions=") {
            debug = on(v);
        }
        i += 1;
    }
    (overflow.unwrap_or(debug), debug)
}

/// Two outcomes of two configurations "differ" when the output differs and the difference is not simply what the options mean:
/// a program that overflows (or breaks an `unsafe` precondition) panics when overflow checks / debug assertions are on and goes on
/// when they are off. That is no compiler bug, and a reducer finds such candidates very quickly (`i8::MAX + 1`).
pub fn differs(a: &Outcome, b: &Outcome, ca: &Config, cb: &Config) -> bool {
    if a.key() == b.key() {
        return false;
    }
    if checks_of(ca) != checks_of(cb) {
        // The side with more checks panics (exit code 101) where the other one goes on, and its output stops where the panic
        // happened: the checks did what they are for. The message cannot be relied on (a program may silence the panic hook).
        let more = |x: (bool, bool), y: (bool, bool)| x.0 >= y.0 && x.1 >= y.1 && x != y;
        let cut_short = |strict: &Outcome, lax: &Outcome| match (strict, lax) {
            (
                Outcome::Ran {
                    code: Some(101),
                    stdout: s1,
                    ..
                },
                Outcome::Ran {
                    code: Some(0),
                    stdout: s2,
                    ..
                },
            ) => s2.starts_with(s1.as_str()),
            _ => false,
        };
        if (more(checks_of(ca), checks_of(cb)) && cut_short(a, b))
            || (more(checks_of(cb), checks_of(ca)) && cut_short(b, a))
        {
            return false;
        }
        let says = |o: &Outcome| matches!(o, Outcome::Ran { stderr, .. } if stderr.contains("with overflow") || stderr.contains("unsafe precondition") || stderr.contains("debug_assert") || stderr.contains("assertion failed: self.is_char_boundary"));
        if says(a) || says(b) {
            return false;
        }
    }
    true
}

/// `-0.0` -> `0.0`, `-0` -> `0` in program output (numbers only, whole tokens).
fn without_negative_zero(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        let boundary =
            i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '.' || c[i - 1] == '_');
        if c[i] == '-' && boundary && c.get(i + 1) == Some(&'0') {
            // the number after the minus: only zeros, an optional fraction of zeros, an optional exponent
            let mut j = i + 1;
            while c.get(j) == Some(&'0') {
                j += 1;
            }
            if c.get(j) == Some(&'.') {
                j += 1;
                while c.get(j) == Some(&'0') {
                    j += 1;
                }
            }
            if matches!(c.get(j), Some('e') | Some('E'))
                && c.get(j + 1)
                    .map(|x| x.is_ascii_digit() || *x == '-' || *x == '+')
                    .unwrap_or(false)
            {
                j += 2;
                while c.get(j).map(|x| x.is_ascii_digit()).unwrap_or(false) {
                    j += 1;
                }
            }
            let ends = c
                .get(j)
                .map(|x| !(x.is_alphanumeric() || *x == '.' || *x == '_'))
                .unwrap_or(true);
            if ends {
                out.extend(&c[i + 1..j]);
                i = j;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// `f32::min` / `f64::max` / `clamp` return either zero when they compare `0.0` and `-0.0` (the documentation says so): a program that
/// uses them and differs only in the sign of a zero shows no compiler bug.
fn uses_float_min_max(src: &str) -> bool {
    (src.contains("f64") || src.contains("f32") || src.contains("T::"))
        && [".min(", ".max(", "::min", "::max", ".clamp("]
            .iter()
            .any(|p| src.contains(p))
}

/// Miri exposes no CPU features, so a program that asks for them runs differently there; a generated case that printed nothing under
/// Miri (feature-gated code) leaves the hash of its output at the starting value.
fn miri_cannot_tell(src: &str, a: &Outcome, b: &Outcome) -> bool {
    const UNTOUCHED_HASH: &str = "cbf29ce484222325";
    let detects_features = ["is_x86_feature_detected", "is_aarch64_feature_detected", "std_detect"]
        .iter()
        .any(|p| src.contains(p));
    let untouched = |o: &Outcome| matches!(o, Outcome::Ran { stdout, .. } if stdout.contains(UNTOUCHED_HASH));
    detects_features || untouched(a) || untouched(b)
}

/// [`differs`] for a known program text.
pub fn differs_src(a: &Outcome, b: &Outcome, ca: &Config, cb: &Config, src: &str) -> bool {
    if !differs(a, b, ca, cb) {
        return false;
    }
    if (ca.miri || cb.miri) && miri_cannot_tell(src, a, b) {
        return false;
    }
    if uses_float_min_max(src)
        && let (
            Outcome::Ran {
                code: c1,
                signal: s1,
                stdout: o1,
                ..
            },
            Outcome::Ran {
                code: c2,
                signal: s2,
                stdout: o2,
                ..
            },
        ) = (a, b)
        && c1 == c2
        && s1 == s2
        && without_negative_zero(o1) == without_negative_zero(o2)
    {
        return false;
    }
    true
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
    let mut a: Vec<String> = vec![
        src.display().to_string(),
        "-o".into(),
        bin.display().to_string(),
    ];
    a.extend(common_flags());
    a.extend(cfg.flags.iter().cloned());
    a
}

pub fn build_and_run(
    rustc: &[String],
    src: &Path,
    bin: &Path,
    cfg: &Config,
    lim: &Limits,
) -> Outcome {
    if cfg.miri {
        return crate::miri::run_program(rustc, src, cfg, lim);
    }
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
    if casts_function_address(&r.stderr) {
        return Outcome::CompileFail(FN_ADDRESS_MESSAGE.into());
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
pub fn check_only(
    rustc: &[String],
    src: &Path,
    out_dir: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let mut c = rustc_cmd(rustc);
    c.args([
        src.display().to_string(),
        "--emit=metadata".into(),
        "--crate-type=bin".into(),
        "--crate-name=fuzz_check".into(),
    ]);
    c.arg("--out-dir").arg(out_dir);
    c.args(COMMON_FLAGS);
    match exec(c, timeout) {
        Ok(r) if r.timed_out => Err("check timed out".into()),
        Ok(r) if r.code == Some(0) && casts_function_address(&r.stderr) => {
            Err(FN_ADDRESS_MESSAGE.into())
        }
        Ok(r) if r.code == Some(0) => Ok(()),
        Ok(r) => Err(r.stderr),
        Err(e) => Err(format!("cannot spawn rustc: {e}")),
    }
}

/// Run `f` over `items` with at most `jobs` threads, results in input order.
pub use rs_minimizer::par_map;

/// Stable fingerprint of a compiler crash, so the same crash found again (other batch, other
/// run) is recognised: the first line naming an LLVM error / ICE / signal, digits normalised.
pub fn crash_signature(o: &Outcome) -> Option<String> {
    if let Outcome::MiriError(e) = o {
        return Some(crate::miri::signature(e));
    }
    let Outcome::CompileFail(e) = o else {
        return None;
    };
    let line = e
        .lines()
        .find(|l| {
            l.contains("LLVM ERROR")
                || l.contains("internal compiler error")
                || l.contains("interrupted by SIG")
                || l.contains("panicked at")
                || l.contains("Assertion")
        })
        .or_else(|| e.lines().find(|l| l.starts_with("error")))?;
    let norm: String = line
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect();
    Some(norm.chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Outcome {
        let dir = std::env::temp_dir().join(format!(
            "rsfuzz-runner-test-{}-{}",
            std::process::id(),
            src.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, src).unwrap();
        let out = build_and_run(
            &["rustc".to_string()],
            &file,
            &dir.join("bin"),
            &Config::new("O0", "-C opt-level=0"),
            &Limits {
                compile: Duration::from_secs(120),
                run: Duration::from_secs(10),
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn a_function_address_turned_into_an_integer_is_not_a_test_case() {
        match run("fn a() {}\nfn main() {\n    print!(\"{}\", a as i8)\n}\n") {
            Outcome::CompileFail(msg) => assert!(msg.contains("function item"), "{msg}"),
            other => panic!("the program must be refused, got {}", other.describe()),
        }
    }

    #[test]
    fn a_plain_program_still_runs() {
        match run("fn a() -> i8 { 3 }\nfn main() {\n    print!(\"{}\", a() as i32)\n}\n") {
            Outcome::Ran { stdout, .. } => assert_eq!(stdout, "3"),
            other => panic!("{}", other.describe()),
        }
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn every_command_names_nightly_and_the_guard_lint_is_not_shown() {
        assert_eq!(
            compiler_for("-C opt-level=3 -Zmir-opt-level=0"),
            "rustc +nightly"
        );
        assert_eq!(compiler_for("-C opt-level=3"), "rustc +nightly");
        let flags: Vec<String> = COMMON_FLAGS.iter().map(|s| s.to_string()).collect();
        let shown = shown_flags(&flags);
        assert!(
            !shown.contains("function_casts_as_integer") && shown.contains("--cap-lints allow"),
            "{shown}"
        );
    }
}

#[cfg(test)]
mod overflow_tests {
    use super::*;

    fn ran(code: i32, stderr: &str) -> Outcome {
        Outcome::Ran {
            code: Some(code),
            signal: None,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn the_sign_of_a_zero_from_min_or_max_is_no_difference() {
        let (off, on) = (Config::new("a", ""), Config::new("b", "-C opt-level=3"));
        let printed = |t: &str| Outcome::Ran {
            code: Some(0),
            signal: None,
            stdout: t.to_string(),
            stderr: String::new(),
        };
        let src = "fn main() { let v = [-0f64, 0f64]; println!(\"{:?}\", v.iter().copied().fold(f64::INFINITY, f64::min)) }";
        assert!(!differs_src(
            &printed("0.0\n"),
            &printed("-0.0\n"),
            &off,
            &on,
            src
        ));
        assert!(
            differs_src(
                &printed("0.0\n"),
                &printed("-0.0\n"),
                &off,
                &on,
                "fn main() { println!(\"{:?}\", (-0f64).abs()) }"
            ),
            "no min/max in the program: still a difference"
        );
        assert!(
            differs_src(&printed("1.0\n"), &printed("-1.0\n"), &off, &on, src),
            "other values still count"
        );
        assert!(differs_src(
            &printed("-0.5\n"),
            &printed("0.5\n"),
            &off,
            &on,
            src
        ));
    }

    #[test]
    fn an_overflow_panic_with_checks_on_is_not_a_difference() {
        let off = Config::new("ref", "");
        let on = Config::new("checks", "-C overflow-checks=on");
        let panic = ran(101, "thread 'main' panicked: attempt to add with overflow");
        assert!(!differs(&ran(0, ""), &panic, &off, &on));
        // the same outputs with the same checks are a real difference
        // a program that silences the panic hook leaves no message: the exit code and the cut-short output decide
        assert!(!differs(&ran(0, ""), &ran(101, ""), &off, &on));
        let other = Config::new("O3", "-C opt-level=3");
        assert!(differs(&ran(0, ""), &panic, &off, &other));
        // output that is not a prefix of the other one is a real difference even with a panic
        let printed = |code: i32, out: &str| Outcome::Ran {
            code: Some(code),
            signal: None,
            stdout: out.to_string(),
            stderr: String::new(),
        };
        assert!(differs(
            &printed(0, "A\nB\n"),
            &printed(101, "X\n"),
            &off,
            &on
        ));
        assert!(!differs(
            &printed(0, "A\nB\n"),
            &printed(101, "A\n"),
            &off,
            &on
        ));
    }

    #[test]
    fn miri_cannot_tell_for_cpu_features_and_an_untouched_hash() {
        let ran = |out: &str| Outcome::Ran { code: Some(0), signal: None, stdout: out.into(), stderr: String::new() };
        let (native, miri) = (Config::new("native", ""), Config::new("miri-mismatch", ""));
        assert!(differs_src(&ran("1 aa\n"), &ran("1 bb\n"), &native, &miri, "fn main() {}"));
        assert!(!differs_src(&ran("1 aa\n"), &ran("1 bb\n"), &native, &miri, "if is_x86_feature_detected!(\"avx2\") {}"));
        assert!(!differs_src(&ran("1 aa\n"), &ran("1 cbf29ce484222325\n"), &native, &miri, "fn main() {}"));
        let (o3, o0) = (Config::new("O3", "-C opt-level=3"), Config::new("O0", "-C opt-level=0"));
        assert!(differs_src(&ran("1 aa\n"), &ran("1 cbf29ce484222325\n"), &o3, &o0, "fn main() {}"), "only Miri comparisons have the exception");
    }

}
