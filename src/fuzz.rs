//! The orchestrator: generate batches, compile them with many configurations, compare, and
//! shrink any divergence down to the few cases responsible.

use crate::batch::{assemble, Assembled, Case};
use crate::rng::Rng;
use crate::runner::{build_and_run, check_only, par_map, rustc_cmd, Config, Limits, Outcome};
use crate::strategies::{Strategy, ALL};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Opts {
    pub seed: u64,
    pub duration: Option<Duration>,
    pub rounds: Option<u64>,
    /// Fixed batch size for every strategy; `None` = per-strategy default.
    pub target_lines: Option<usize>,
    /// Percentage applied to the batch size (100 = as is).
    pub scale: usize,
    pub strategies: Vec<&'static Strategy>,
    pub out: PathBuf,
    pub jobs: usize,
    pub rustc: Vec<String>,
    pub configs: Vec<Config>,
    pub limits: Limits,
    pub max_findings: usize,
    pub minimize_budget: usize,
    /// Count findings that match a known upstream bug (`// known-bug:` marker) towards the exit code.
    pub fail_on_known: bool,
    /// Report compile timeouts as findings (default: only log them - big batches under LTO can
    /// legitimately be slow, and a timeout cannot be attributed to a case).
    pub timeouts_are_findings: bool,
    /// Use exactly this batch seed (the `seed N` printed in the logs) instead of deriving one.
    pub batch_seed: Option<u64>,
}

/// Configurations that must agree for any UB-free program. The first one is the baseline.
pub fn default_configs(nightly: bool) -> Vec<Config> {
    let mut v = vec![
        Config::new("O0", "-C opt-level=0"),
        Config::new("O1", "-C opt-level=1"),
        Config::new("O2", "-C opt-level=2"),
        Config::new("O3", "-C opt-level=3"),
        Config::new("Os", "-C opt-level=s"),
        Config::new("Oz", "-C opt-level=z"),
        Config::new("O3-cgu1", "-C opt-level=3 -C codegen-units=1"),
        Config::new("O3-lto-fat", "-C opt-level=3 -C lto=fat -C codegen-units=1"),
        Config::new("O2-native", "-C opt-level=2 -C target-cpu=native"),
        Config::new("O1-checks", "-C opt-level=1 -C overflow-checks=on -C debug-assertions=on"),
        Config::new("O3-checks", "-C opt-level=3 -C overflow-checks=on -C debug-assertions=on"),
    ];
    if nightly {
        v.push(Config::new("O3-mir0", "-C opt-level=3 -Z mir-opt-level=0"));
        v.push(Config::new("O3-mir3", "-C opt-level=3 -Z mir-opt-level=3"));
        v.push(Config::new("O0-mir3", "-C opt-level=0 -Z mir-opt-level=3"));
        v.push(Config::new("O2-validate-mir", "-C opt-level=2 -Z validate-mir"));
    }
    v
}

pub fn rustc_version(rustc: &[String]) -> String {
    let mut c = rustc_cmd(rustc);
    c.arg("-vV");
    match crate::runner::exec(c, Duration::from_secs(60)) {
        Ok(r) => String::from_utf8_lossy(&r.stdout).into_owned(),
        Err(e) => format!("unknown ({e})"),
    }
}

pub fn is_nightly(version: &str) -> bool {
    version.contains("-nightly") || version.contains("-dev")
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Fuzzer<'a> {
    o: &'a Opts,
    version: String,
    work: PathBuf,
    findings: usize,
    known: usize,
    seen_crashes: BTreeSet<(String, String)>,
}

pub fn generate_batch(s: &Strategy, batch_seed: u64, target_lines: usize) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut lines = 0;
    let mut idx = 0;
    while lines < target_lines {
        let seed = Rng::mix(batch_seed, idx as u64);
        let body = (s.generate)(&mut Rng::new(seed));
        lines += body.lines().count() + 3;
        cases.push(Case { id: idx, strategy: s.name, seed, body });
        idx += 1;
    }
    cases
}

/// Case ids whose line span is mentioned in rustc diagnostics (`--> file:LINE:COL`).
fn blamed_cases(stderr: &str, a: &Assembled) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for l in stderr.lines() {
        let t = l.trim_start();
        let rest = match t.strip_prefix("--> ").or_else(|| t.strip_prefix("::: ")) {
            Some(r) => r,
            None => continue,
        };
        let mut parts = rest.rsplitn(3, ':');
        let _col = parts.next();
        let line: usize = match parts.next().and_then(|x| x.parse().ok()) {
            Some(n) => n,
            None => continue,
        };
        for &(id, s, e) in &a.spans {
            if line >= s && line <= e {
                out.insert(id);
            }
        }
    }
    out
}

fn parse_lines(stdout: &str) -> BTreeMap<usize, String> {
    stdout
        .lines()
        .filter_map(|l| {
            let (id, rest) = l.split_once(' ')?;
            Some((id.parse().ok()?, rest.to_string()))
        })
        .collect()
}

/// Strategies tag snippets that are known to trigger an open upstream bug with
/// `// known-bug: <reference>`; findings made only of such cases are filed under `known/`.
fn known_bug(body: &str) -> Option<&str> {
    let l = body.lines().find(|l| l.contains("// known-bug:"))?;
    Some(l.split("// known-bug:").nth(1)?.trim())
}

const KNOWN_BUGS: &str = include_str!("../known-bugs.txt");

/// Is this crash signature listed in known-bugs.txt?
fn known_signature(sig: &str) -> Option<&'static str> {
    KNOWN_BUGS
        .lines()
        .map(|l| l.split("    #").next().unwrap_or(l).trim())
        .find(|l| !l.is_empty() && !l.starts_with('#') && sig.contains(l))
}

fn append_config(dir: &Path, cfg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(dir.join("report.md")) {
        let _ = writeln!(f, "\n- also diverges from the baseline under: `{cfg}`");
    }
}

/// Stable fingerprint of a compiler crash, so the same crash found again (other batch, other
/// run) is recognised: the first line naming an LLVM error / ICE / signal, digits normalised.
fn crash_signature(o: &Outcome) -> Option<String> {
    let Outcome::CompileFail(e) = o else { return None };
    let line = e
        .lines()
        .find(|l| l.contains("LLVM ERROR") || l.contains("internal compiler error") || l.contains("interrupted by SIG") || l.contains("panicked at") || l.contains("Assertion"))
        .or_else(|| e.lines().find(|l| l.starts_with("error")))?;
    let norm: String = line.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    Some(norm.chars().take(160).collect())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Fuzzer<'_> {
    fn path(&self, suffix: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        self.work.join(format!("t{}_{}{}", std::process::id(), n, suffix))
    }

    fn eval(&self, a: &Assembled, cfg: &Config) -> Outcome {
        let src = self.path(".rs");
        std::fs::write(&src, &a.source).unwrap();
        let bin = self.path(".bin");
        let r = build_and_run(&self.o.rustc, &src, &bin, cfg, &self.o.limits);
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&bin);
        r
    }

    /// Compare baseline vs `bad` on exactly these cases.
    fn diverges(&self, cases: &[Case], base: &Config, bad: &Config) -> Option<(Outcome, Outcome)> {
        let refs: Vec<&Case> = cases.iter().collect();
        let a = assemble(&refs);
        let r = par_map(&[base, bad], 2, |c| self.eval(&a, c));
        if r[0].key() != r[1].key() {
            Some((r[0].clone(), r[1].clone()))
        } else {
            None
        }
    }

    fn minimize(&self, all: &[Case], base: &Config, bad: &Config, base_out: &Outcome, bad_out: &Outcome) -> Vec<Case> {
        // Shortcut: stdout lines identify the diverging cases directly.
        let mut cur: Vec<Case> = all.to_vec();
        if let (Outcome::Ran { stdout: x, .. }, Outcome::Ran { stdout: y, .. }) = (base_out, bad_out) {
            let (lx, ly) = (parse_lines(x), parse_lines(y));
            let ids: BTreeSet<usize> =
                lx.keys().chain(ly.keys()).copied().filter(|k| lx.get(k) != ly.get(k)).collect();
            let sub: Vec<Case> = all.iter().filter(|c| ids.contains(&c.id)).cloned().collect();
            if !sub.is_empty() && sub.len() < all.len() && self.diverges(&sub, base, bad).is_some() {
                cur = sub;
            }
        }
        // ddmin-style: drop chunks while the divergence persists.
        let mut tests = 0;
        let mut chunk = (cur.len() / 2).max(1);
        loop {
            let mut i = 0;
            let mut progress = false;
            while i < cur.len() && cur.len() > 1 {
                if tests >= self.o.minimize_budget {
                    return cur;
                }
                let end = (i + chunk).min(cur.len());
                let mut cand: Vec<Case> = cur[..i].to_vec();
                cand.extend_from_slice(&cur[end..]);
                tests += 1;
                if self.diverges(&cand, base, bad).is_some() {
                    cur = cand;
                    progress = true;
                } else {
                    i = end;
                }
            }
            if chunk == 1 {
                if !progress {
                    break;
                }
            } else {
                chunk = (chunk / 2).max(1);
            }
        }
        cur
    }

    fn write_finding(&mut self, kind: &str, strategy: &str, cases: &[Case], batch_seed: u64, base: (&Config, &Outcome), bad: (&Config, &Outcome)) -> PathBuf {
        let known: BTreeSet<&str> = cases.iter().filter_map(|c| known_bug(&c.body)).collect();
        let sig_known = crash_signature(bad.1).and_then(|s| known_signature(&s)).is_some();
        let is_known = sig_known || (!cases.is_empty() && cases.iter().all(|c| known_bug(&c.body).is_some()));
        let (bucket, n) = if is_known { self.known += 1; ("known", self.known) } else { self.findings += 1; ("findings", self.findings) };
        let dir = self.o.out.join(bucket).join(format!("{}_{}_{}_{}", now_secs(), strategy, bad.0.name, n));
        std::fs::create_dir_all(&dir).unwrap();
        let refs: Vec<&Case> = cases.iter().collect();
        let a = assemble(&refs);
        std::fs::write(dir.join("case.rs"), &a.source).unwrap();
        let mut r = String::new();
        let _ = writeln!(r, "# {kind}: `{}` vs `{}` ({strategy})\n", base.0.name, bad.0.name);
        if is_known {
            let _ = writeln!(r, "**Matches a known bug** ({}) - not counted as a new finding.\n", if known.is_empty() { "known-bugs.txt".to_string() } else { known.iter().copied().collect::<Vec<_>>().join(", ") });
        }
        let _ = writeln!(r, "- cases kept after minimisation: {}", cases.iter().map(|c| format!("`case_{}` ({}, seed {})", c.id, c.strategy, c.seed)).collect::<Vec<_>>().join(", "));
        let _ = writeln!(r, "- batch seed: {batch_seed}, fuzzer seed: {}", self.o.seed);
        let _ = writeln!(r, "- regenerate one case: `rs-compiler-fuzzer gen --strategy {strategy} --case-seed <seed>` (seed listed above)\n");
        let _ = writeln!(r, "## Toolchain\n```\n{}\n```\n", self.version.trim());
        for (cfg, out) in [base, bad] {
            let _ = writeln!(r, "## `{}`: {}\n", cfg.name, out.describe());
            let _ = writeln!(r, "```\nrustc case.rs {} {}\n```", crate::runner::COMMON_FLAGS.join(" "), cfg.flags.join(" "));
            match out {
                Outcome::CompileFail(e) => {
                    let _ = writeln!(r, "stderr (truncated):\n```\n{}\n```", e.chars().take(6000).collect::<String>());
                }
                Outcome::Ran { stdout, stderr, .. } => {
                    let _ = writeln!(r, "stdout:\n```\n{}\n```", stdout.lines().take(60).collect::<Vec<_>>().join("\n"));
                    if !stderr.trim().is_empty() {
                        let _ = writeln!(r, "stderr:\n```\n{}\n```", stderr.chars().take(3000).collect::<String>());
                    }
                }
                _ => {}
            }
        }
        let _ = writeln!(r, "\nBefore reporting upstream: check the case is free of UB (e.g. run it under Miri) and shrink it by hand; the `*-checks` configs and `PANIC` lines can also indicate a generator bug rather than a compiler bug.");
        std::fs::write(dir.join("report.md"), r).unwrap();
        let meta = format!(
            "strategy={strategy}\nbase_name={}\nbase_flags={}\nbad_name={}\nbad_flags={}\nbatch_seed={batch_seed}\ntoolchain={}\n",
            base.0.name,
            base.0.flags.join(" "),
            bad.0.name,
            bad.0.flags.join(" "),
            self.version.lines().find(|l| l.starts_with("release:")).unwrap_or("release: ?").trim_start_matches("release: ")
        );
        std::fs::write(dir.join("repro.cfg"), meta).unwrap();
        if let Some(sig) = crash_signature(bad.1) {
            std::fs::write(dir.join("signature.txt"), sig).unwrap();
        }
        eprintln!("  {} {kind} `{}` vs `{}` -> {}", if is_known { "~~ (known)" } else { "!!" }, base.0.name, bad.0.name, dir.display());
        dir
    }

    fn process_batch(&mut self, s: &Strategy, batch_seed: u64, mut cases: Vec<Case>) {
        let t0 = Instant::now();
        let total_cases = cases.len();
        // 1. cheap check pass: drop cases the generator got wrong (type/borrow errors).
        let mut dropped = 0;
        for _ in 0..12 {
            let refs: Vec<&Case> = cases.iter().collect();
            let a = assemble(&refs);
            let src = self.path(".rs");
            std::fs::write(&src, &a.source).unwrap();
            let r = check_only(&self.o.rustc, &src, &self.work, self.o.limits.compile);
            let _ = std::fs::remove_file(&src);
            let err = match r {
                Ok(()) => break,
                Err(e) => e,
            };
            let blamed = blamed_cases(&err, &a);
            if blamed.is_empty() {
                eprintln!("  generator error not attributable to a case; aborting batch:\n{}", err.chars().take(1500).collect::<String>());
                return;
            }
            for c in cases.iter().filter(|c| blamed.contains(&c.id)) {
                self.save_generator_bug(s.name, c, &err);
            }
            dropped += blamed.len();
            cases.retain(|c| !blamed.contains(&c.id));
        }
        let t_check = t0.elapsed();

        // 2. compile + run under all configurations.
        let refs: Vec<&Case> = cases.iter().collect();
        let a = assemble(&refs);
        let lines = a.source.lines().count();
        let outs = par_map(&self.o.configs, self.o.jobs, |c| self.eval(&a, c));
        let keys: Vec<String> = outs.iter().map(Outcome::key).collect();
        // Baseline = first configuration that compiled and ran. If e.g. O0 crashes the compiler, the
        // others are still compared with each other and O0 is reported as a compile failure.
        let b = outs.iter().position(|o| matches!(o, Outcome::Ran { .. })).unwrap_or(0);
        let diverging: Vec<usize> = (0..outs.len())
            .filter(|&i| i != b && keys[i] != keys[b])
            .filter(|&i| {
                let skip = matches!(outs[i], Outcome::CompileTimeout) && !self.o.timeouts_are_findings;
                if skip {
                    eprintln!("  (compile of `{}` timed out - inconclusive, not reported; use --timeouts-are-findings)", self.o.configs[i].name);
                }
                !skip
            })
            .collect();
        eprintln!(
            "[{}] seed {batch_seed}: {} cases ({} dropped as invalid), {lines} lines, check {:.1}s, total {:.1}s, divergent configs: {}",
            s.name,
            cases.len(),
            dropped,
            t_check.as_secs_f32(),
            t0.elapsed().as_secs_f32(),
            diverging.iter().map(|&i| self.o.configs[i].name.as_str()).collect::<Vec<_>>().join(",")
        );
        let _ = total_cases;

        if diverging.is_empty() {
            let all_same = keys.iter().all(|k| *k == keys[0]);
            if all_same && (matches!(outs[0], Outcome::CompileFail(_) | Outcome::RunTimeout) || (self.o.timeouts_are_findings && matches!(outs[0], Outcome::CompileTimeout))) {
                let _ = self.write_finding("uniform failure", s.name, &cases, batch_seed, (&self.o.configs[0], &outs[0]), (&self.o.configs[1], &outs[1]));
            }
            return;
        }
        // One finding per root cause: later configurations whose culprit cases were already
        // minimised are attached to the existing finding instead of getting their own.
        let mut handled: Vec<(BTreeSet<usize>, PathBuf)> = Vec::new();
        for i in diverging {
            if self.findings >= self.o.max_findings {
                return;
            }
            let (base, bad) = (&self.o.configs[b], &self.o.configs[i]);
            if let Some(sig) = crash_signature(&outs[i]) {
                if !self.seen_crashes.insert((bad.name.clone(), sig.clone())) {
                    eprintln!("  (same crash as before under `{}`: {sig})", bad.name);
                    continue;
                }
            }
            if let (Outcome::Ran { stdout: x, .. }, Outcome::Ran { stdout: y, .. }) = (&outs[b], &outs[i]) {
                let (lx, ly) = (parse_lines(x), parse_lines(y));
                let ids: BTreeSet<usize> = lx.keys().chain(ly.keys()).copied().filter(|k| lx.get(k) != ly.get(k)).collect();
                if let Some((_, dir)) = handled.iter().find(|(h, _)| !ids.is_empty() && ids.is_subset(h)) {
                    append_config(dir, &bad.name);
                    continue;
                }
            }
            let min = self.minimize(&cases, base, bad, &outs[b], &outs[i]);
            let ids: BTreeSet<usize> = min.iter().map(|c| c.id).collect();
            if let Some((_, dir)) = handled.iter().find(|(h, _)| *h == ids) {
                append_config(dir, &bad.name);
                continue;
            }
            // re-evaluate the minimised set to record what each side really did
            let (bo, xo) = self.diverges(&min, base, bad).unwrap_or_else(|| (outs[b].clone(), outs[i].clone()));
            let kind = match (&bo, &xo) {
                (Outcome::Ran { .. }, Outcome::Ran { .. }) => "MISCOMPILE candidate",
                (_, Outcome::CompileFail(_)) | (_, Outcome::CompileTimeout) => "compile failure/hang",
                _ => "behaviour divergence",
            };
            let dir = self.write_finding(kind, s.name, &min, batch_seed, (base, &bo), (bad, &xo));
            handled.push((ids, dir));
        }
    }

    fn save_generator_bug(&self, strategy: &str, c: &Case, err: &str) {
        let dir = self.o.out.join("generator-bugs");
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("{strategy}_{}", c.seed);
        let text = format!("// strategy {strategy}, case seed {}\n// rustc said:\n{}\n{}", c.seed, err.lines().take(25).map(|l| format!("// {l}")).collect::<Vec<_>>().join("\n"), c.body);
        let _ = std::fs::write(dir.join(format!("{name}.rs")), text);
    }
}

pub fn run(o: &Opts) -> i32 {
    let version = rustc_version(&o.rustc);
    eprintln!("toolchain: {}", version.lines().find(|l| l.starts_with("release") || l.starts_with("rustc")).unwrap_or("?"));
    eprintln!("configs: {}", o.configs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "));
    let work = o.out.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut f = Fuzzer { o, version, work, findings: 0, known: 0, seen_crashes: BTreeSet::new() };
    let start = Instant::now();
    let mut round = 0u64;
    loop {
        if o.rounds.is_some_and(|r| round >= r) || o.duration.is_some_and(|d| start.elapsed() >= d) {
            break;
        }
        for (si, s) in o.strategies.iter().enumerate() {
            if o.duration.is_some_and(|d| start.elapsed() >= d) || f.findings >= o.max_findings {
                break;
            }
            let batch_seed = o.batch_seed.unwrap_or_else(|| Rng::mix(o.seed, round * 1000 + si as u64));
            let lines = o.target_lines.unwrap_or(s.lines) * o.scale / 100;
            let cases = generate_batch(s, batch_seed, lines.max(200));
            f.process_batch(s, batch_seed, cases);
        }
        round += 1;
        if f.findings >= o.max_findings {
            eprintln!("max findings reached");
            break;
        }
    }
    eprintln!("done: {round} rounds in {:.0}s, {} new findings, {} matching known bugs", start.elapsed().as_secs_f32(), f.findings, f.known);
    let _ = std::fs::remove_dir_all(&f.work);
    if f.findings > 0 || (o.fail_on_known && f.known > 0) { 2 } else { 0 }
}

/// Known miscompilations - prove the detector can fire at all. The bugs get fixed over time,
/// so the check passes when at least one canary diverges between configurations.
const CANARIES: &[(&str, &str)] = &[
    ("rust-lang/rust#164297 (lto=fat)", include_str!("../canaries/case_164297.rs")),
    ("rust-lang/rust#164295 (-O, LLVM 23)", include_str!("../canaries/case_164295.rs")),
    ("rust-lang/rust#163311 (-O2/-O3)", include_str!("../canaries/case_163311.rs")),
];

pub fn canary(o: &Opts) -> i32 {
    let work = o.out.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut detected = 0;
    for (name, src) in CANARIES {
        let path = work.join("canary.rs");
        std::fs::write(&path, src).unwrap();
        let mut keys = Vec::new();
        let mut shown = Vec::new();
        for c in &o.configs {
            let bin = work.join(format!("canary_{}", c.name));
            let r = build_and_run(&o.rustc, &path, &bin, c, &o.limits);
            shown.push(match &r {
                Outcome::Ran { stdout, signal, code, .. } => {
                    format!("{}={}", c.name, if stdout.trim().is_empty() { format!("exit {code:?}/{signal:?}") } else { stdout.trim().to_string() })
                }
                other => format!("{}={}", c.name, other.describe()),
            });
            keys.push(r.key());
        }
        let diverges = keys.iter().any(|k| *k != keys[0]);
        detected += diverges as usize;
        eprintln!("  {} {name}: {}", if diverges { "DIVERGES" } else { "same    " }, shown.join(" "));
    }
    let _ = std::fs::remove_dir_all(&work);
    if detected > 0 {
        eprintln!("canary: detector works ({detected}/{} known bugs reproduce on this toolchain)", CANARIES.len());
        0
    } else {
        eprintln!("canary: NO known bug reproduced - all fixed on this toolchain, or the detector is broken");
        1
    }
}

/// Re-run stored findings (dirs with `case.rs` + `repro.cfg`) on the current toolchain.
/// Prints `STILL <dir>` / `FIXED <dir>` / `SKIP <dir>` and writes `fixed.txt` into `report_dir`.
pub fn verify(o: &Opts, root: &Path, report_dir: &Path) -> i32 {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("case.rs").exists() && p.join("repro.cfg").exists()).collect(),
        Err(e) => {
            eprintln!("cannot read {}: {e}", root.display());
            return 1;
        }
    };
    dirs.sort();
    let work = o.out.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(report_dir).unwrap();
    let (mut still, mut fixed) = (Vec::new(), Vec::new());
    let jobs = o.jobs;
    let results = par_map(&dirs, jobs, |d| {
        let meta: std::collections::HashMap<String, String> = std::fs::read_to_string(d.join("repro.cfg"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        let get = |k: &str| meta.get(k).cloned().unwrap_or_default();
        let base = Config::new(&get("base_name"), &get("base_flags"));
        let bad = Config::new(&get("bad_name"), &get("bad_flags"));
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let src = d.join("case.rs");
        let r: Vec<Outcome> = [&base, &bad]
            .iter()
            .enumerate()
            .map(|(i, c)| build_and_run(&o.rustc, &src, &work.join(format!("v{}_{n}_{i}", std::process::id())), c, &o.limits))
            .collect();
        (r[0].key() != r[1].key(), r[0].describe(), r[1].describe(), get("bad_name"))
    });
    for (d, (diverges, b, x, badname)) in dirs.iter().zip(results) {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        if diverges {
            eprintln!("STILL {name} [{badname}] baseline: {b}; bad: {x}");
            still.push(name);
        } else {
            eprintln!("FIXED {name} [{badname}]");
            fixed.push(name);
        }
    }
    let _ = std::fs::write(report_dir.join("fixed.txt"), fixed.join("\n"));
    let _ = std::fs::write(report_dir.join("still.txt"), still.join("\n"));
    eprintln!("verify: {} still reproduce, {} fixed/not reproducing", still.len(), fixed.len());
    let _ = std::fs::remove_dir_all(&work);
    0
}

pub fn strategies_by_name(names: &str) -> Result<Vec<&'static Strategy>, String> {
    if names == "all" {
        return Ok(ALL.iter().collect());
    }
    names.split(',').map(|n| crate::strategies::find(n.trim()).ok_or_else(|| format!("unknown strategy `{n}`"))).collect()
}

pub fn ensure_dir(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::COMMON_FLAGS;

    #[test]
    fn generation_is_deterministic() {
        for s in ALL {
            let a = generate_batch(s, 99, 800);
            let b = generate_batch(s, 99, 800);
            assert_eq!(a.len(), b.len());
            assert!(a.iter().zip(&b).all(|(x, y)| x.body == y.body), "{} is not deterministic", s.name);
        }
    }

    /// Every strategy must emit code that type- and borrow-checks (a generator that emits
    /// invalid programs wastes whole batches).
    #[test]
    fn every_strategy_typechecks() {
        let dir = std::env::temp_dir().join(format!("rsfuzz_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = COMMON_FLAGS;
        for s in ALL {
            for seed in 0..4u64 {
                let cases = generate_batch(s, 1000 + seed, 1500);
                let refs: Vec<&Case> = cases.iter().collect();
                let a = assemble(&refs);
                let src = dir.join(format!("{}_{seed}.rs", s.name));
                std::fs::write(&src, &a.source).unwrap();
                if let Err(e) = check_only(&["rustc".to_string()], &src, &dir, Duration::from_secs(300)) {
                    let blamed = blamed_cases(&e, &a);
                    panic!("strategy {} seed {} produced invalid code (cases {:?}):\n{}", s.name, 1000 + seed, blamed, e.chars().take(3000).collect::<String>());
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
