//! `miri` mode: run generated programs under Miri. The generators are meant to produce UB-free programs, so
//!  - "Undefined Behavior" reports point at a generator bug (the usual case), a Miri false positive, or a
//!    real soundness bug in std/rustc (e.g. a safe API that is not),
//!  - differences between Miri's output and the natively compiled program (O0) point at a Miri bug, a
//!    rustc/LLVM bug, or at the generator relying on unspecified behaviour.
//! Findings land in `<out>/miri/<n>_<strategy>_<case>/` (case.rs, report.md, repro.cfg with `mode=miri`).

use log::{info, warn};
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
            .env("MIRIFLAGS", "-Zmiri-disable-isolation -Zmiri-ignore-leaks")
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

fn tool_versions() -> (String, String) {
    let run = |args: &[&str], prog: &str| {
        let mut c = Command::new(prog);
        c.args(args);
        exec(c, Duration::from_secs(60)).map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string()).unwrap_or_default()
    };
    (run(&["+nightly", "-vV"], "rustc"), format!("miri: {}", run(&["+nightly", "miri", "--version"], "cargo")))
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
    /// (`rustc +nightly -vV`, `cargo +nightly miri --version`)
    versions: (String, String),
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
        let native = &self.o.configs[0];
        let native_cmd = format!("rustc case.rs {} {} -o case && ./case", crate::runner::COMMON_FLAGS.join(" "), native.flags.join(" ")).replace("  ", " ");
        let miri_cmd = "cargo new miri_case && cp case.rs miri_case/src/main.rs && cd miri_case\n\
            MIRIFLAGS=\"-Zmiri-disable-isolation -Zmiri-ignore-leaks\" CARGO_PROFILE_RELEASE_OPT_LEVEL=0 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=false \\\n\
            CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=false cargo +nightly miri run --release";
        let what = match kind {
            "ub" => "Miri reports undefined behavior in a program that is meant to be UB-free",
            "mismatch" => "Miri's output differs from the natively compiled program",
            _ => "Miri cannot run this program",
        };
        let mut r = String::new();
        let _ = writeln!(r, "# Miri {kind}: strategy `{strategy}`, case {} (batch seed {batch_seed}, case seed {})\n", case.id, case.seed);
        let _ = writeln!(r, "{what}: `{sig}`\n");
        let _ = writeln!(r, "## How to reproduce\n\n`case.rs` in this directory is the single failing case (everything needed is in the file).\n");
        let _ = writeln!(r, "Under Miri:\n```\n{miri_cmd}\n```\nNative:\n```\n{native_cmd}\n```\n");
        let _ = writeln!(r, "Regenerate just this case: `rs-compiler-fuzzer gen --strategy {strategy} --case-seed {}`\n", case.seed);
        let _ = writeln!(r, "## Toolchain\n\n```\n{}\n{}\n```\n", self.versions.0, self.versions.1);
        match kind {
            "ub" => {
                let _ = writeln!(r, "## What it probably is\n\nThe generators only produce UB-free programs, so one of:\n1. **generator bug** (most likely) - fix the strategy or avoid the construct;\n2. Miri false positive (check the Miri issue tracker; Stacked/Tree Borrows are strict);\n3. a real soundness bug in std/rustc (a safe API causing UB) - then reduce and report upstream.\n");
            }
            "mismatch" => {
                let _ = writeln!(r, "## What it probably is\n\nOne of: Miri bug, rustc/LLVM miscompilation at `{}`, or the generator depends on unspecified behaviour (layout, padding, NaN sign/payload, addresses, CPU feature detection).\n", native.name);
            }
            _ => {}
        }
        let _ = writeln!(r, "## Output\n\n```\n{}\n```", detail.chars().take(8000).collect::<String>());
        std::fs::write(dir.join("report.md"), &r).unwrap();

        let mut is = String::new();
        let _ = writeln!(is, "<!-- Draft generated by rs-compiler-fuzzer's Miri mode; check that it is not a generator bug before submitting. -->\n");
        let _ = writeln!(is, "I tried this code:\n\n```rust\n{}\n```\n", single.trim_end());
        match kind {
            "mismatch" => {
                let _ = writeln!(is, "Native (`{}`):\n```\n{native_cmd}\n```\nand under Miri:\n```\n{miri_cmd}\n```\ngive different results:\n```\n{}\n```\n", native.name, detail.trim());
            }
            _ => {
                let _ = writeln!(is, "Running it under Miri:\n```\n{miri_cmd}\n```\nprints\n```\n{}\n```\n", detail.chars().take(4000).collect::<String>().trim());
            }
        }
        let _ = writeln!(is, "### Meta\n\n```\n{}\n{}\n```\n\nProblem was found by a rust fuzzer (created by an LLM) - https://github.com/qarmin/rs-compiler-fuzzer", self.versions.0, self.versions.1);
        std::fs::write(dir.join("ISSUE.md"), is).unwrap();

                let mut st = String::new();
        let _ = writeln!(st, "## Status (Miri {kind})\n\n- Found by running generated programs under Miri; the case is not reduced and was not checked against a second tool.\n- Decide first whether the generator or Miri is at fault: see `report.md`.\n- Search for the message in rust-lang/miri and rust-lang/rust before reporting.\n");
        std::fs::write(dir.join("STATUS.md"), st).unwrap();
        let cfg = format!("strategy={strategy}\nbase_name=native\nbase_flags=\nbad_name=miri-{kind}\nbad_flags=\nbatch_seed={batch_seed}\nmode=miri\ntoolchain=nightly\n");
        std::fs::write(dir.join("repro.cfg"), cfg).unwrap();
        let status = format!("kind=miri-{kind}\nstate=open\nprogress=0/0\n");
        std::fs::write(dir.join("status.cfg"), status).unwrap();
        info!("MIRI {kind}: {sig}  -> {}", dir.display());
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
            info!("miri attempt {attempt}: {} cases, {} lines, {:.0}s, {}", cases.len(), a.source.lines().count(), t0.elapsed().as_secs_f32(), if r.ok { "ok" } else if r.timed_out { "TIMEOUT" } else { "failed" });
            if r.timed_out {
                // some case is (much) slower under Miri than the rest: find the slow part by splitting into chunks
                if depth == 0 && cases.len() >= 8 {
                    info!("Miri timed out; retrying in 4 chunks (batch seed {batch_seed})");
                    let chunk = cases.len().div_ceil(4);
                    for part in cases.chunks(chunk) {
                        self.batch_at(strategy, part.to_vec(), batch_seed, 1);
                    }
                } else {
                    info!("Miri timed out on {} cases; skipping them (batch seed {batch_seed})", cases.len());
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
                        // an untouched hash means the case printed nothing under Miri: feature-gated code (CPU features are not exposed by Miri)
                        if m.get(&c.id).is_some_and(|v| v == "cbf29ce484222325") && n.get(&c.id) != m.get(&c.id) {
                            continue;
                        }
                        if m.get(&c.id) != n.get(&c.id) {
                            self.mismatch += 1;
                            let d = format!("miri: {:?}\nnative {}: {:?}", m.get(&c.id), native_cfg.name, n.get(&c.id));
                            self.save("mismatch", strategy, c, &format!("{strategy} case differs between Miri and native (case seed {})", c.seed), &d, batch_seed);
                        }
                    }
                }
                return;
            }
            let blamed = blamed_cases(&r.stderr, &a, None);
            let sig = signature(&r.stderr);
            let compile_error = !r.stderr.contains("Undefined Behavior") && !r.stderr.contains("unsupported operation") && (r.stderr.contains("error[E") || r.stderr.contains("could not compile") || r.stderr.lines().any(|l| l.starts_with("error:")) && !r.stderr.contains("the evaluated program"));
            if blamed.is_empty() {
                warn!("Miri failed, not attributable to a case: {sig}\n{}", r.stderr.chars().take(1500).collect::<String>());
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
    crate::strategies::set_miri_mode(true);
    let work = o.out.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(o.out.join("miri")).unwrap();
    let have = {
        let mut c = Command::new("cargo");
        c.args(["+nightly", "miri", "--version"]);
        exec(c, Duration::from_secs(120)).map(|r| r.code == Some(0)).unwrap_or(false)
    };
    if !have {
        warn!("Miri is not installed: rustup component add miri rust-src --toolchain nightly");
        return 1;
    }
    info!("miri mode: configs for the native comparison: {}", o.configs[0].name);
    let mut st = State { o, miri: Miri::new(&work), out: o.out.clone(), n: 0, seen: BTreeSet::new(), ub: 0, mismatch: 0, unsupported: 0, generator: 0, versions: tool_versions() };
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
            info!("[{}] seed {batch_seed}, ~{lines} lines", s.name);
            let cases = generate_batch(s, batch_seed, lines);
            st.batch(s.name, cases, batch_seed);
            longest = longest.max(t0.elapsed());
        }
        round += 1;
    }
    info!("miri done: {} rounds in {:.0}s; UB reports {}, Miri-vs-native mismatches {}, unsupported {}, generator compile errors {}", round, start.elapsed().as_secs_f32(), st.ub, st.mismatch, st.unsupported, st.generator);
    let _ = std::fs::remove_dir_all(&work);
    // unsupported operations are expected for a few constructs; UB and mismatches are the interesting ones
    if st.ub + st.mismatch > 0 {
        2
    } else {
        0
    }
}
