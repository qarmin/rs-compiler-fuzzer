//! Triage of a stored finding: is it ready to be reported upstream?
//!
//! For one finding directory (`case.rs` + `repro.cfg`) this
//!  1. re-runs it on every installed channel (stable/beta/nightly),
//!  2. reduces it (line/let/call-chain passes + `treereduce-rust`) -> `reduced.rs`,
//!  3. checks the reduced program with Miri (undefined behaviour / does Miri agree with one side?),
//!  4. searches GitHub for already reported issues (needs an authenticated `gh`),
//!  5. writes `status.cfg`, `STATUS.md`, `ISSUE.md` and a "Status" block at the top of `report.md`,
//!     ending in an explicit verdict with the reasons.

use log::info;
use crate::reduce::{reduce_all, Interest, ReduceLog};
use crate::steps;
use crate::runner::{build_and_run, crash_signature, exec, Config, Limits, Outcome};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Clone)]
pub struct TriageOpts {
    pub budget: Duration,
    pub search: bool,
    pub work: PathBuf,
    pub no_reduce: bool,
    /// time for the regression-range search (0 = off)
    pub bisect: Duration,
    /// keep the best reduced program on disk while reducing (and use it when interrupted): for local, interactive runs
    pub checkpoint: bool,
    /// reduce again even when an earlier run converged (a stronger machine / longer time may get further)
    pub force_reduce: bool,
}

impl TriageOpts {
    pub fn clone_opts(&self) -> TriageOpts {
        self.clone()
    }
}

/// Version of the triage / minimisation logic. Bump it by hand whenever the results of `triage` get
/// better (smarter reduction, more exact regression range, ...): every stored finding with a smaller
/// `fuzzer_id` is then triaged again by the `retriage` job and replaced in the package.
pub const FUZZER_ID: u32 = 4;

pub type Cfg = BTreeMap<String, String>;

pub fn read_cfg(path: &Path) -> Cfg {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

pub fn write_cfg(path: &Path, cfg: &Cfg) {
    let s: String = cfg.iter().map(|(k, v)| format!("{k}={}\n", v.replace('\n', " "))).collect();
    std::fs::write(path, s).unwrap();
}

fn today() -> String {
    Command::new("date").args(["-u", "+%F"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// Installed channels as (name, "rustc 1.99.1 (...)").
pub fn channels() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for ch in ["stable", "beta", "nightly"] {
        let mut c = Command::new("rustc");
        c.arg(format!("+{ch}")).arg("--version");
        if let Ok(r) = exec(c, Duration::from_secs(30)) {
            if r.code == Some(0) {
                out.push((ch.to_string(), String::from_utf8_lossy(&r.stdout).trim().to_string()));
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Repro {
    Yes,
    No,
    NotApplicable,
}

impl Repro {
    fn word(self) -> &'static str {
        match self {
            Repro::Yes => "yes",
            Repro::No => "no",
            Repro::NotApplicable => "n/a",
        }
    }
}

pub fn reproduces(rustc: &[String], src: &Path, base: &Config, bad: &Config, sig: &Option<String>, lim: &Limits, work: &Path) -> (Repro, Outcome, Outcome) {
    let id = std::process::id();
    let a = build_and_run(rustc, src, &work.join(format!("tri_a_{id}")), base, lim);
    let b = build_and_run(rustc, src, &work.join(format!("tri_b_{id}")), bad, lim);
    // options that only exist on nightly: the configuration cannot be applied on this channel
    let unsupported = |o: &Outcome| matches!(o, Outcome::CompileFail(e) if e.contains("only accepted on the nightly") || e.contains("unstable option") || e.contains("unknown unstable option"));
    if unsupported(&a) || unsupported(&b) {
        return (Repro::NotApplicable, a, b);
    }
    let yes = match sig {
        Some(s) => matches!(a, Outcome::Ran { .. }) && crash_signature(&b).as_deref() == Some(s.as_str()),
        None => matches!((&a, &b), (Outcome::Ran { .. }, Outcome::Ran { .. })) && a.key() != b.key(),
    };
    (if yes { Repro::Yes } else { Repro::No }, a, b)
}

struct MiriResult {
    verdict: String, // "ok", "ub", "panic", "skipped", "error"
    detail: String,
    stdout: String,
    /// what Miri printed when it did not simply run the program (for the issue, if the program has no `unsafe`)
    report: String,
}

fn miri(src: &str, work: &Path) -> MiriResult {
    let have = {
        let mut c = Command::new("cargo");
        c.args(["+nightly", "miri", "--version"]);
        exec(c, Duration::from_secs(60)).map(|r| r.code == Some(0)).unwrap_or(false)
    };
    if !have {
        return MiriResult { verdict: "skipped".into(), detail: "Miri is not installed (rustup component add miri rust-src --toolchain nightly)".into(), stdout: String::new(), report: String::new() };
    }
    let dir = work.join(format!("miri_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"miri_check\"\nversion = \"0.0.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), format!("#![allow(warnings)]\n{src}")).unwrap();
    let mut c = Command::new("cargo");
    c.args(["+nightly", "miri", "run", "-q"]).current_dir(&dir).env("MIRIFLAGS", "-Zmiri-disable-isolation -Zmiri-ignore-leaks").env("CARGO_PROFILE_DEV_OVERFLOW_CHECKS", "false").env("CARGO_PROFILE_DEV_DEBUG_ASSERTIONS", "false");
    let r = exec(c, Duration::from_secs(900));
    let _ = std::fs::remove_dir_all(&dir);
    match r {
        Ok(r) if r.timed_out => MiriResult { verdict: "error".into(), detail: "Miri timed out".into(), stdout: String::new(), report: String::new() },
        Ok(r) if r.code == Some(0) => MiriResult { verdict: "ok".into(), detail: "Miri ran the program to completion".into(), stdout: String::from_utf8_lossy(&r.stdout).into_owned(), report: String::new() },
        Ok(r) => {
            let e = r.stderr;
            let first = e.lines().find(|l| l.contains("Undefined Behavior") || l.contains("panicked") || l.starts_with("error")).unwrap_or("").trim().to_string();
            let v = if e.contains("Undefined Behavior") { "ub" } else if e.contains("panicked") || e.contains("overflow") { "panic" } else { "error" };
            MiriResult { verdict: v.into(), detail: first, stdout: String::from_utf8_lossy(&r.stdout).into_owned(), report: e.lines().take(40).collect::<Vec<_>>().join("\n") }
        }
        Err(e) => MiriResult { verdict: "error".into(), detail: e.to_string(), stdout: String::new(), report: String::new() },
    }
}

#[derive(Default)]
struct Search {
    performed: bool,
    lines: Vec<String>,   // human readable, one per query
    possible: Vec<String>, // "#N title (state) url"
    note: String,
}

fn gh_issues(args: &[&str]) -> Result<Vec<(String, String, String, String)>, String> {
    let mut c = Command::new("gh");
    c.args(["search", "issues"]).args(args).args(["--json", "number,title,state,url", "--jq", r#".[] | "\(.number)\t\(.state)\t\(.title)\t\(.url)""#]);
    let r = exec(c, Duration::from_secs(60)).map_err(|e| e.to_string())?;
    if r.code != Some(0) {
        return Err(r.stderr.lines().next().unwrap_or("gh failed").to_string());
    }
    Ok(String::from_utf8_lossy(&r.stdout)
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '\t');
            Some((p.next()?.to_string(), p.next()?.to_string(), p.next()?.to_string(), p.next()?.to_string()))
        })
        .collect())
}

/// Method names (`.foo(`) in the program, rarest-looking first.
fn distinctive_methods(src: &str) -> Vec<String> {
    const INTERESTING: &[&str] = &[
        "abs_diff", "isqrt", "midpoint", "ilog2", "ilog10", "rem_euclid", "div_euclid", "wrapping_rem_euclid", "wrapping_div_euclid",
        "is_power_of_two", "leading_ones", "trailing_ones", "count_zeros", "saturating_pow", "checked_pow", "overflowing_pow",
        "swap_bytes", "reverse_bits", "copysign", "mul_add", "sqrt", "round_ties_even", "signum", "recip", "fract", "select_nth_unstable",
        "binary_search", "partition_point", "sort_unstable_by_key", "rotate_left", "rotate_right", "checked_next_power_of_two", "unsigned_abs",
        "wrapping_abs", "saturating_mul", "checked_shl", "overflowing_shl", "catch_unwind", "read_unaligned", "write_unaligned", "expose_provenance",
        "wrapping_pow", "wrapping_neg", "checked_neg", "saturating_sub", "saturating_add", "wrapping_shl", "wrapping_shr", "wrapping_mul", "wrapping_add",
    ];
    let mut found = Vec::new();
    for m in INTERESTING {
        if src.contains(&format!(".{m}(")) {
            found.push(m.to_string());
        }
    }
    found
}

fn upstream_search(reduced: &str, bad: &Config, sig: &Option<String>) -> Search {
    let mut s = Search::default();
    // is gh usable at all?
    if let Err(e) = gh_issues(&["--repo", "rust-lang/rust", "--limit", "1", "miscompile"]) {
        s.note = format!("NOT PERFORMED: `gh search` is unavailable ({e}). Search rust-lang/rust and llvm/llvm-project by hand.");
        return s;
    }
    s.performed = true;
    let run = |s: &mut Search, repo: &str, label: &str, extra: &[&str], q: &str, state_filter: bool| {
        let mut args: Vec<&str> = vec!["--repo", repo, "--limit", "10"];
        args.extend_from_slice(extra);
        args.push(q);
        match gh_issues(&args) {
            Err(e) => s.lines.push(format!("- `{repo}` {label} `{q}`: search failed ({e})")),
            Ok(hits) if hits.is_empty() => s.lines.push(format!("- `{repo}` {label} `{q}`: **no issues found**")),
            Ok(hits) => {
                s.lines.push(format!("- `{repo}` {label} `{q}`: {} result(s)", hits.len()));
                for (n, st, title, url) in hits {
                    s.lines.push(format!("    - #{n} [{st}] {title} - {url}"));
                    if state_filter {
                        s.possible.push(format!("{repo}#{n}"));
                    }
                }
            }
        }
    };
    if let Some(sig) = sig {
        let text: String = sig.replace("rustc-LLVM ERROR: ", "").replace("error: ", "").replace('#', "");
        let text = text.trim().chars().take(70).collect::<String>();
        let text = text.trim_end_matches(|c: char| !c.is_alphanumeric()).to_string();
        let q = format!("\"{text}\"");
        run(&mut s, "rust-lang/rust", "crash text", &[], &q, true);
        run(&mut s, "llvm/llvm-project", "crash text", &[], &q, true);
    } else {
        let flag = bad.flags.iter().find(|f| f.contains("opt-level") || f.contains("lto") || f.contains("target-cpu") || f.contains("mir")).cloned().unwrap_or_default();
        let methods = distinctive_methods(reduced);
        if !flag.is_empty() {
            run(&mut s, "rust-lang/rust", "miscompile + flag", &["--label", "I-miscompile"], &format!("\"{flag}\""), false);
        }
        if methods.len() >= 2 {
            let q = format!("{} {}", methods[0], methods[1]);
            run(&mut s, "rust-lang/rust", "methods", &[], &q, true);
        }
        if let Some(m) = methods.first() {
            run(&mut s, "rust-lang/rust", "method + miscompile", &["--label", "I-miscompile"], m, true);
        }
        // title scan of every I-miscompile issue for words that appear in the reduced program
        match gh_issues(&["--repo", "rust-lang/rust", "--label", "I-miscompile", "--limit", "100", ""]) {
            Ok(all) => {
                let mut words: Vec<String> = ["i128", "u128", "i64", "u64", "i32", "u32", "i16", "u16", "i8", "u8", "f64", "f32", "union", "packed", "MaybeUninit", "NonZero", "async", "closure", "dyn", "drop", "loop", "match", "enum", "slice", "LTO", "vector", "select", "unwind", "panic", "simd", "avx"]
                    .iter()
                    .filter(|w| reduced.contains(&w.to_lowercase()) || reduced.contains(**w))
                    .map(|w| w.to_string())
                    .collect();
                if !flag.is_empty() {
                    words.push(flag.rsplit('=').next().unwrap_or("").to_string());
                }
                words.retain(|w| !w.is_empty());
                let related: Vec<_> = all.iter().filter(|(_, _, t, _)| words.iter().any(|w| t.to_lowercase().contains(&w.to_lowercase()))).take(12).collect();
                s.lines.push(format!("- scanned titles of {} `I-miscompile` issues in rust-lang/rust for {:?}: {} possibly related (by keyword only - read them)", all.len(), words, related.len()));
                for (n, st, title, url) in related {
                    s.lines.push(format!("    - #{n} [{st}] {title} - {url}"));
                }
            }
            Err(e) => s.lines.push(format!("- title scan of I-miscompile issues failed ({e})")),
        }
    }
    s
}

fn show_flags(f: &str) -> String {
    if f.trim().is_empty() { "(none)".to_string() } else { format!("`{f}`") }
}

fn lines_of(s: &str) -> usize {
    s.lines().filter(|l| !l.trim().is_empty()).count()
}

/// `["-C", "opt-level=3", "--edition", "2021", "-Zfoo"]` -> `[["-C","opt-level=3"], ["--edition","2021"], ["-Zfoo"]]`
fn flag_groups(flags: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let takes_value = (flags[i] == "-C" || flags[i] == "-Z" || (flags[i].starts_with("--") && !flags[i].contains('='))) && i + 1 < flags.len() && !flags[i + 1].starts_with('-');
        if takes_value {
            out.push(vec![flags[i].clone(), flags[i + 1].clone()]);
            i += 2;
        } else {
            out.push(vec![flags[i].clone()]);
            i += 1;
        }
    }
    out
}

/// The same program built with other optimisation levels: which ones show the bug.
#[derive(Clone, Debug)]
struct Variant {
    flags: String,
    bug: bool,
    out: String,
}

fn variants_to_text(v: &[Variant]) -> String {
    v.iter().map(|x| format!("{}\t{}\t{}\n", x.flags, if x.bug { "bug" } else { "ok" }, x.out.replace('\n', "\\n"))).collect()
}

fn variants_from_text(t: &str) -> Vec<Variant> {
    t.lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some(Variant { flags: p.next()?.to_string(), bug: p.next()? == "bug", out: p.next().unwrap_or("").replace("\\n", "\n") })
        })
        .collect()
}

/// What a run printed, as shown in the issue.
fn show_out(o: &Outcome) -> String {
    match o {
        Outcome::Ran { stdout, code, .. } if stdout.trim().is_empty() => {
            if *code == Some(0) { "(no output)".to_string() } else { format!("(no output, exit code {:?})", code) }
        }
        Outcome::Ran { stdout, .. } => stdout.trim().to_string(),
        other => other.describe(),
    }
}

/// Does this flag group set `name` (`opt-level=`, `codegen-units=`, `mir-opt-level=` ...)? `-C opt-level=3`, `-Copt-level=3` and `-Zmir-opt-level=3` all count,
/// `-Zmir-opt-level=3` does not count as `opt-level=`.
fn has_flag(g: &[String], name: &str) -> bool {
    g.iter().any(|t| {
        let n = t.trim_start_matches('-');
        n.starts_with(name) || ((n.starts_with('C') || n.starts_with('Z')) && n[1..].starts_with(name))
    })
}

/// The flag groups with the one that sets `name` replaced by `new` (appended if there is none): a flag is never given twice.
fn with_flag(groups: &[Vec<String>], name: &str, new: Vec<String>) -> Vec<Vec<String>> {
    let mut g2 = groups.to_vec();
    match g2.iter().position(|g| has_flag(g, name)) {
        Some(i) => g2[i] = new,
        None => g2.push(new),
    }
    g2
}

fn variant_of(o: &Outcome, sig: &Option<String>, reference: &Option<Outcome>) -> (bool, String) {
    let bug = match sig {
        Some(s) => crash_signature(o).as_deref() == Some(s.as_str()),
        None => match (o, reference) {
            (Outcome::Ran { .. }, Some(r)) => o.key() != r.key(),
            _ => false,
        },
    };
    let out = match o {
        Outcome::CompileFail(_) if bug => "(compiler crash)".to_string(),
        Outcome::CompileFail(_) => "(does not compile)".to_string(),
        other => show_out(other),
    };
    (bug, out)
}

/// The same program at the other `opt-level`s (on the channel the issue shows) and at the other `-Zmir-opt-level`s (nightly): which of them show the bug.
fn opt_variants(rustc: &[String], src: &Path, base: &Config, bad: &Config, sig: &Option<String>, reference: &Option<Outcome>, lim: &Limits, work: &Path) -> Vec<Variant> {
    let groups = flag_groups(&bad.flags);
    let mut cfgs: Vec<(Vec<String>, Config)> = Vec::new();
    for l in ["0", "1", "2", "3", "s", "z"] {
        let mut c = bad.clone();
        c.name = format!("opt-{l}");
        c.flags = with_flag(&groups, "opt-level=", vec!["-C".to_string(), format!("opt-level={l}")]).into_iter().flatten().collect();
        cfgs.push((rustc.to_vec(), c));
    }
    // MIR optimisation levels need the nightly compiler
    for l in ["0", "1", "2", "3"] {
        let mut c = bad.clone();
        c.name = format!("mir{l}");
        c.flags = with_flag(&groups, "mir-opt-level=", vec![format!("-Zmir-opt-level={l}")]).into_iter().flatten().collect();
        cfgs.push((vec!["rustc".to_string(), "+nightly".to_string()], c));
    }
    let _ = base;
    let id = std::process::id();
    let outs = crate::runner::par_map(&cfgs, 3, |(r, c)| build_and_run(r, src, &work.join(format!("var_{id}_{}", c.name)), c, lim));
    cfgs.iter()
        .zip(outs)
        .filter_map(|((_, c), o)| {
            if c.name.starts_with("mir") && matches!(&o, Outcome::CompileFail(e) if e.contains("only accepted on the nightly") || e.contains("unknown unstable option") || e.contains("unstable option")) {
                return None; // no nightly installed
            }
            let (bug, out) = variant_of(&o, sig, reference);
            Some(Variant { flags: c.flags.join(" "), bug, out })
        })
        .collect()
}

/// The culprit found by bisecting the optimisation passes.
#[derive(Clone, Debug, Default)]
struct Culprit {
    kind: String, // "llvm" | "mir"
    n: usize,
    total: usize,
    name: String,
    target: String,
    good: Variant,
    bad: Variant,
    note: String,
}

impl Default for Variant {
    fn default() -> Self {
        Variant { flags: String::new(), bug: false, out: String::new() }
    }
}

fn culprit_to_text(c: &Culprit) -> String {
    format!(
        "kind={}\nn={}\ntotal={}\nname={}\ntarget={}\nnote={}\n{}{}",
        c.kind,
        c.n,
        c.total,
        c.name,
        c.target,
        c.note,
        variants_to_text(&[c.good.clone()]).replace('\t', "\u{1}").lines().map(|l| format!("good={l}\n")).collect::<String>(),
        variants_to_text(&[c.bad.clone()]).replace('\t', "\u{1}").lines().map(|l| format!("bad={l}\n")).collect::<String>()
    )
}

fn culprit_from_text(t: &str) -> Culprit {
    let mut c = Culprit::default();
    for l in t.lines() {
        if let Some(x) = l.strip_prefix("kind=") {
            c.kind = x.into();
        } else if let Some(x) = l.strip_prefix("n=") {
            c.n = x.parse().unwrap_or(0);
        } else if let Some(x) = l.strip_prefix("total=") {
            c.total = x.parse().unwrap_or(0);
        } else if let Some(x) = l.strip_prefix("name=") {
            c.name = x.into();
        } else if let Some(x) = l.strip_prefix("target=") {
            c.target = x.into();
        } else if let Some(x) = l.strip_prefix("note=") {
            c.note = x.into();
        } else if let Some(x) = l.strip_prefix("good=") {
            c.good = variants_from_text(&x.replace('\u{1}', "\t")).pop().unwrap_or_default();
        } else if let Some(x) = l.strip_prefix("bad=") {
            c.bad = variants_from_text(&x.replace('\u{1}', "\t")).pop().unwrap_or_default();
        }
    }
    c
}

/// Binary search over the passes of LLVM (`-C llvm-args=-opt-bisect-limit=N`, one codegen unit) or of MIR (`-Zmir-opt-bisect-limit=N`)
/// for the first pass whose execution makes the difference. Nightly only.
fn pass_bisect(kind: &str, src: &Path, base: &Config, bad: &Config, sig: &Option<String>, lim: &Limits, work: &Path) -> Culprit {
    let rustc = vec!["rustc".to_string(), "+nightly".to_string()];
    let groups = flag_groups(&bad.flags);
    let mk = |n: i64| -> Config {
        let g2 = if kind == "llvm" {
            let g = with_flag(&groups, "codegen-units=", vec!["-C".into(), "codegen-units=1".into()]);
            with_flag(&g, "llvm-args=-opt-bisect-limit=", vec!["-C".into(), format!("llvm-args=-opt-bisect-limit={n}")])
        } else {
            with_flag(&groups, "mir-opt-bisect-limit=", vec![format!("-Zmir-opt-bisect-limit={n}")])
        };
        let mut c = bad.clone();
        c.name = format!("bis{}", n.max(0));
        c.flags = g2.into_iter().flatten().collect();
        c
    };
    let mut res = Culprit { kind: kind.to_string(), ..Default::default() };
    let id = std::process::id();
    // the crate is named after the file: keep the temporary name out of the report
    let sdir = work.join(format!("pbsrc_{id}"));
    let _ = std::fs::create_dir_all(&sdir);
    let _ = std::fs::copy(src, sdir.join("reduced.rs"));
    let src_owned = sdir.join("reduced.rs");
    let src = src_owned.as_path();
    let reference = Some(build_and_run(&rustc, src, &work.join(format!("pb_{id}_ref")), base, lim));
    let run = |n: i64| -> (Variant, bool) {
        let c = mk(n);
        let o = build_and_run(&rustc, src, &work.join(format!("pb_{id}_{}", c.name)), &c, lim);
        let (bug, out) = variant_of(&o, sig, &reference);
        (Variant { flags: c.flags.join(" "), bug, out }, bug)
    };
    // all passes, listed
    let c_all = mk(1_000_000);
    let mut cmd = crate::runner::rustc_cmd(&rustc);
    cmd.args(crate::runner::compile_args(src, &work.join(format!("pb_{id}_list")), &c_all));
    let listing = exec(cmd, lim.compile).map(|r| r.stderr).unwrap_or_default();
    let passes: Vec<&str> = listing.lines().filter(|l| l.starts_with("BISECT: running pass (")).collect();
    res.total = passes.len();
    if passes.is_empty() {
        res.note = "the compiler did not list any pass".into();
        return res;
    }
    let (v_all, bug_all) = run(res.total as i64);
    if !bug_all {
        res.note = format!("does not reproduce with {} (so the difference depends on the pass pipeline as a whole)", v_all.flags);
        return res;
    }
    let (v0, bug0) = run(0);
    if bug0 {
        res.note = "still present with no optimisation pass at all".into();
        return res;
    }
    let (mut lo, mut hi, mut v_hi, mut v_lo) = (0usize, res.total, v_all, v0);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        let (v, bug) = run(mid as i64);
        if bug {
            hi = mid;
            v_hi = v;
        } else {
            lo = mid;
            v_lo = v;
        }
    }
    // "BISECT: running pass (N) name on target"
    let line = passes[hi - 1];
    let rest = line.splitn(2, ") ").nth(1).unwrap_or("");
    let (name, target) = rest.split_once(" on ").unwrap_or((rest, ""));
    res.n = hi;
    res.name = name.to_string();
    let t = crate::demangle::demangle_text(target.trim());
    res.target = t.strip_prefix("function (").and_then(|x| x.strip_suffix(')')).unwrap_or(&t).to_string();
    let _ = std::fs::remove_dir_all(&sdir);
    res.good = v_lo;
    res.bad = v_hi;
    res
}

fn search_to_text(s: &Search) -> String {
    let mut t = format!("performed={}\nnote={}\npossible={}\n", s.performed, s.note, s.possible.join(" "));
    for l in &s.lines {
        t += &format!("line={l}\n");
    }
    t
}

fn search_from_text(t: &str) -> Search {
    let mut s = Search::default();
    for l in t.lines() {
        if let Some(x) = l.strip_prefix("performed=") {
            s.performed = x == "true";
        } else if let Some(x) = l.strip_prefix("note=") {
            s.note = x.to_string();
        } else if let Some(x) = l.strip_prefix("possible=") {
            s.possible = x.split_whitespace().map(String::from).collect();
        } else if let Some(x) = l.strip_prefix("line=") {
            s.lines.push(x.to_string());
        }
    }
    s
}

pub fn triage(o: &TriageOpts, dir: &Path) -> Result<String, String> {
    let meta = read_cfg(&dir.join("repro.cfg"));
    let case = std::fs::read_to_string(dir.join("case.rs")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let get = |k: &str| meta.get(k).cloned().unwrap_or_default();
    crate::runner::set_common_flags(None);
    let mut base = Config::new(&get("base_name"), &get("base_flags"));
    let mut bad = Config::new(&get("bad_name"), &get("bad_flags"));
    let sig = std::fs::read_to_string(dir.join("signature.txt")).ok().filter(|s| !s.trim().is_empty());
    let crash = sig.is_some();
    std::fs::create_dir_all(&o.work).unwrap();
    let lim = Limits { compile: Duration::from_secs(300), run: Duration::from_secs(20) };
    let chans = channels();
    let case_path = o.work.join(format!("tri_case_{}.rs", std::process::id()));
    std::fs::write(&case_path, &case).unwrap();
    let date = today();

    // what is already known about this finding (older findings get their checklist derived from the fields they have)
    let mut old = read_cfg(&dir.join("status.cfg"));
    steps::backfill(&mut old, dir);
    let prev_reduced = std::fs::read_to_string(dir.join("reduced.rs")).ok().filter(|p| !p.trim().is_empty() && p.len() < case.len());

    info!("triage {}: step 1, reproduction matrix on the original program ({} lines)", dir.display(), case.lines().count());
    // 1. reproduction matrix on the original reproducer
    let mut matrix: Vec<(String, String, Repro)> = Vec::new();
    for (ch, ver) in &chans {
        let rustc = vec!["rustc".to_string(), format!("+{ch}")];
        let (r, _, _) = reproduces(&rustc, &case_path, &base, &bad, &sig, &lim, &o.work);
        matrix.push((ch.clone(), ver.clone(), r));
    }
    let primary = matrix.iter().filter(|m| m.2 == Repro::Yes).map(|m| m.0.clone()).max_by_key(|c| (c == "nightly") as u8).or_else(|| chans.iter().find(|c| c.0 == "nightly").map(|c| c.0.clone()));

    // 2. reduction - skipped when an earlier run already converged with this version of the minimiser
    let mut reduce_log = ReduceLog(Vec::new());
    let mut reduced = case.clone();
    let mut did_reduce = false;
    let mut continued_from: Option<String> = None;
    let reduce_started = std::time::Instant::now();
    let min_done = !o.force_reduce && steps::current(&old, "minimized", "", &date).filter(|m| m.status == "done").is_some();
    let min_version_ok = steps::get(&old, "minimized").map(|m| m.version == steps::REDUCE_VERSION).unwrap_or(true);
    let prev_runs: u32 = if min_version_ok { old.get("min_runs").and_then(|v| v.parse().ok()).unwrap_or(0) } else { 0 };
    let mut reduction_kept = false;
    match (&prev_reduced, min_done) {
        (Some(prev), true) => {
            reduced = prev.clone();
            reduction_kept = true;
            reduce_log.push(format!("minimisation converged earlier ({} lines); not repeated", prev.lines().count()));
        }
        _ => {
            if let (Some(pc), false) = (&primary, o.no_reduce) {
                if matrix.iter().any(|m| m.2 == Repro::Yes) {
                    let it = Interest { rustc: vec!["rustc".into(), format!("+{pc}")], a: base.clone(), b: bad.clone(), crash_sig: sig.clone(), limits: Limits { compile: Duration::from_secs(120), run: Duration::from_secs(10) }, work: o.work.clone(), cache: Default::default() };
                    // a finding that is triaged again continues from its earlier (already small) result when that still shows the bug
                    let start_src = match &prev_reduced {
                        Some(prev) if it.check(prev) => {
                            reduce_log.push(format!("continuing from the previously reduced program ({} lines)", prev.lines().count()));
                            continued_from = Some(crate::pack::text_hash(prev));
                            prev.clone()
                        }
                        _ => case.clone(),
                    };
                    if it.check(&start_src) {
                        if o.checkpoint {
                            // the checkpoint overwrites reduced.rs: an earlier result that the reducer could not use is kept aside
                            if let (Some(prev), None) = (&prev_reduced, &continued_from) {
                                let _ = std::fs::write(dir.join("reduced.rs.bak"), prev);
                                reduce_log.push("the earlier reduced.rs was not interesting under the reducer's limits: saved as reduced.rs.bak");
                            }
                            crate::reduce::set_checkpoint(Some(dir.join("reduced.rs")));
                        }
                        info!("step 2: reduction from {} lines, {}s budget{}", start_src.lines().count(), o.budget.as_secs(), if o.checkpoint { ", checkpointed to reduced.rs" } else { "" });
                        // Rounds: a round that changed the program is followed by another one on its result while time is left (the
                        // confirmation run that used to wait for the next day happens here), a round that changed nothing ends it.
                        let t0 = std::time::Instant::now();
                        let mut round_src = start_src.clone();
                        let mut last_unchanged = false;
                        for round in 1..=4 {
                            let out = reduce_all(&it, &round_src, o.budget.saturating_sub(t0.elapsed()), &mut reduce_log);
                            last_unchanged = out == round_src;
                            round_src = out;
                            let more = !last_unchanged && crate::reduce::last_run_complete() && !crate::reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) && t0.elapsed() * 4 < o.budget * 3 && round < 4;
                            if !more {
                                break;
                            }
                            info!("round {round} changed the program to {} lines; another round on it ({}s of {}s used)", round_src.lines().count(), t0.elapsed().as_secs(), o.budget.as_secs());
                        }
                        reduced = round_src;
                        if last_unchanged {
                            continued_from = Some(crate::pack::text_hash(&reduced));
                        }
                        if o.checkpoint {
                            // an interrupted run may hold a smaller program than the pass that was cut short returned
                            if let Some(best) = crate::reduce::best_so_far().filter(|b| b.len() < reduced.len() && it.check(b)) {
                                reduce_log.push(format!("kept the smallest program seen: {} lines", best.lines().count()));
                                reduced = best;
                            }
                            crate::reduce::set_checkpoint(None);
                        }
                        did_reduce = true;
                    } else {
                        reduce_log.push("original no longer interesting under the reducer's stricter limits - not reduced");
                    }
                }
            }
        }
    }
    let reduced_path = o.work.join(format!("tri_reduced_{}.rs", std::process::id()));
    std::fs::write(&reduced_path, &reduced).unwrap();
    std::fs::write(dir.join("reduced.rs"), &reduced).unwrap();
    let rhash = crate::pack::text_hash(&reduced);
    // results of older findings that carry no program hash cannot be trusted once the program changed
    if old.get("reduced_hash").map(|h| *h != rhash).unwrap_or(false) {
        steps::forget_unhashed(&mut old);
    }

    info!("step 3: shortest command line");
    // 3. flags: reuse the stored shortest command line when the program is the same
    let flags_ctx = |old: &Cfg| steps::current(old, "flags", &rhash, &date).is_some() && old.contains_key("min_bad_flags");
    let flags_reused = flags_ctx(&old);
    if flags_reused {
        let g = |k: &str| old.get(k).cloned().unwrap_or_default();
        crate::runner::set_common_flags(Some(g("min_common_flags").split_whitespace().map(String::from).collect()));
        base = Config::new(&base.name, &g("min_base_flags"));
        bad = Config::new(&bad.name, &g("min_bad_flags"));
    }

    // 3a. matrix on the reduced reproducer + the outputs that go into the issue text
    let mut matrix2: Vec<(String, String, Repro)> = Vec::new();
    let mut outputs: Option<(Outcome, Outcome)> = None;
    for (ch, ver) in &chans {
        let rustc = vec!["rustc".to_string(), format!("+{ch}")];
        let (r, a, b) = reproduces(&rustc, &reduced_path, &base, &bad, &sig, &lim, &o.work);
        if r == Repro::Yes && outputs.is_none() {
            outputs = Some((a, b));
        }
        matrix2.push((ch.clone(), ver.clone(), r));
    }
    let mut still = matrix2.iter().any(|m| m.2 == Repro::Yes);

    // 3b. the shortest command line that still shows it (the issue shows exactly these flags)
    if still && !flags_reused && !crate::reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
        let yes_channels: Vec<String> = matrix2.iter().filter(|m| m.2 == Repro::Yes).map(|m| m.0.clone()).collect();
        let ok = |base: &Config, bad: &Config| {
            yes_channels.iter().all(|ch| {
                let rustc = vec!["rustc".to_string(), format!("+{ch}")];
                reproduces(&rustc, &reduced_path, base, bad, &sig, &lim, &o.work).0 == Repro::Yes
            })
        };
        let flat = |g: &[Vec<String>]| g.iter().flatten().cloned().collect::<Vec<String>>();
        let mut common = flag_groups(&crate::runner::common_flags());
        for i in (0..common.len()).rev() {
            let mut cand = common.clone();
            cand.remove(i);
            crate::runner::set_common_flags(Some(flat(&cand)));
            if ok(&base, &bad) {
                common = cand;
            }
        }
        crate::runner::set_common_flags(Some(flat(&common)));
        // the reference configuration: nothing at all if possible
        if !base.flags.is_empty() {
            let mut cand = base.clone();
            cand.flags.clear();
            if ok(&cand, &bad) {
                base = cand;
            } else {
                let mut groups = flag_groups(&base.flags);
                for i in (0..groups.len()).rev() {
                    let mut g2 = groups.clone();
                    g2.remove(i);
                    let mut cand = base.clone();
                    cand.flags = flat(&g2);
                    if ok(&cand, &bad) {
                        groups = g2;
                        base = cand;
                    }
                }
            }
        }
        let mut groups = flag_groups(&bad.flags);
        for i in (0..groups.len()).rev() {
            let mut g2 = groups.clone();
            g2.remove(i);
            let mut cand = bad.clone();
            cand.flags = flat(&g2);
            if ok(&base, &cand) {
                groups = g2;
                bad = cand;
            }
        }
        // matrix and outputs again, with the final flags
        matrix2.clear();
        outputs = None;
        for (ch, ver) in &chans {
            let rustc = vec!["rustc".to_string(), format!("+{ch}")];
            let (r, a, b) = reproduces(&rustc, &reduced_path, &base, &bad, &sig, &lim, &o.work);
            if r == Repro::Yes && outputs.is_none() {
                outputs = Some((a, b));
            }
            matrix2.push((ch.clone(), ver.clone(), r));
        }
        still = matrix2.iter().any(|m| m.2 == Repro::Yes);
    }
    // everything below depends on the program *and* the command line
    let common_txt = crate::runner::common_flags().join(" ");
    let ctx = crate::pack::text_hash(&format!("{reduced}\n{common_txt}|{}|{}", base.flags.join(" "), bad.flags.join(" ")));
    let cur = |id: &str| steps::current(&old, id, &ctx, &date).is_some();

    info!("step 4: MIR vs LLVM, optimisation levels, pass bisection");
    // 4. MIR optimisations or LLVM? (-Zmir-opt-level=0 on nightly: if the difference stays, MIR opts are not the cause)
    let mir_reused = cur("mir_llvm") && old.contains_key("mir_opt");
    let mir_opt = if mir_reused {
        old.get("mir_opt").cloned().unwrap_or_default()
    } else if still && matrix2.iter().any(|m| m.0 == "nightly" && m.2 == Repro::Yes) {
        let with0 = |c: &Config| {
            let mut c = c.clone();
            c.flags.push("-Zmir-opt-level=0".into());
            c
        };
        let rustc = vec!["rustc".to_string(), "+nightly".to_string()];
        match reproduces(&rustc, &reduced_path, &with0(&base), &with0(&bad), &sig, &lim, &o.work).0 {
            Repro::Yes => "llvm: still reproduces with -Zmir-opt-level=0, so MIR optimisations are not the cause (LLVM or code generation)".to_string(),
            Repro::No => "mir: does not reproduce with -Zmir-opt-level=0, so a MIR optimisation is involved".to_string(),
            Repro::NotApplicable => "unknown: -Zmir-opt-level=0 not applicable".to_string(),
        }
    } else {
        "unknown: not reproduced on nightly".to_string()
    };

    // 4b. the same program at the other optimisation levels (on the channel the issue shows)
    let var_chan = matrix2.iter().find(|m| m.2 == Repro::Yes).map(|m| m.0.clone());
    let var_reused = cur("opt_levels") && dir.join("variants.txt").exists();
    let variants: Vec<Variant> = if var_reused {
        variants_from_text(&std::fs::read_to_string(dir.join("variants.txt")).unwrap_or_default())
    } else if let (true, Some(ch), false) = (still, &var_chan, crate::reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed)) {
        let rustc = vec!["rustc".to_string(), format!("+{ch}")];
        opt_variants(&rustc, &reduced_path, &base, &bad, &sig, &outputs.as_ref().map(|o| o.0.clone()), &lim, &o.work)
    } else {
        Vec::new()
    };
    if !variants.is_empty() {
        let _ = std::fs::write(dir.join("variants.txt"), variants_to_text(&variants));
    }

    // 4c. which optimisation pass? (opt-bisect: LLVM when MIR is cleared, MIR otherwise)
    let culprit_kind = if mir_opt.starts_with("llvm") { Some("llvm") } else if mir_opt.starts_with("mir") { Some("mir") } else { None };
    let cul_reused = cur("pass_bisect") && dir.join("culprit.txt").exists();
    let culprit: Option<Culprit> = if cul_reused {
        Some(culprit_from_text(&std::fs::read_to_string(dir.join("culprit.txt")).unwrap_or_default()))
    } else if let (true, Some(k), false) = (still, culprit_kind, crate::reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed)) {
        Some(pass_bisect(k, &reduced_path, &base, &bad, &sig, &lim, &o.work))
    } else {
        None
    };
    match &culprit {
        Some(c) => {
            let _ = std::fs::write(dir.join("culprit.txt"), culprit_to_text(c));
        }
        None => {
            let _ = std::fs::remove_file(dir.join("culprit.txt"));
        }
    }

    info!("step 5: Miri");
    // 5. Miri (only meaningful for wrong-code findings)
    let ub_reused = !crash && still && cur("ub_check") && old.contains_key("miri");
    let (m, miri_agrees) = if crash || !still {
        (MiriResult { verdict: "skipped".into(), detail: if crash { "not applicable (compiler crash)".into() } else { "does not reproduce".into() }, stdout: String::new(), report: String::new() }, String::new())
    } else if ub_reused {
        let s = old.get("miri").cloned().unwrap_or_default();
        let (v, d) = s.split_once(": ").unwrap_or((s.as_str(), ""));
        (MiriResult { verdict: v.to_string(), detail: d.to_string(), stdout: String::new(), report: std::fs::read_to_string(dir.join("miri.txt")).unwrap_or_default() }, old.get("miri_agrees").cloned().unwrap_or_default())
    } else {
        let m = miri(&reduced, &o.work);
        let agrees = match (&outputs, m.verdict.as_str()) {
            (Some((a, b)), "ok") => {
                let norm = |s: &str| s.trim().to_string();
                let (sa, sb) = (
                    if let Outcome::Ran { stdout, .. } = a { norm(stdout) } else { String::new() },
                    if let Outcome::Ran { stdout, .. } = b { norm(stdout) } else { String::new() },
                );
                let sm = norm(&m.stdout);
                if sm == sa { format!("Miri's output equals `{}` (so `{}` looks wrong)", base.name, bad.name) } else if sm == sb { format!("Miri's output equals `{}` (so `{}` looks wrong)", bad.name, base.name) } else { "Miri's output matches neither side".to_string() }
            }
            _ => String::new(),
        };
        (m, agrees)
    };

    info!("step 6: upstream search");
    // 6. upstream search (repeated after two weeks)
    let search_reused = cur("upstream") && dir.join("search.txt").exists();
    let search = if search_reused {
        search_from_text(&std::fs::read_to_string(dir.join("search.txt")).unwrap_or_default())
    } else if o.search && !crate::reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
        upstream_search(&reduced, &bad, &sig)
    } else {
        Search { note: "NOT PERFORMED (disabled)".into(), ..Default::default() }
    };

    info!("step 7: regression range (bisect budget {}s)", o.bisect.as_secs());
    // 7. regression range: only the parts that are still open
    let prior = std::fs::read_to_string(dir.join("bisect.txt")).ok().map(|t| crate::bisect::Bisect::from_text(&t)).or_else(|| crate::bisect::Bisect::from_legacy(&old));
    let prior_known = prior.is_some();
    let mut bis = prior.unwrap_or_default();
    if !o.bisect.is_zero() && still && m.verdict != "ub" && !crate::reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
        let yes = |ch: &str| matrix2.iter().any(|m| m.0 == ch && m.2 == Repro::Yes);
        let want = crate::bisect::Want { stable: !prior_known || !cur("bisect_stable"), nightly: !prior_known || !cur("bisect_nightly"), pr: !prior_known || !cur("bisect_pr") };
        if want.stable || want.nightly || want.pr {
            bis = crate::bisect::bisect(&reduced_path, &base, &bad, &sig, &o.work, o.bisect, want, yes("stable"), yes("nightly"), &bis);
        }
    }
    let _ = std::fs::write(dir.join("bisect.txt"), bis.to_text());

    info!("step 8: writing the checklist and the issue draft");
    // 8. how settled is the minimisation? (the daily job keeps going until two runs agree)
    let min_runs = if reduction_kept { prev_runs } else if did_reduce { prev_runs + 1 } else { prev_runs };
    let complete = did_reduce && crate::reduce::last_run_complete();
    let unchanged = continued_from.as_deref() == Some(rhash.as_str());
    let rl = lines_of(&reduced);
    let min_status = if reduction_kept {
        old.get("min_status").cloned().unwrap_or_else(|| "converged".into())
    } else if !did_reduce {
        "not reduced".to_string()
    } else if complete && unchanged {
        "converged".to_string()
    } else if min_runs >= 6 {
        "converged (run limit)".to_string()
    } else if complete {
        "complete once - a confirmation run is pending".to_string()
    } else {
        "in progress - a pass ran out of time".to_string()
    };
    let converged = min_status.starts_with("converged");

    // ---- the checklist
    let mut st = old.clone();
    let put = |st: &mut Cfg, id: &str, status: &str, hash: &str, note: String| steps::set(st, id, steps::State::new(status, hash, &date, &note));
    // a step that was not redone keeps its result (and date) but now belongs to this program
    let keep = |st: &mut Cfg, id: &str| {
        if let Some(s) = steps::current(&old, id, &ctx, &date) {
            let mut s = s;
            s.hash = ctx.clone();
            steps::set(st, id, s);
        }
    };
    let yes_list: Vec<String> = matrix2.iter().filter(|m| m.2 == Repro::Yes).map(|m| m.0.clone()).collect();
    if still {
        put(&mut st, "reproduced", "done", &ctx, format!("reproduces on {}", yes_list.join(", ")));
    } else {
        put(&mut st, "reproduced", "failed", &ctx, "does not reproduce on any installed channel any more".into());
    }
    if !converged {
        let note = if !did_reduce { "not reduced (the reducer did not run or the original was not interesting under its limits)".to_string() } else { format!("{min_status}; run {min_runs} of at most 6") };
        put(&mut st, "minimized", if did_reduce { "partial" } else { "todo" }, "", note);
    } else if rl > 300 {
        put(&mut st, "minimized", "partial", "", format!("still {rl} lines and no single case shows it on its own: it depends on the size/shape of the whole program (inlining or similar) - needs a longer reduction or a hand investigation"));
    } else {
        let extra = if rl > 80 { format!(" - but still {rl} lines: reduce further by hand (tools/reduce.py, treereduce-rust)") } else { String::new() };
        put(&mut st, "minimized", "done", "", format!("{min_status}, {rl} lines after {min_runs} run(s){extra}"));
    }
    if still && flags_reused {
        keep(&mut st, "flags");
    } else if still {
        put(&mut st, "flags", "done", &ctx, format!("reference: {}; failing: {}; common: {}", show_flags(&base.flags.join(" ")), show_flags(&bad.flags.join(" ")), show_flags(&common_txt)));
    } else {
        put(&mut st, "flags", "todo", &ctx, "needs a reproducing program first".into());
    }
    if var_reused {
        keep(&mut st, "opt_levels");
    } else if variants.is_empty() {
        put(&mut st, "opt_levels", "todo", &ctx, "needs a reproducing program first".into());
    } else {
        let list = |bug: bool| variants.iter().filter(|v| v.bug == bug).map(|v| v.flags.clone()).collect::<Vec<_>>().join(", ");
        put(&mut st, "opt_levels", "done", &ctx, format!("{} levels tried; bug with: {}; not with: {}", variants.len(), list(true), list(false)));
    }
    if ub_reused {
        keep(&mut st, "ub_check");
    } else {
    match m.verdict.as_str() {
        "ok" => put(&mut st, "ub_check", "done", &ctx, "Miri found no undefined behaviour".into()),
        "ub" => put(&mut st, "ub_check", "failed", &ctx, format!("Miri reports undefined behaviour: {}", m.detail)),
        "panic" => put(&mut st, "ub_check", "partial", &ctx, format!("the program panics under Miri (arithmetic overflow?): {}", m.detail)),
        "skipped" if crash => put(&mut st, "ub_check", "n/a", &ctx, "compiler crash: there is no program output to check".into()),
        "skipped" => put(&mut st, "ub_check", "todo", &ctx, m.detail.clone()),
        _ => put(&mut st, "ub_check", "partial", &ctx, format!("Miri could not decide: {}", m.detail)),
    }
    }
    if cul_reused {
        keep(&mut st, "pass_bisect");
    } else {
        match &culprit {
            Some(c) if c.n > 0 => put(&mut st, "pass_bisect", "done", &ctx, format!("first {} pass that matters: #{} of {}: {} on {}", c.kind.to_uppercase(), c.n, c.total, c.name, c.target)),
            Some(c) => put(&mut st, "pass_bisect", "done", &ctx, format!("tried, {} pass bisection could not isolate a pass: {}", c.kind, c.note)),
            None if still => put(&mut st, "pass_bisect", "n/a", &ctx, "neither MIR nor LLVM could be determined as the cause".into()),
            None => put(&mut st, "pass_bisect", "todo", &ctx, "needs a reproducing program first".into()),
        }
    }
    if mir_reused {
        keep(&mut st, "mir_llvm");
    } else if mir_opt.starts_with("llvm") || mir_opt.starts_with("mir") {
        put(&mut st, "mir_llvm", "done", &ctx, mir_opt.clone());
    } else if mir_opt.starts_with("unknown: not reproduced") {
        put(&mut st, "mir_llvm", "n/a", &ctx, "does not reproduce on nightly".into());
    } else {
        put(&mut st, "mir_llvm", "todo", &ctx, mir_opt.clone());
    }
    // the batch: is a single case enough?
    {
        let n = reduced.matches("mod case_").count();
        if n <= 1 {
            put(&mut st, "minimize_cases", "done", "", if n == 0 { "reduced to a plain program".into() } else { "a single case is enough".into() });
        } else {
            put(&mut st, "minimize_cases", "partial", "", format!("{n} cases of the batch are still needed together (the bug depends on the size/shape of the whole program)"));
        }
    }
    // regression or long-standing? decides whether the bisection applies at all
    {
        let reg = |p: &crate::bisect::Part| p.summary.contains("first bad");
        let old_bug = |p: &crate::bisect::Part| p.summary.contains("since <=");
        if reg(&bis.stable) || reg(&bis.nightly) {
            put(&mut st, "regression", "done", &ctx, format!("a regression ({})", [bis.stable.summary.as_str(), bis.nightly.summary.as_str()].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("; ")));
        } else if old_bug(&bis.stable) || old_bug(&bis.nightly) {
            put(&mut st, "regression", "done", &ctx, "long-standing: present in the oldest release that can compile it - not a regression, so the bisection steps are skipped".into());
        } else if bis.stable.na && bis.nightly.na {
            put(&mut st, "regression", "n/a", &ctx, "does not reproduce on the current stable or nightly".into());
        } else {
            put(&mut st, "regression", "todo", &ctx, "needs the stable/nightly search first".into());
        }
    }
    for (id, part) in [("bisect_stable", &bis.stable), ("bisect_nightly", &bis.nightly), ("bisect_pr", &bis.pr)] {
        let ran_here = !o.bisect.is_zero() && still && m.verdict != "ub";
        if cur(id) && prior_known && !ran_here {
            keep(&mut st, id);
        } else if part.na {
            put(&mut st, id, "n/a", &ctx, part.note.clone());
        } else if part.done {
            put(&mut st, id, "done", &ctx, part.summary.clone());
        } else if cur(id) && prior_known {
            keep(&mut st, id);
        } else if !part.note.is_empty() || !part.lines.is_empty() {
            put(&mut st, id, "partial", &ctx, part.note.clone());
        } else {
            put(&mut st, id, "todo", &ctx, if o.bisect.is_zero() { "regression search disabled".into() } else { "not done yet".into() });
        }
    }
    if search_reused {
        keep(&mut st, "upstream");
    } else if search.performed {
        let note = if search.possible.is_empty() { "no matching issue found".to_string() } else { format!("possible duplicates: {} - read them before reporting", search.possible.join(" ")) };
        put(&mut st, "upstream", "done", &ctx, note);
    } else {
        put(&mut st, "upstream", "todo", &ctx, if search.note.is_empty() { "not performed".into() } else { search.note.clone() });
    }
    put(&mut st, "issue_draft", if still { "done" } else { "todo" }, &ctx, String::new());

    if m.report.is_empty() {
        let _ = std::fs::remove_file(dir.join("miri.txt"));
    } else {
        let _ = std::fs::write(dir.join("miri.txt"), &m.report);
    }
    // ---- write status files
    st.remove("verdict");
    st.remove("blockers");
    st.remove("warnings");
    st.insert("triaged_at".into(), date.clone());
    st.insert("reduced_lines".into(), rl.to_string());
    st.insert("original_lines".into(), lines_of(&case).to_string());
    st.insert("miri".into(), format!("{}: {}", m.verdict, m.detail));
    st.insert("miri_agrees".into(), miri_agrees.clone());
    st.insert("mir_opt".into(), mir_opt.clone());
    st.insert("upstream_search".into(), if search.performed { if search.possible.is_empty() { "no matching issue found".into() } else { format!("possible duplicates: {}", search.possible.join(" ")) } } else { "not performed".into() });
    st.insert("kind".into(), if crash { "compiler-crash".into() } else { "wrong-code".into() });
    st.insert("fuzzer_id".into(), FUZZER_ID.to_string());
    st.insert("min_status".into(), min_status.clone());
    st.insert("min_runs".into(), min_runs.to_string());
    if !reduction_kept {
        st.insert("min_last_run_seconds".into(), reduce_started.elapsed().as_secs().to_string());
    }
    st.insert("min_common_flags".into(), common_txt.clone());
    st.insert("min_base_flags".into(), base.flags.join(" "));
    st.insert("min_bad_flags".into(), bad.flags.join(" "));
    st.insert("reduced_hash".into(), rhash.clone());
    st.insert("context_hash".into(), ctx.clone());
    st.insert("regression_edges".into(), bis.edges().iter().map(|(l, y)| format!("{l} => {}", if *y { "yes" } else { "no" })).collect::<Vec<_>>().join(" ; "));
    st.insert("regression".into(), if bis.summary().is_empty() { "unknown".into() } else { bis.summary() });
    if !bis.pr.lines.is_empty() {
        st.insert("regression_pr".into(), bis.pr.lines.iter().map(|l| l.trim_start_matches("- ").to_string()).collect::<Vec<_>>().join(" | "));
    } else {
        st.remove("regression_pr");
    }
    for (ch, ver, r) in &matrix2 {
        st.insert(format!("repro.{ch}"), format!("{} ({})", r.word(), ver));
    }
    st.insert("last_verified".into(), date.clone());
    st.entry("state".into()).or_insert_with(|| "open".into());
    let (done_n, total_n) = steps::progress(&st, &date);
    st.insert("progress".into(), format!("{done_n}/{total_n}"));
    write_cfg(&dir.join("status.cfg"), &st);
    if search.performed {
        let _ = std::fs::write(dir.join("search.txt"), search_to_text(&search));
    }
    crate::pack::write_finding_json(dir);

    let mut md = String::new();
    let _ = writeln!(md, "## Status ({date})\n");
    md += &steps::checklist_md(&st, &date);
    let open = steps::open_steps(&st, &date);
    if !open.is_empty() {
        let _ = writeln!(md, "\nStill to do:");
        for (title, why) in &open {
            let _ = writeln!(md, "- {title}: {why}");
        }
    }
    let _ = writeln!(md);
    let _ = writeln!(md, "Kind: {}\n", if crash { "compiler crash / compile failure" } else { "wrong code (different output between two configurations)" });
    let _ = writeln!(md, "### Reproduction (reduced program, `reduced.rs`: {rl} lines; original `case.rs`: {} lines)\n", lines_of(&case));
    let _ = writeln!(md, "| channel | original case.rs | reduced.rs |\n|---|---|---|");
    for ((ch, ver, r1), (_, _, r2)) in matrix.iter().zip(matrix2.iter()) {
        let _ = writeln!(md, "| {ch} ({ver}) | {} | {} |", r1.word(), r2.word());
    }
    let _ = writeln!(md);
    if !reduce_log.0.is_empty() {
        let _ = writeln!(md, "### Reduction log\n");
        for l in &reduce_log.0 {
            let _ = writeln!(md, "- {l}");
        }
        let _ = writeln!(md);
    }
    let _ = writeln!(md, "### Regression range\n");
    let bl = bis.lines();
    if bl.is_empty() {
        let _ = writeln!(md, "- not determined");
    }
    for l in &bl {
        let _ = writeln!(md, "{}", if l.starts_with("- ") { l.clone() } else { format!("- {l}") });
    }
    let _ = writeln!(md);
    let _ = writeln!(md, "### MIR optimisations or LLVM?\n\n- {mir_opt}\n");
    let _ = writeln!(md, "### Minimisation\n\n- status: **{min_status}** (run {min_runs}; last run {}s; every pass finished before its deadline: {})\n", reduce_started.elapsed().as_secs(), if complete { "yes" } else if reduction_kept { "kept from an earlier run" } else { "no" });
    let _ = writeln!(md, "### UB check (Miri, nightly)\n\n- result: **{}** - {}", m.verdict, m.detail);
    if !miri_agrees.is_empty() {
        let _ = writeln!(md, "- {miri_agrees}");
    }
    let _ = writeln!(md, "\n### Already reported upstream? (searched {date})\n");
    if search.performed {
        if search.possible.is_empty() {
            let _ = writeln!(md, "**No matching issue found on GitHub** by any of the queries below (this does not prove it was never reported - skim the keyword scan too):\n");
        } else {
            let _ = writeln!(md, "**Possible duplicates found - review them:** {}\n", search.possible.join(", "));
        }
        for l in &search.lines {
            let _ = writeln!(md, "{l}");
        }
    } else {
        let _ = writeln!(md, "{}", search.note);
    }
    std::fs::write(dir.join("STATUS.md"), &md).unwrap();

    // issue draft: only the reduced program, the shortest commands, the output and a version table
    let mut is = String::new();
    let shown_common = crate::runner::shown_flags(&crate::runner::common_flags());
    let vcmd = |flags: &str| {
        let line = format!("{shown_common} {flags}");
        format!("{} reduced.rs {line}", crate::runner::compiler_for(&line)).split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let cmd = |c: &Config| vcmd(&c.flags.join(" "));
    let first_yes = matrix2.iter().find(|m| m.2 == Repro::Yes).map(|m| m.0.clone());
    let vv = first_yes
        .as_ref()
        .and_then(|ch| {
            let mut c = Command::new("rustc");
            c.arg(format!("+{ch}")).arg("-vV");
            exec(c, Duration::from_secs(30)).ok().map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string())
        })
        .unwrap_or_default();
    let title = if crash {
        let s = sig.clone().unwrap_or_default().replace('#', "N");
        if bad.flags.is_empty() { s } else { format!("{s} with `{}`", bad.flags.join(" ")) }
    } else {
        if base.flags.is_empty() {
            format!("Miscompilation with `{}`: different output than the default build", bad.flags.join(" "))
        } else {
            format!("Miscompilation: `{}` and `{}` produce different output", base.flags.join(" "), bad.flags.join(" "))
        }
    };
    let _ = std::fs::write(dir.join("ISSUE_TITLE.txt"), format!("{title}\n"));
    let _ = writeln!(is, "I tried this code:\n\n```rust\n{}\n```\n", reduced.trim_end());
    // every command with what it printed: the reference build, the failing build and the sibling optimisation levels
    struct Entry {
        flags: String,
        text: String,
        fails: bool,
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut crash_err = String::new();
    if let Some((a, b)) = &outputs {
        entries.push(Entry { flags: base.flags.join(" "), text: show_out(a), fails: false });
        crash_err = match b {
            Outcome::CompileFail(e) => crate::demangle::demangle_text(e.trim()).chars().take(20000).collect::<String>(),
            other => other.describe(),
        };
        entries.push(Entry { flags: bad.flags.join(" "), text: if crash { crash_err.clone() } else { show_out(b) }, fails: true });
    }
    // the opt-bisect runs only tell us which pass is to blame (the sentence below); their commands are not part of the issue
    for v in variants.iter() {
        if entries.iter().any(|e| e.flags == v.flags) {
            continue;
        }
        entries.push(Entry { flags: v.flags.clone(), text: if crash && v.bug { crash_err.clone() } else { v.out.clone() }, fails: v.bug });
    }
    if entries.is_empty() {
        let _ = writeln!(is, "Commands:\n```\n{}\n{}\n```\n(did not reproduce when this draft was written)\n", cmd(&base), cmd(&bad));
    } else {
        // group by result; the group of the reference build comes first
        let mut groups: Vec<(String, bool, Vec<String>)> = Vec::new();
        for e in &entries {
            let key = if crash && e.fails { "\u{0}fail".to_string() } else { e.text.clone() };
            let command = if crash && e.fails { vcmd(&e.flags) } else { format!("{}; ./reduced", vcmd(&e.flags)) };
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => g.2.push(command),
                None => groups.push((key, crash && e.fails, vec![command])),
            }
        }
        for (i, (key, is_fail, cmds)) in groups.iter().enumerate() {
            let lead = match i { 0 => "Rust compiled with", 1 => "but compiled with", _ => "and compiled with" };
            let text = if *is_fail { crash_err.as_str() } else { key.as_str() };
            let verb = if *is_fail { "fails with" } else if crash { "compiles and prints" } else { "prints" };
            if crash && !*is_fail && text == "(no output)" {
                let _ = writeln!(is, "{lead}\n```\n{}\n```\nthe program builds and runs without errors.\n", cmds.join("\n"));
            } else {
                let _ = writeln!(is, "{lead}\n```\n{}\n```\n{verb}\n```\n{text}\n```\n", cmds.join("\n"));
            }
        }
    }
    if let Some(c) = culprit.as_ref().filter(|c| c.n > 0) {
        let what = if c.kind == "llvm" { "LLVM pass" } else { "MIR pass" };
        let limit = if c.kind == "llvm" { "-C llvm-args=-opt-bisect-limit" } else { "-Zmir-opt-bisect-limit" };
        let _ = writeln!(is, "Bisecting the optimization passes with `{limit}`: the first {what} that changes the result is pass #{} of {}, `{}` on `{}`.\n", c.n, c.total, c.name, c.target);
    }
    // Miri is a required step of every finding; its output only belongs here when the program has no `unsafe` and Miri still complains
    if !m.report.is_empty() && !reduced.contains("unsafe") {
        let _ = writeln!(is, "Miri reports for this program (it contains no `unsafe`):\n```\n{}\n```\n", m.report.trim());
    }
    let _ = writeln!(is, "### Meta\n\n`rustc --version --verbose`:\n```\n{vv}\n```\n");
    let _ = writeln!(is, "| channel | reproducible |\n|---|---|");
    for (ch, ver, r) in &matrix2 {
        let _ = writeln!(is, "| {ch} ({ver}) | {} |", r.word());
    }
    for (label, yes) in bis.edges() {
        let _ = writeln!(is, "| {label} | {} |", if yes { "yes" } else { "no" });
    }
    if bis.pr.lines.iter().any(|l| l.contains("rust-lang/rust")) {
        let _ = writeln!(is, "\nBisected with `cargo bisect-rustc`:\n");
        for l in &bis.pr.lines {
            let _ = writeln!(is, "{l}");
        }
    }
    let _ = writeln!(is, "\nProblem was found by a rust fuzzer (created by an LLM) - https://github.com/qarmin/rs-compiler-fuzzer");
    std::fs::write(dir.join("ISSUE.md"), is).unwrap();

    // put the status at the top of report.md (replace an older block)
    let rep_path = dir.join("report.md");
    if let Ok(rep) = std::fs::read_to_string(&rep_path) {
        let body = match rep.find("<!-- /status -->\n") {
            Some(p) => rep[p + "<!-- /status -->\n".len()..].to_string(),
            None => rep,
        };
        std::fs::write(&rep_path, format!("{md}<!-- /status -->\n{body}")).unwrap();
    }
    let _ = std::fs::remove_file(&case_path);
    let _ = std::fs::remove_file(&reduced_path);
    let summary = open.iter().map(|(t, _)| *t).take(3).collect::<Vec<_>>().join("; ");
    Ok(format!("{done_n}/{total_n} steps done{}", if open.is_empty() { String::new() } else { format!("; open: {summary}") }))
}

/// The checklist steps of a stored finding that are still open (ids).
pub fn open_ids_of(dir: &Path) -> Vec<&'static str> {
    let mut st = read_cfg(&dir.join("status.cfg"));
    steps::backfill(&mut st, dir);
    steps::open_ids(&st, &today())
}

/// Stored findings (in an unpacked package) with open checklist steps, closest to completion first. `skip` = keys already handled by
/// this job. A finding whose only open step is the (long) minimisation is not started again on the day it was last triaged.
pub fn stale_findings(pkg: &Path, skip: &std::collections::HashSet<String>) -> Vec<PathBuf> {
    let root = pkg.join("findings");
    let today = today();
    let mut v: Vec<(usize, usize, String, PathBuf)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("case.rs").exists() && p.join("repro.cfg").exists())
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            let mut st = read_cfg(&p.join("status.cfg"));
            let meta = read_cfg(&p.join("repro.cfg"));
            steps::backfill(&mut st, &p);
            let done = st.get("state").map(|s| s == "fixed" || s == "not-reproducing").unwrap_or(false) || meta.get("mode").map(|m| m == "miri").unwrap_or(false) || skip.contains(&name);
            if done || steps::has_failure(&st) {
                return None;
            }
            let open = steps::open_steps(&st, &today);
            if open.is_empty() {
                return None;
            }
            let only_min = open.len() == 1 && open[0].0 == steps::step("minimized").title;
            if only_min && st.get("triaged_at").map(|d| *d == today).unwrap_or(false) {
                return None;
            }
            let lines = st.get("reduced_lines").and_then(|v| v.parse::<usize>().ok()).unwrap_or(usize::MAX);
            Some((open.len(), lines, name, p))
        })
        .collect();
    v.sort();
    v.into_iter().map(|x| x.3).collect()
}
