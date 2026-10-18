//! rs-compiler-fuzzer: differential testing of rustc for miscompilations.
//!
//! Generates large batches of UB-free Rust programs, compiles each batch with many
//! rustc configurations, runs the binaries and compares their output.

mod batch;
mod fuzz;
mod rng;
mod runner;
mod strategies;

use runner::{Config, Limits};
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "\
rs-compiler-fuzzer - find rustc miscompilations by differential testing

USAGE:
  rs-compiler-fuzzer fuzz   [options]   generate, compile with all configs, compare, minimise
  rs-compiler-fuzzer gen    [options]   only print a generated batch (or one case) to stdout / --out FILE
  rs-compiler-fuzzer canary [options]   check the detector against a known miscompilation
  rs-compiler-fuzzer verify --dir DIR   re-run stored findings (case.rs + repro.cfg); writes fixed.txt/still.txt to --out
  rs-compiler-fuzzer list               list strategies and default configurations

OPTIONS:
  --seed N              base seed (default: time based, printed at start)
  --duration SECS       stop starting new batches after this many seconds
  --rounds N            number of rounds (each round = one batch per strategy)
  --target-lines N      lines per batch file for every strategy (default: per strategy, 5k-150k)
  --scale PERCENT       multiply the batch size, e.g. 50 or 300 (default 100)
  --strategies a,b|all  strategies to use (default all)
  --out DIR             output directory for findings (default fuzz-out)
  --jobs N              configurations compiled in parallel (default: cores/2)
  --rustc \"CMD\"         rustc command, e.g. \"rustc +nightly\" (default: rustc)
  --configs a,b,c       use only these named configurations (first = baseline)
  --config NAME=FLAGS   add a custom configuration (repeatable)
  --compile-timeout S   per-compilation timeout (default 900)
  --run-timeout S       per-execution timeout (default 60)
  --max-findings N      stop after N findings (default 10)
  --minimize-budget N   max extra compilations spent shrinking one finding (default 40)
  --fail-on-known       exit 2 also for findings that match a known upstream bug
  --batch-seed N        reproduce one batch: the `seed N` printed in the logs (with --strategies X --target-lines L --rounds 1)
  --timeouts-are-findings  report compile timeouts as findings (default: log only)
gen only:
  --strategy NAME       strategy to generate (required)
  --case-seed N         generate the single case with this seed
";

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        print!("{HELP}");
        return;
    };
    let mut seed: Option<u64> = None;
    let mut duration = None;
    let mut rounds = None;
    let mut target_lines: Option<usize> = None;
    let mut scale = 100usize;
    let mut dir_arg: Option<PathBuf> = None;
    let mut strategies = "all".to_string();
    let mut out = PathBuf::from("fuzz-out");
    let mut jobs = std::thread::available_parallelism().map(|n| (n.get() / 2).max(1)).unwrap_or(1);
    let mut rustc = "rustc".to_string();
    let mut only: Option<Vec<String>> = None;
    let mut extra: Vec<Config> = Vec::new();
    let mut compile_timeout = 900u64;
    let mut run_timeout = 60u64;
    let mut max_findings = 10usize;
    let mut minimize_budget = 40usize;
    let mut fail_on_known = false;
    let mut timeouts_are_findings = false;
    let mut batch_seed: Option<u64> = None;
    let mut strategy: Option<String> = None;
    let mut case_seed: Option<u64> = None;
    let mut gen_out: Option<PathBuf> = None;

    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| die(&format!("{a} needs a value")));
        let num = |s: String| -> u64 { s.parse().unwrap_or_else(|_| die(&format!("bad number `{s}`"))) };
        match a.as_str() {
            "--seed" => seed = Some(num(val())),
            "--duration" => duration = Some(Duration::from_secs(num(val()))),
            "--rounds" => rounds = Some(num(val())),
            "--target-lines" => target_lines = Some(num(val()) as usize),
            "--scale" => scale = num(val()).max(1) as usize,
            "--dir" => dir_arg = Some(PathBuf::from(val())),
            "--strategies" => strategies = val(),
            "--out" => {
                let v = val();
                out = PathBuf::from(&v);
                gen_out = Some(PathBuf::from(v));
            }
            "--jobs" => jobs = num(val()).max(1) as usize,
            "--rustc" => rustc = val(),
            "--configs" => only = Some(val().split(',').map(|s| s.trim().to_string()).collect()),
            "--config" => {
                let v = val();
                let (n, f) = v.split_once('=').unwrap_or_else(|| die("--config wants NAME=FLAGS"));
                extra.push(Config::new(n, f));
            }
            "--compile-timeout" => compile_timeout = num(val()),
            "--run-timeout" => run_timeout = num(val()),
            "--max-findings" => max_findings = num(val()) as usize,
            "--minimize-budget" => minimize_budget = num(val()) as usize,
            "--fail-on-known" => fail_on_known = true,
            "--timeouts-are-findings" => timeouts_are_findings = true,
            "--batch-seed" => batch_seed = Some(num(val())),
            "--strategy" => strategy = Some(val()),
            "--case-seed" => case_seed = Some(num(val())),
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            other => die(&format!("unknown option `{other}`")),
        }
    }

    let rustc_argv: Vec<String> = rustc.split_whitespace().map(String::from).collect();
    if rustc_argv.is_empty() {
        die("--rustc is empty");
    }
    let seed = seed.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1));

    match cmd.as_str() {
        "list" => {
            println!("strategies:");
            for s in strategies::ALL {
                println!("  {:<10} {}", s.name, s.about);
            }
            println!("default configurations (nightly adds -Z ones):");
            for c in fuzz::default_configs(true) {
                println!("  {:<16} {}", c.name, c.flags.join(" "));
            }
        }
        "gen" => {
            let name = strategy.unwrap_or_else(|| die("gen needs --strategy"));
            let s = strategies::find(&name).unwrap_or_else(|| die("unknown strategy"));
            let cases = match case_seed {
                Some(cs) => vec![batch::Case { id: 0, strategy: s.name, seed: cs, body: (s.generate)(&mut rng::Rng::new(cs)) }],
                None => fuzz::generate_batch(s, seed, target_lines.unwrap_or(s.lines) * scale / 100),
            };
            let refs: Vec<&batch::Case> = cases.iter().collect();
            let src = batch::assemble(&refs).source;
            match gen_out {
                Some(p) => std::fs::write(p, src).unwrap(),
                None => print!("{src}"),
            }
        }
        "fuzz" | "canary" | "verify" => {
            let version = fuzz::rustc_version(&rustc_argv);
            let mut configs = fuzz::default_configs(fuzz::is_nightly(&version));
            configs.extend(extra);
            if let Some(names) = only {
                let mut picked = Vec::new();
                for n in &names {
                    picked.push(configs.iter().find(|c| &c.name == n).cloned().unwrap_or_else(|| die(&format!("unknown config `{n}`"))));
                }
                configs = picked;
            }
            if configs.len() < 2 && cmd != "verify" {
                die("need at least two configurations");
            }
            let strategies = fuzz::strategies_by_name(&strategies).unwrap_or_else(|e| die(&e));
            fuzz::ensure_dir(&out);
            if cmd == "fuzz" && duration.is_none() && rounds.is_none() {
                rounds = Some(1);
            }
            eprintln!("seed: {seed}");
            let opts = fuzz::Opts {
                seed,
                duration,
                rounds,
                target_lines,
                scale,
                strategies,
                out,
                jobs,
                rustc: rustc_argv,
                configs,
                limits: Limits { compile: Duration::from_secs(compile_timeout), run: Duration::from_secs(run_timeout) },
                max_findings,
                minimize_budget,
                fail_on_known,
                timeouts_are_findings,
                batch_seed,
            };
            let code = match cmd.as_str() {
                "fuzz" => fuzz::run(&opts),
                "canary" => fuzz::canary(&opts),
                _ => {
                    let dir = dir_arg.unwrap_or_else(|| die("verify needs --dir"));
                    let report = opts.out.clone();
                    fuzz::verify(&opts, &dir, &report)
                }
            };
            std::process::exit(code);
        }
        _ => {
            print!("{HELP}");
            std::process::exit(1);
        }
    }
}
