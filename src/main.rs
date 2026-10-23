//! rs-compiler-fuzzer: differential testing of rustc for miscompilations.
//!
//! Generates large batches of UB-free Rust programs, compiles each batch with many
//! rustc configurations, runs the binaries and compares their output.

mod batch;
mod fuzz;
mod rng;
mod reduce;
mod bisect;
mod demangle;
mod miri;
mod pack;
mod steps;
mod runner;
mod triage;

mod strategies;

use log::{error, info, warn};
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
  rs-compiler-fuzzer miri               run generated programs under Miri (UB reports = generator/Miri/std bugs; Miri-vs-native differences); same options as fuzz
  rs-compiler-fuzzer triage --dir DIR   reproduce on all channels, reduce, Miri-check, search GitHub, write STATUS.md/ISSUE.md/verdict
                            (--bisect SECS: find the first stable release / nightly that has the bug; --dirs ROOT: every finding below ROOT; --budget SECS reduction time per finding; --no-search; --no-reduce; --limit N)
  rs-compiler-fuzzer minimize DIR       local, long minimisation of one stored finding on all cores (--jobs N, --budget SECS per run, --deadline SECS overall,
                            --bisect SECS, --search, --no-force = skip a finding that already converged, --ui auto|tui|basic: dashboard with a size graph and the last diff in a terminal, plain log otherwise); Ctrl+C keeps the best program and rewrites ISSUE.md/STATUS.md/finding.json; run it again to continue
  rs-compiler-fuzzer retriage --pkg DIR  triage again (in place) every stored finding with an older fuzzer_id (--shard I/N, --limit N, --deadline SECS, --budget, --bisect; --finding NAME: just that one, minimised again)
  rs-compiler-fuzzer pack --pkg DIR     merge finding directories (--add DIR, repeatable) into an unpacked package, rebuild INDEX.md
                            (--toolchain NAME records where they were found; --contains KEYS_FILE checks that keys are present)
  rs-compiler-fuzzer reduce FILE.rs --test CMD    reduce any Rust file while CMD (given the candidate path, exit 0 = still interesting) succeeds; --help for the options
  rs-compiler-fuzzer interesting --cfg FILE --file F   the check as a command line (exit 0 = still interesting), for scripts
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
  --profile fast|full   fast = skip the slowest configs (fat/thin LTO, one codegen unit); default full
  --configs a,b,c       use only these named configurations (first = baseline)
  --config NAME=FLAGS   add a custom configuration (repeatable)
  --compile-timeout S   per-compilation timeout (default 900)
  --run-timeout S       per-execution timeout (default 60)
  --minimize-secs N     wall-clock budget for shrinking one finding (default 1500)
  --max-findings N      stop after N findings (default 10)
  --minimize-budget N   max extra compilations spent shrinking one finding (default 40)
  --fail-on-known       exit 2 also for findings that match a known upstream bug
  --batch-seed N        reproduce one batch: the `seed N` printed in the logs (with --strategies X --target-lines L --rounds 1)
  --timeouts-are-findings  report compile timeouts as findings (default: log only)
gen only:
  --strategy NAME       strategy to generate (required)
  --case-seed N         generate the single case with this seed
";

/// Terminal logger; the long-running local commands also write to `tmp/<command>.log` (appended, no colours).
fn init_logging(cmd: &str, ui: bool) {
    use handsome_logger::{ColorChoice, CombinedLogger, Config, ConfigBuilder, LevelFilter, TermLogger, TerminalMode, WriteLogger};
    // with the dashboard on the screen the log goes to the file only
    let mut loggers: Vec<Box<dyn handsome_logger::SharedLogger>> = if ui { Vec::new() } else { vec![TermLogger::new(Config::default(), TerminalMode::Mixed, ColorChoice::Auto)] };
    // the long-running commands also write a much more detailed log to a file (debug level: every improvement, every pass start), which
    // the CI jobs keep as an artifact
    if matches!(cmd, "minimize" | "triage" | "retriage") {
        let path = PathBuf::from("tmp").join(format!("{cmd}.log"));
        let file = std::fs::create_dir_all("tmp").and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(&path));
        match file {
            Ok(f) => loggers.push(WriteLogger::new(ConfigBuilder::default().set_level(LevelFilter::Debug).build(), f)),
            Err(e) => eprintln!("cannot write the log file {}: {e}", path.display()),
        }
    }
    if let Err(e) = CombinedLogger::init(loggers) {
        eprintln!("cannot start the logger: {e}");
    }
}

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
    if cmd == "reduce" {
        // the standalone reducer of any Rust file (its own logger, its own options)
        std::process::exit(rs_minimizer::cli::run(args[1..].to_vec()));
    }
    let ui_arg = args.windows(2).find(|w| w[0] == "--ui").map(|w| w[1].clone()).unwrap_or_else(|| "auto".to_string());
    let ui = cmd == "minimize" && rs_minimizer::ui::wanted(&ui_arg);
    init_logging(&cmd, ui);
    let mut seed: Option<u64> = None;
    let mut duration = None;
    let mut rounds = None;
    let mut target_lines: Option<usize> = None;
    let mut dir_arg: Option<PathBuf> = None;
    let mut scale = 100usize;
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
    let mut profile = "full".to_string();
    let mut budget = 900u64;
    let mut no_search = false;
    let mut no_reduce = false;
    let mut limit = 8usize;
    let mut bisect_secs = 0u64;
    let mut shard = String::from("0/1");
    let mut deadline_secs = 0u64;
    let mut dirs_root: Option<PathBuf> = None;
    let mut cfg_file: Option<PathBuf> = None;
    let mut file_arg: Option<PathBuf> = None;
    let mut adds: Vec<PathBuf> = Vec::new();
    let mut pkg: Option<PathBuf> = None;
    let mut toolchain_name = String::from("unknown");
    let mut contains: Option<PathBuf> = None;
    let mut skip_file: Option<PathBuf> = None;
    let mut finding: Option<String> = None;
    let mut budget_max = 0u64;
    let mut minimize_secs = 1500u64;
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
            "--profile" => profile = val(),
            "--budget" => budget = num(val()),
            "--no-search" => no_search = true,
            "--search" | "--no-force" if cmd == "minimize" => {}
            "--no-reduce" => no_reduce = true,
            "--bisect" => bisect_secs = num(val()),
            "--shard" => shard = val(),
            "--deadline" => deadline_secs = num(val()),
            "--limit" => limit = num(val()) as usize,
            "--dirs" => dirs_root = Some(PathBuf::from(val())),
            "--cfg" => cfg_file = Some(PathBuf::from(val())),
            "--file" => file_arg = Some(PathBuf::from(val())),
            "--add" => adds.push(PathBuf::from(val())),
            "--pkg" => pkg = Some(PathBuf::from(val())),
            "--toolchain" => toolchain_name = val(),
            "--contains" => contains = Some(PathBuf::from(val())),
            "--skip" => skip_file = Some(PathBuf::from(val())),
            "--finding" => finding = Some(val()),
            "--ui" => {
                let _ = val();
            }
            "--budget-max" => budget_max = num(val()),
            "--minimize-secs" => minimize_secs = num(val()),
            "--strategy" => strategy = Some(val()),
            "--case-seed" => case_seed = Some(num(val())),
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            other if cmd == "minimize" && !other.starts_with('-') && dir_arg.is_none() => dir_arg = Some(PathBuf::from(other)),
            other => die(&format!("unknown option `{other}`")),
        }
    }

    let rustc_argv: Vec<String> = rustc.split_whitespace().map(String::from).collect();
    if rustc_argv.is_empty() {
        die("--rustc is empty");
    }
    let seed = seed.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1));

    // reducers run the oracle from other working directories
    let out = std::path::absolute(&out).unwrap_or(out);
    match cmd.as_str() {
        "list" => {
            println!("strategies:");
            for s in strategies::ALL {
                println!("  {:<10} {}", s.name, s.about);
            }
            println!("stdlib tables: {}", strategies::stdlib_table_names().join(" "));
            println!("default configurations (nightly adds -Z ones):");
            for c in fuzz::default_configs(true) {
                println!("  {:<16} {}{}", c.name, c.flags.join(" "), if c.slow { "   [slow: not in --profile fast]" } else { "" });
            }
        }
        "interesting" => {
            let cfg = cfg_file.unwrap_or_else(|| die("interesting needs --cfg"));
            let file = file_arg.unwrap_or_else(|| die("interesting needs --file"));
            let it = reduce::Interest::from_cfg(&std::fs::read_to_string(&cfg).unwrap_or_else(|e| die(&format!("{}: {e}", cfg.display()))));
            let src = std::fs::read_to_string(&file).unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
            std::process::exit(if it.check(&src) { 0 } else { 1 });
        }
        "triage" => {
            let mut targets: Vec<PathBuf> = Vec::new();
            if let Some(d) = &dir_arg {
                targets.push(d.clone());
            }
            if let Some(root) = &dirs_root {
                let mut v: Vec<PathBuf> = std::fs::read_dir(root).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("case.rs").exists() && p.join("repro.cfg").exists()).collect()).unwrap_or_default();
                v.sort();
                targets.extend(v);
            }
            if targets.is_empty() {
                die("triage needs --dir DIR or --dirs ROOT");
            }
            let o = triage::TriageOpts { budget: Duration::from_secs(budget), search: !no_search, work: out.join("work"), no_reduce, bisect: Duration::from_secs(bisect_secs), checkpoint: false, force_reduce: false };
            let mut worst = 0;
            for (i, d) in targets.iter().take(limit).enumerate() {
                info!("[{}/{}] triage {}", i + 1, targets.len().min(limit), d.display());
                match triage::triage(&o, d) {
                    Ok(s) => info!("{s}"),
                    Err(e) => {
                        warn!("triage failed: {e}");
                        worst = 1;
                    }
                }
            }
            let _ = std::fs::remove_dir_all(out.join("work"));
            std::process::exit(worst);
        }
        "minimize" => {
            // local, interactive minimisation of one stored finding: all cores, hours of time, Ctrl+C keeps what was reached
            let dir = dir_arg.unwrap_or_else(|| die("minimize needs the finding directory, e.g. `minimize findings/arith__Oz__4a1a1f9e41`"));
            if !dir.join("case.rs").exists() || !dir.join("repro.cfg").exists() {
                die(&format!("{} is not a finding directory (case.rs / repro.cfg missing)", dir.display()));
            }
            let flag_given = |f: &str| args.iter().any(|a| a == f);
            let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            let par = if flag_given("--jobs") { jobs } else { cores };
            reduce::set_parallel(par);
            reduce::install_sigint();
            let per_run = if flag_given("--budget") { budget } else { 4 * 3600 };
            let total = if deadline_secs > 0 { deadline_secs } else { 12 * 3600 };
            let o = triage::TriageOpts { budget: Duration::from_secs(per_run), search: !no_search && flag_given("--search"), work: out.join("work"), no_reduce, bisect: Duration::from_secs(bisect_secs), checkpoint: true, force_reduce: !flag_given("--no-force") };
            let start = std::time::Instant::now();
            let status_of = |d: &PathBuf| std::fs::read_to_string(d.join("status.cfg")).unwrap_or_default().lines().find_map(|l| l.strip_prefix("min_status=").map(String::from)).unwrap_or_default();
            let lines_now = |d: &PathBuf| std::fs::read_to_string(d.join("reduced.rs")).map(|t| t.lines().count()).unwrap_or(0);
            info!("minimize {}: {par} parallel jobs on {cores} cores, up to {per_run}s per pass over everything, {total}s overall", dir.display());
            info!("Ctrl+C once = stop and write everything up, twice = leave at once (reduced.rs is saved on every improvement)");
            reduce::start_heartbeat(Duration::from_secs(30));
            if ui {
                let case = std::fs::read_to_string(dir.join("case.rs")).unwrap_or_default();
                rs_minimizer::ui::start(&dir.display().to_string(), Duration::from_secs(total), (case.lines().count(), case.len()), &format!("{}/reduced.rs", dir.display()), "tmp/minimize.log");
            }
            let mut runs = 0;
            loop {
                runs += 1;
                let left = total.saturating_sub(start.elapsed().as_secs());
                let mut o2 = o.clone_opts();
                o2.budget = Duration::from_secs(per_run.min(left));
                info!("--- minimisation run {runs} (reduced.rs now {} lines) ---", lines_now(&dir));
                match triage::triage(&o2, &dir) {
                    Ok(s) => info!("{s}"),
                    Err(e) => {
                        error!("triage failed: {e}");
                        break;
                    }
                }
                let st = status_of(&dir);
                info!("minimisation status: {st}; reduced.rs {} lines", lines_now(&dir));
                if reduce::INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
                    warn!("interrupted - STATUS.md, ISSUE.md, status.cfg and finding.json are written for what was reached");
                    break;
                }
                if st.starts_with("converged") || start.elapsed().as_secs() >= total || runs >= 8 {
                    break;
                }
            }
            if let Some(root) = dir.canonicalize().ok().and_then(|d| d.parent().map(|p| p.to_path_buf())).filter(|p| p.file_name().map(|n| n == "findings").unwrap_or(false)) {
                if let Some(pkg) = root.parent() {
                    pack::write_index(pkg);
                }
            }
            let _ = std::fs::remove_dir_all(out.join("work"));
            info!("done: {}/reduced.rs, ISSUE.md, STATUS.md", dir.display());
            rs_minimizer::control::write_rejected_log(std::path::Path::new("tmp/minimize.rejected.log"));
            if ui {
                let case = std::fs::read_to_string(dir.join("case.rs")).unwrap_or_default();
                let red = std::fs::read_to_string(dir.join("reduced.rs")).unwrap_or_default();
                let size = if red.is_empty() || red.len() >= case.len() {
                    format!("{}: no reduced program was written ({} lines / {} bytes at the start)", dir.display(), case.lines().count(), case.len())
                } else {
                    format!("{}: {} lines / {} bytes -> {} lines / {} bytes ({:.1}% fewer bytes), status: {}", dir.display(), case.lines().count(), case.len(), red.lines().count(), red.len(), 100.0 * (case.len() as f64 - red.len() as f64) / case.len().max(1) as f64, status_of(&dir))
                };
                rs_minimizer::ui::stop(&[size, format!("written: {0}/reduced.rs, {0}/ISSUE.md, {0}/STATUS.md; log: tmp/minimize.log, rejected candidates: tmp/minimize.rejected.log", dir.display())]);
            }
            std::process::exit(0);
        }
        "retriage" => {
            // triage again every stored finding that was last triaged by an older `fuzzer_id`, in place
            let pkg = pkg.unwrap_or_else(|| die("retriage needs --pkg DIR (an unpacked findings.zip)"));
            let (si, sn) = shard.split_once('/').map(|(a, b)| (a.parse::<usize>().unwrap_or(0), b.parse::<usize>().unwrap_or(1).max(1))).unwrap_or((0, 1));
            let skip: std::collections::HashSet<String> = skip_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
            let mut stale: Vec<PathBuf> = triage::stale_findings(&pkg, &skip).into_iter().filter(|p| pack::name_hash(&p.file_name().unwrap().to_string_lossy()) % sn as u64 == si as u64).collect();
            if let Some(name) = &finding {
                // one finding, whatever its checklist says, minimised again from where it stands
                let dir = pkg.join("findings").join(name);
                if !dir.join("case.rs").exists() {
                    die(&format!("--finding {name}: {} has no case.rs", dir.display()));
                }
                stale = vec![dir];
            }
            info!("retriage: {} stored finding(s) with open checklist steps in shard {shard}", stale.len());
            let mut o = triage::TriageOpts { budget: Duration::from_secs(budget), search: !no_search, work: out.join("work"), no_reduce, bisect: Duration::from_secs(bisect_secs), checkpoint: false, force_reduce: finding.is_some() };
            let start = std::time::Instant::now();
            let max_budget = budget_max.max(budget);
            let mut done = 0;
            for d in stale.iter().take(limit) {
                // Minimisation gets the time the other waiting findings do not need: the smaller the queue, the longer a finding
                // may be minimised (between --budget and --budget-max); what a finding does not use stays for the next one.
                let remaining = if deadline_secs > 0 { deadline_secs.saturating_sub(start.elapsed().as_secs()) } else { 0 };
                // a bigger finding, or one whose minimisation is not settled yet, gets a bigger share of what is left
                let weight = |p: &PathBuf| -> f64 {
                    let lines = std::fs::read_to_string(p.join("reduced.rs")).or_else(|_| std::fs::read_to_string(p.join("case.rs"))).map(|t| t.lines().count()).unwrap_or(500) as f64;
                    let settled = std::fs::read_to_string(p.join("status.cfg")).unwrap_or_default().lines().any(|l| l.starts_with("min_status=converged"));
                    1.0 + lines / 400.0 + if settled { 0.0 } else { 1.0 }
                };
                let (mut other, mut mins, mut weights) = (0u64, 0u64, 0.0f64);
                for p in &stale {
                    let ids = triage::open_ids_of(p);
                    if ids.iter().any(|i| *i == "minimized" || *i == "minimize_cases") {
                        mins += 1;
                        weights += weight(p);
                    }
                    other += 300 + if ids.iter().any(|i| i.starts_with("bisect") || *i == "pass_bisect") { bisect_secs + 600 } else { 0 };
                }
                let this_min = triage::open_ids_of(d).iter().any(|i| *i == "minimized" || *i == "minimize_cases");
                let b = if deadline_secs > 0 && this_min {
                    let share = remaining.saturating_sub(other) as f64 * weight(d) / weights.max(1.0);
                    (share as u64).clamp(budget, max_budget)
                } else {
                    budget
                };
                let _ = mins;
                o.budget = Duration::from_secs(b);
                let bis_needed = triage::open_ids_of(d).iter().any(|i| i.starts_with("bisect") || *i == "pass_bisect");
                if deadline_secs > 0 && start.elapsed() + Duration::from_secs(budget.min(b) + if bis_needed { bisect_secs + 600 } else { 0 } + 240) > Duration::from_secs(deadline_secs) {
                    info!("retriage: not enough time left for another finding");
                    break;
                }
                info!("retriage: minimisation budget for this finding: {b}s");
                info!("retriage {}", d.display());
                match triage::triage(&o, d) {
                    Ok(s) => {
                        info!("{s}");
                        done += 1;
                        println!("RETRIAGED {}", d.file_name().unwrap().to_string_lossy());
                    }
                    Err(e) => warn!("failed: {e}"),
                }
            }
            let _ = std::fs::remove_dir_all(out.join("work"));
            info!("retriage: {done} finding(s) updated");
            std::process::exit(0);
        }
        "pack" => {
            let pkg = pkg.unwrap_or_else(|| die("pack needs --pkg DIR"));
            let code = pack::run(&pkg, &adds, &toolchain_name, contains.as_deref());
            std::process::exit(code);
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
        "fuzz" | "canary" | "verify" | "miri" => {
            let version = fuzz::rustc_version(&rustc_argv);
            let mut configs = fuzz::default_configs(fuzz::is_nightly(&version));
            configs.extend(extra);
            match profile.as_str() {
                "fast" => configs.retain(|c| !c.slow),
                "full" => {}
                other => die(&format!("unknown profile `{other}` (fast|full)")),
            }
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
            info!("seed: {seed}");
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
                minimize_secs,
            };
            let code = match cmd.as_str() {
                "fuzz" => fuzz::run(&opts),
                "canary" => fuzz::canary(&opts),
                "miri" => miri::run(&opts),
                _ => {
                    let dir = dir_arg.unwrap_or_else(|| die("verify needs --dir"));
                    let report = opts.out.clone();
                    fuzz::verify(&opts, &dir, &report, if toolchain_name == "unknown" { "" } else { &toolchain_name })
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
