//! Triage of a stored finding: is it ready to be reported upstream?
//!
//! For one finding directory (`case.rs` + `repro.cfg`) this
//!  1. re-runs it on every installed channel (stable/beta/nightly),
//!  2. reduces it (line/let/call-chain passes + `treereduce-rust`) -> `reduced.rs`,
//!  3. checks the reduced program with Miri (undefined behaviour / does Miri agree with one side?),
//!  4. searches GitHub for already reported issues (needs an authenticated `gh`),
//!  5. writes `status.cfg`, `STATUS.md`, `ISSUE.md` and a "Status" block at the top of `report.md`,
//!     ending in an explicit verdict with the reasons.

use crate::reduce::{reduce_all, Interest, ReduceLog};
use crate::steps;
use crate::runner::{build_and_run, crash_signature, exec, Config, Limits, Outcome};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub struct TriageOpts {
    pub budget: Duration,
    pub search: bool,
    pub work: PathBuf,
    pub no_reduce: bool,
    /// time for the regression-range search (0 = off)
    pub bisect: Duration,
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
}

fn miri(src: &str, work: &Path) -> MiriResult {
    let have = {
        let mut c = Command::new("cargo");
        c.args(["+nightly", "miri", "--version"]);
        exec(c, Duration::from_secs(60)).map(|r| r.code == Some(0)).unwrap_or(false)
    };
    if !have {
        return MiriResult { verdict: "skipped".into(), detail: "Miri is not installed (rustup component add miri rust-src --toolchain nightly)".into(), stdout: String::new() };
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
        Ok(r) if r.timed_out => MiriResult { verdict: "error".into(), detail: "Miri timed out".into(), stdout: String::new() },
        Ok(r) if r.code == Some(0) => MiriResult { verdict: "ok".into(), detail: "Miri ran the program to completion".into(), stdout: String::from_utf8_lossy(&r.stdout).into_owned() },
        Ok(r) => {
            let e = r.stderr;
            let first = e.lines().find(|l| l.contains("Undefined Behavior") || l.contains("panicked") || l.starts_with("error")).unwrap_or("").trim().to_string();
            let v = if e.contains("Undefined Behavior") { "ub" } else if e.contains("panicked") || e.contains("overflow") { "panic" } else { "error" };
            MiriResult { verdict: v.into(), detail: first, stdout: String::from_utf8_lossy(&r.stdout).into_owned() }
        }
        Err(e) => MiriResult { verdict: "error".into(), detail: e.to_string(), stdout: String::new() },
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
    let min_done = steps::current(&old, "minimized", "", &date).filter(|m| m.status == "done").is_some();
    let min_version_ok = steps::get(&old, "minimized").map(|m| m.version == steps::REDUCE_VERSION).unwrap_or(true);
    let prev_runs: u32 = if min_version_ok { old.get("min_runs").and_then(|v| v.parse().ok()).unwrap_or(0) } else { 0 };
    let mut reduction_kept = false;
    match (&prev_reduced, min_done) {
        (Some(prev), true) => {
            reduced = prev.clone();
            reduction_kept = true;
            reduce_log.0.push(format!("minimisation converged earlier ({} lines); not repeated", prev.lines().count()));
        }
        _ => {
            if let (Some(pc), false) = (&primary, o.no_reduce) {
                if matrix.iter().any(|m| m.2 == Repro::Yes) {
                    let it = Interest { rustc: vec!["rustc".into(), format!("+{pc}")], a: base.clone(), b: bad.clone(), crash_sig: sig.clone(), limits: Limits { compile: Duration::from_secs(120), run: Duration::from_secs(10) }, work: o.work.clone(), cache: Default::default() };
                    // a finding that is triaged again continues from its earlier (already small) result when that still shows the bug
                    let start_src = match &prev_reduced {
                        Some(prev) if it.check(prev) => {
                            reduce_log.0.push(format!("continuing from the previously reduced program ({} lines)", prev.lines().count()));
                            continued_from = Some(crate::pack::text_hash(prev));
                            prev.clone()
                        }
                        _ => case.clone(),
                    };
                    if it.check(&start_src) {
                        reduced = reduce_all(&it, &start_src, o.budget, &mut reduce_log);
                        did_reduce = true;
                    } else {
                        reduce_log.0.push("original no longer interesting under the reducer's stricter limits - not reduced".into());
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
    if still && !flags_reused {
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

    // 5. Miri (only meaningful for wrong-code findings)
    let ub_reused = !crash && still && cur("ub_check") && old.contains_key("miri");
    let (m, miri_agrees) = if crash || !still {
        (MiriResult { verdict: "skipped".into(), detail: if crash { "not applicable (compiler crash)".into() } else { "does not reproduce".into() }, stdout: String::new() }, String::new())
    } else if ub_reused {
        let s = old.get("miri").cloned().unwrap_or_default();
        let (v, d) = s.split_once(": ").unwrap_or((s.as_str(), ""));
        (MiriResult { verdict: v.to_string(), detail: d.to_string(), stdout: String::new() }, old.get("miri_agrees").cloned().unwrap_or_default())
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

    // 6. upstream search (repeated after two weeks)
    let search_reused = cur("upstream") && dir.join("search.txt").exists();
    let search = if search_reused {
        search_from_text(&std::fs::read_to_string(dir.join("search.txt")).unwrap_or_default())
    } else if o.search {
        upstream_search(&reduced, &bad, &sig)
    } else {
        Search { note: "NOT PERFORMED (disabled)".into(), ..Default::default() }
    };

    // 7. regression range: only the parts that are still open
    let prior = std::fs::read_to_string(dir.join("bisect.txt")).ok().map(|t| crate::bisect::Bisect::from_text(&t)).or_else(|| crate::bisect::Bisect::from_legacy(&old));
    let prior_known = prior.is_some();
    let mut bis = prior.unwrap_or_default();
    if !o.bisect.is_zero() && still && m.verdict != "ub" {
        let yes = |ch: &str| matrix2.iter().any(|m| m.0 == ch && m.2 == Repro::Yes);
        let want = crate::bisect::Want { stable: !prior_known || !cur("bisect_stable"), nightly: !prior_known || !cur("bisect_nightly"), pr: !prior_known || !cur("bisect_pr") };
        if want.stable || want.nightly || want.pr {
            bis = crate::bisect::bisect(&reduced_path, &base, &bad, &sig, &o.work, o.bisect, want, yes("stable"), yes("nightly"), &bis);
        }
    }
    let _ = std::fs::write(dir.join("bisect.txt"), bis.to_text());

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
    if mir_reused {
        keep(&mut st, "mir_llvm");
    } else if mir_opt.starts_with("llvm") || mir_opt.starts_with("mir") {
        put(&mut st, "mir_llvm", "done", &ctx, mir_opt.clone());
    } else if mir_opt.starts_with("unknown: not reproduced") {
        put(&mut st, "mir_llvm", "n/a", &ctx, "does not reproduce on nightly".into());
    } else {
        put(&mut st, "mir_llvm", "todo", &ctx, mir_opt.clone());
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
    let cmd = |c: &Config| format!("rustc reduced.rs {} {}", common_txt, c.flags.join(" ")).split_whitespace().collect::<Vec<_>>().join(" ");
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
    match &outputs {
        Some((a, b)) if crash => {
            let a_out = if let Outcome::Ran { stdout, .. } = a { stdout.trim().to_string() } else { String::new() };
            let err = match b {
                Outcome::CompileFail(e) => crate::demangle::demangle_text(e.trim()).chars().take(20000).collect::<String>(),
                other => other.describe(),
            };
            let works = if a_out.is_empty() { "compiles and runs fine".to_string() } else { format!("compiles, and running the binary prints `{}`", a_out.lines().next().unwrap_or("")) };
            let _ = writeln!(is, "This command {works}:\n```\n{}\n```\nbut this one\n```\n{}\n```\nshows an error\n```\n{err}\n```\n", cmd(&base), cmd(&bad));
        }
        Some((a, b)) => {
            let show = |o: &Outcome| match o {
                Outcome::Ran { stdout, code, .. } if stdout.trim().is_empty() => format!("(no output, exit code {:?})", code),
                Outcome::Ran { stdout, .. } => stdout.trim().to_string(),
                other => other.describe(),
            };
            let _ = writeln!(is, "Compiled with\n```\n{}\n```\nit prints\n```\n{}\n```\nbut compiled with\n```\n{}\n```\nit prints\n```\n{}\n```\n", cmd(&base), show(a), cmd(&bad), show(b));
            if !miri_agrees.is_empty() {
                let _ = writeln!(is, "{miri_agrees}; the program is free of undefined behavior according to Miri.\n");
            }
        }
        None => {
            let _ = writeln!(is, "Commands:\n```\n{}\n{}\n```\n(did not reproduce when this draft was written)\n", cmd(&base), cmd(&bad));
        }
    }
    if let Some(l) = mir_opt.strip_prefix("llvm: ").or_else(|| mir_opt.strip_prefix("mir: ")) {
        let _ = writeln!(is, "{}\n", l.replace("does not reproduce", "The difference disappears").replace("still reproduces", "The difference remains").replacen(", so", "; so", 1).replace("with -Zmir-opt-level=0", "with `-Zmir-opt-level=0` (nightly)"));
    }
    let _ = writeln!(is, "### Meta\n\n`rustc --version --verbose`:\n```\n{vv}\n```\n");
    let _ = writeln!(is, "Problem was found by a rust fuzzer (created by an LLM) - https://github.com/qarmin/rs-compiler-fuzzer\n");
    let _ = writeln!(is, "| channel | reproducible |\n|---|---|");
    for (ch, ver, r) in &matrix2 {
        let _ = writeln!(is, "| {ch} ({ver}) | {} |", r.word());
    }
    for (label, yes) in bis.edges() {
        let _ = writeln!(is, "| {label} | {} |", if yes { "yes" } else { "no" });
    }
    if !bis.pr.lines.is_empty() {
        let _ = writeln!(is, "\nBisected with `cargo bisect-rustc`:\n");
        for l in &bis.pr.lines {
            let _ = writeln!(is, "{l}");
        }
    }
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

/// Stored findings (in an unpacked package) with open checklist steps, closest to completion first. `skip` = keys already handled by
/// this job. A finding whose only open step is the (long) minimisation is not started again on the day it was last triaged.
pub fn stale_findings(pkg: &Path, skip: &std::collections::HashSet<String>) -> Vec<PathBuf> {
    let root = pkg.join("findings");
    let today = today();
    let mut v: Vec<(usize, String, PathBuf)> = std::fs::read_dir(&root)
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
            let done = st.get("state").map(|s| s == "fixed").unwrap_or(false) || meta.get("mode").map(|m| m == "miri").unwrap_or(false) || skip.contains(&name);
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
            Some((open.len(), name, p))
        })
        .collect();
    v.sort();
    v.into_iter().map(|x| x.2).collect()
}
