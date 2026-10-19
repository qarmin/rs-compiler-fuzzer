//! `miri` mode: run generated programs under Miri. The generators are meant to produce UB-free programs, so
//!  - "Undefined Behavior" reports point at a generator bug (the usual case), a Miri false positive, or a
//!    real soundness bug in std/rustc (e.g. a safe API that is not),
//!  - differences between Miri's output and the natively compiled program (O0) point at a Miri bug, a
//!    rustc/LLVM bug, or at the generator relying on unspecified behaviour.
//! Findings land in `<out>/miri/<n>_<strategy>_<case>/` (case.rs, report.md, repro.cfg with `mode=miri`).

use crate::batch::{assemble, Case};
use crate::fuzz::{blamed_cases, generate_batch, Opts};
use crate::rng::Rng;
use crate::runner::{build_and_run, exec, Outcome};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const MIRI_TIMEOUT: Duration = Duration::from_secs(7 * 60);

struct Miri {
    proj: PathBuf,
    main: PathBuf,
}

struct MiriRun {
    ok: bool,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

impl Miri {
    fn new(work: &Path) -> Miri {
        let proj = work.join("miri_proj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"miri_fuzz\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n# the fuzz configs build with --cap-lints allow; the deny-by-default lints must not fail the build here\n[lints.rust]\noverflowing_literals = \"allow\"\narithmetic_overflow = \"allow\"\nunconditional_panic = \"allow\"\ninvalid_value = \"allow\"\n").unwrap();
        let main = proj.join("src/main.rs");
        Miri { proj, main }
    }

    fn run(&self, src: &str) -> MiriRun {
        std::fs::write(&self.main, src).unwrap();
        let mut c = Command::new("cargo");
        c.args(["+nightly", "miri", "run", "--release", "-q"])
            .current_dir(&self.proj)
            .env("MIRIFLAGS", "-Zmiri-disable-isolation")
            // the fuzz configs compare with `-C opt-level=0 -C overflow-checks=off`
            .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "0")
            .env("CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS", "false")
            .env("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "false");
        match exec(c, MIRI_TIMEOUT) {
            Ok(r) => MiriRun { ok: r.code == Some(0), timed_out: r.timed_out, stdout: String::from_utf8_lossy(&r.stdout).into_owned(), stderr: r.stderr },
            Err(e) => MiriRun { ok: false, timed_out: false, stdout: String::new(), stderr: e.to_string() },
        }
    }
}

fn parse_lines(stdout: &str) -> BTreeMap<usize, String> {
    stdout.lines().filter_map(|l| l.split_once(' ').and_then(|(id, rest)| Some((id.parse().ok()?, rest.to_string())))).collect()
}

/// The UB headline with addresses/numbers normalised, used to de-duplicate.
fn signature(stderr: &str) -> String {
    let l = stderr.lines().find(|l| l.contains("Undefined Behavior") || l.contains("unsupported operation") || l.starts_with("error")).unwrap_or("miri failure");
    let mut s = String::new();
    let mut prev_digit = false;
    for ch in l.trim().chars() {
        let d = ch.is_ascii_digit();
        if d && prev_digit {
            continue;
        }
        s.push(if d { '#' } else { ch });
        prev_digit = d;
    }
    s.chars().take(160).collect()
}

struct State<'a> {
    o: &'a Opts,
    miri: Miri,
    out: PathBuf,
    n: usize,
    seen: BTreeSet<String>,
    ub: usize,
    mismatch: usize,
    unsupported: usize,
    generator: usize,
}

impl State<'_> {
    fn save(&mut self, kind: &str, strategy: &str, case: &Case, sig: &str, detail: &str, batch_seed: u64) {
        if !self.seen.insert(format!("{kind}:{sig}")) {
            return;
        }
        self.n += 1;
        let dir = self.out.join("miri").join(format!("{}_{}_{}_{}", std::process::id() % 100000, self.n, strategy, kind));
        std::fs::create_dir_all(&dir).unwrap();
        let single = assemble(&[case]).source;
        std::fs::write(dir.join("case.rs"), &single).unwrap();
        std::fs::write(dir.join("signature.txt"), format!("{kind}: {sig}")).unwrap();
        let mut r = String::new();
        let _ = writeln!(r, "# Miri {kind}: strategy `{strategy}`, case {} (batch seed {batch_seed}, case seed {})\n", case.id, case.seed);
        let _ = writeln!(r, "`{sig}`\n");
        let _ = writeln!(r, "Run it: `cargo +nightly miri run` with case.rs as src/main.rs (`-Zmiri-disable-isolation`).\n");
        match kind {
            "ub" => {
                let _ = writeln!(r, "The generator should only produce UB-free programs, so one of these holds:\n1. **generator bug** (most likely): fix the strategy or avoid the construct;\n2. Miri false positive (check the Miri issue tracker, e.g. Stacked/Tree Borrows strictness);\n3. real soundness bug in std/rustc (a safe API causing UB) - then reduce and report upstream.\n");
            }
            "mismatch" => {
                let _ = writeln!(r, "Miri's output differs from the native O0 build, so one of: Miri bug, rustc/LLVM miscompilation at O0, or the generator depends on unspecified behaviour (layout, padding, NaN payloads, address values).\n");
            }
            _ => {}
        }
        let _ = writeln!(r, "```\n{}\n```", detail.chars().take(6000).collect::<String>());
        std::fs::write(dir.join("report.md"), r).unwrap();
        let cfg = format!("strategy={strategy}\nbase_name=native\nbase_flags=\nbad_name=miri-{kind}\nbad_flags=\nbatch_seed={batch_seed}\nmode=miri\ntoolchain=nightly\n");
        std::fs::write(dir.join("repro.cfg"), cfg).unwrap();
        eprintln!("  MIRI {kind}: {sig}  -> {}", dir.display());
    }

    fn batch(&mut self, strategy: &'static str, cases: Vec<Case>, batch_seed: u64) {
        self.batch_at(strategy, cases, batch_seed, 0);
    }

    fn batch_at(&mut self, strategy: &'static str, cases: Vec<Case>, batch_seed: u64, depth: u32) {
        let mut cases = cases;
        let native_cfg = self.o.configs[0].clone();
        for attempt in 0..12 {
            if cases.is_empty() {
                return;
            }
            let refs: Vec<&Case> = cases.iter().collect();
            let a = assemble(&refs);
            let t0 = Instant::now();
            let r = self.miri.run(&a.source);
            eprintln!("  miri attempt {attempt}: {} cases, {} lines, {:.0}s, {}", cases.len(), a.source.lines().count(), t0.elapsed().as_secs_f32(), if r.ok { "ok" } else if r.timed_out { "TIMEOUT" } else { "failed" });
            if r.timed_out {
                // some case is (much) slower under Miri than the rest: find the slow part by splitting into chunks
                if depth == 0 && cases.len() >= 8 {
                    eprintln!("  Miri timed out; retrying in 4 chunks (batch seed {batch_seed})");
                    let chunk = cases.len().div_ceil(4);
                    for part in cases.chunks(chunk) {
                        self.batch_at(strategy, part.to_vec(), batch_seed, 1);
                    }
                } else {
                    eprintln!("  Miri timed out on {} cases; skipping them (batch seed {batch_seed})", cases.len());
                }
                return;
            }
            if r.ok {
                // compare with the native build
                let src = self.out.join("work").join(format!("miri_native_{}.rs", std::process::id()));
                std::fs::write(&src, &a.source).unwrap();
                let bin = self.out.join("work").join(format!("miri_native_{}", std::process::id()));
                let native = build_and_run(&self.o.rustc, &src, &bin, &native_cfg, &self.o.limits);
                let _ = std::fs::remove_file(&src);
                let _ = std::fs::remove_file(&bin);
                if let Outcome::Ran { stdout, .. } = native {
                    let (m, n) = (parse_lines(&r.stdout), parse_lines(&stdout));
                    for c in &cases {
                        if m.get(&c.id) != n.get(&c.id) {
                            self.mismatch += 1;
                            let d = format!("miri: {:?}\nnative {}: {:?}", m.get(&c.id), native_cfg.name, n.get(&c.id));
                            self.save("mismatch", strategy, c, &format!("{strategy} case differs between Miri and native"), &d, batch_seed);
                        }
                    }
                }
                return;
            }
            let blamed = blamed_cases(&r.stderr, &a, None);
            let sig = signature(&r.stderr);
            let compile_error = !r.stderr.contains("Undefined Behavior") && !r.stderr.contains("unsupported operation") && (r.stderr.contains("error[E") || r.stderr.contains("could not compile") || r.stderr.lines().any(|l| l.starts_with("error:")) && !r.stderr.contains("the evaluated program"));
            if blamed.is_empty() {
                eprintln!("  Miri failed, not attributable to a case: {sig}\n{}", r.stderr.chars().take(1500).collect::<String>());
                return;
            }
            for c in cases.iter().filter(|c| blamed.contains(&c.id)) {
                if compile_error {
                    self.generator += 1;
                } else if r.stderr.contains("Undefined Behavior") {
                    self.ub += 1;
                    self.save("ub", strategy, c, &sig, &r.stderr, batch_seed);
                } else {
                    // e.g. "unsupported operation": Miri cannot run this construct (inline asm, some intrinsics)
                    self.unsupported += 1;
                    self.save("unsupported", strategy, c, &sig, &r.stderr, batch_seed);
                }
            }
            cases.retain(|c| !blamed.contains(&c.id));
        }
    }
}

/// Lines per batch that Miri digests in a few minutes (collections is ~100x slower per line than arith).
fn miri_cap(strategy: &str) -> usize {
    match strategy {
        "collections" => 300,
        "loops" | "recursion" | "floats" => 1500,
        _ => 2500,
    }
}

pub fn run(o: &Opts) -> i32 {
    let work = o.out.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(o.out.join("miri")).unwrap();
    let have = {
        let mut c = Command::new("cargo");
        c.args(["+nightly", "miri", "--version"]);
        exec(c, Duration::from_secs(120)).map(|r| r.code == Some(0)).unwrap_or(false)
    };
    if !have {
        eprintln!("Miri is not installed: rustup component add miri rust-src --toolchain nightly");
        return 1;
    }
    eprintln!("miri mode: configs for the native comparison: {}", o.configs[0].name);
    let mut st = State { o, miri: Miri::new(&work), out: o.out.clone(), n: 0, seen: BTreeSet::new(), ub: 0, mismatch: 0, unsupported: 0, generator: 0 };
    let start = Instant::now();
    let mut round = 0u64;
    let mut longest = Duration::ZERO;
    'outer: loop {
        if o.rounds.is_some_and(|r| round >= r) || o.duration.is_some_and(|d| start.elapsed() >= d) {
            break;
        }
        for (si, s) in o.strategies.iter().enumerate() {
            if o.duration.is_some_and(|d| start.elapsed() + longest >= d) && longest > Duration::ZERO {
                break 'outer;
            }
            let batch_seed = o.batch_seed.unwrap_or_else(|| Rng::mix(o.seed, round * 1000 + si as u64));
            // Miri is orders of magnitude slower than native code: much smaller batches
            let lines = (o.target_lines.unwrap_or(s.lines.min(miri_cap(s.name))) * o.scale / 100).max(200);
            let t0 = Instant::now();
            eprintln!("[{}] seed {batch_seed}, ~{lines} lines", s.name);
            let cases = generate_batch(s, batch_seed, lines);
            st.batch(s.name, cases, batch_seed);
            longest = longest.max(t0.elapsed());
        }
        round += 1;
    }
    eprintln!("miri done: {} rounds in {:.0}s; UB reports {}, Miri-vs-native mismatches {}, unsupported {}, generator compile errors {}", round, start.elapsed().as_secs_f32(), st.ub, st.mismatch, st.unsupported, st.generator);
    let _ = std::fs::remove_dir_all(&work);
    // unsupported operations are expected for a few constructs; UB and mismatches are the interesting ones
    if st.ub + st.mismatch > 0 {
        2
    } else {
        0
    }
}
