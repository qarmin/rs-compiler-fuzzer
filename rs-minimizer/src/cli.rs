use handsome_logger::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode, WriteLogger};
use crate::control::{install_ctrlc, set_checkpoint, set_parallel, start_heartbeat};
use crate::rustc_oracle::{Mode, Spec, detect};
use crate::{Cached, Options, Oracle, ReduceLog, minimize, script_predicate};
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "\
reduce - make a Rust program as small as possible while it keeps misbehaving

USAGE:
  rs-compiler-fuzzer reduce FILE.rs [options]            the file decides: crash, error, hang, miscompilation... (see AUTO)
  rs-compiler-fuzzer reduce FILE.rs --test CMD [options]  your own check: CMD gets the candidate path as its last argument and exits 0
                                                         while the problem shows

WHAT TO KEEP (default: AUTO - the file is compiled and run, and the first thing that stands out is what is kept):
  1. the compiler takes longer than --timeout      -> --hang
  2. the compiler crashes (ICE, LLVM error, signal) -> --ice       the same crash (same message and compiler source file)
  3. the compilation fails                          -> --error     the same error code / message
  4. the program behaves differently at -C opt-level 0 and 1/2/3/s/z (stdout or exit code) -> --vs
  5. the program panics or fails when run          -> --stderr TEXT
  Say it yourself instead of waiting for AUTO:
  --ice | --error [TEXT] | --hang [SECS] | --oom GB | --vs \"FLAGS\" | --stdout TEXT | --stderr TEXT

COMPILER:
  --rustc CMD        compiler command (default `rustc`), e.g. \"rustc +nightly\"; an empty string switches the passes that need rustc off
  --flags \"FLAGS\"   flags of the build, e.g. \"-C opt-level=3 -Z mir-opt-level=4\" (`--edition 2021` is added unless you give one)
  --timeout SECS     longest compilation (default 120); --run-timeout SECS longest run (default 20)

OPTIONS:
  -o, --output FILE  where the result goes (default: FILE.min.rs); it is also the checkpoint, rewritten on every improvement
  --budget SECS      time limit (default 600)
  --jobs N           parallel runs (default: number of cores)
  --no-shrinkray     do not use Shrink Ray (`pip install shrinkray`; used when installed)
  --treereduce       also use treereduce-rust (`cargo install treereduce-rust`; off by default, it rarely finds anything more)
  --log FILE         also write the log to FILE

Ctrl+C once stops and keeps the best program found so far, twice leaves at once.

EXAMPLES:
  rs-compiler-fuzzer reduce ice.rs --rustc \"rustc +nightly\"
  rs-compiler-fuzzer reduce wrong.rs --rustc \"rustc +nightly\" --vs \"-C opt-level=0\" --flags \"-C opt-level=3\"
  rs-compiler-fuzzer reduce big.rs --rustc \"rustc +nightly\" --oom 8
";

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

/// The command line of the standalone reducer (`args` without the program name); returns the exit code.
pub fn run(args: Vec<String>) -> i32 {
    // the check an external reducer (Shrink Ray, treereduce) calls: `reduce --check SPEC FILE`
    if args.first().map(String::as_str) == Some("--check") {
        let spec = args.get(1).and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| Spec::from_text(&t));
        let src = args.get(2).and_then(|f| std::fs::read_to_string(f).ok());
        return match (spec, src) {
            (Some(spec), Some(src)) if spec.check(&src) => 0,
            _ => 1,
        };
    }
    let (mut input, mut test, mut output, mut log_file) = (None::<PathBuf>, None::<String>, None::<PathBuf>, None::<PathBuf>);
    let mut budget = 600u64;
    let mut jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut rustc = "rustc".to_string();
    let mut flags = String::new();
    let (mut compile_secs, mut run_secs) = (120u64, 20u64);
    let (mut shrinkray, mut treereduce) = (true, false);
    // explicit modes: (name, optional argument)
    let mut wanted: Option<(String, Option<String>)> = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| die(&format!("{a} needs a value")));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return 0;
            }
            "--test" => test = Some(val()),
            "-o" | "--output" => output = Some(PathBuf::from(val())),
            "--budget" => budget = val().parse().unwrap_or_else(|_| die("--budget needs a number")),
            "--jobs" => jobs = val().parse().unwrap_or_else(|_| die("--jobs needs a number")),
            "--timeout" => compile_secs = val().parse().unwrap_or_else(|_| die("--timeout needs a number of seconds")),
            "--run-timeout" => run_secs = val().parse().unwrap_or_else(|_| die("--run-timeout needs a number of seconds")),
            "--rustc" => rustc = val(),
            "--flags" => flags = val(),
            "--log" => log_file = Some(PathBuf::from(val())),
            "--no-shrinkray" => shrinkray = false,
            "--no-treereduce" => treereduce = false,
            "--treereduce" => treereduce = true,
            "--ice" => wanted = Some(("ice".into(), None)),
            "--auto" => wanted = None,
            "--error" | "--hang" => {
                let optional = it.peek().filter(|n| !n.starts_with('-') && !n.ends_with(".rs")).map(|n| n.to_string());
                if optional.is_some() {
                    it.next();
                }
                wanted = Some((a.trim_start_matches('-').to_string(), optional));
            }
            "--oom" | "--vs" | "--stdout" | "--stderr" => wanted = Some((a.trim_start_matches('-').to_string(), Some(val()))),
            other if !other.starts_with('-') && input.is_none() => input = Some(PathBuf::from(other)),
            other => die(&format!("unknown option `{other}` (see --help)")),
        }
    }
    let input = input.unwrap_or_else(|| die("FILE.rs is required (see --help)"));
    let output = output.unwrap_or_else(|| input.with_extension("min.rs"));

    let mut loggers: Vec<Box<dyn handsome_logger::SharedLogger>> = vec![TermLogger::new(Config::default(), TerminalMode::Mixed, ColorChoice::Auto)];
    if let Some(f) = &log_file {
        match std::fs::OpenOptions::new().create(true).append(true).open(f) {
            Ok(file) => loggers.push(WriteLogger::new(Config::default(), file)),
            Err(e) => eprintln!("cannot write {}: {e}", f.display()),
        }
    }
    let _ = CombinedLogger::init(loggers);

    let src = std::fs::read_to_string(&input).unwrap_or_else(|e| die(&format!("{}: {e}", input.display())));
    let work = std::env::temp_dir().join(format!("rs-minimizer-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&work);
    let words = |t: &str| t.split_whitespace().map(String::from).collect::<Vec<_>>();
    let mut uses_rustc_passes = !rustc.trim().is_empty();

    let oracle: Box<dyn Oracle> = if let Some(test) = test {
        let command = words(&test);
        if command.is_empty() {
            die("--test is empty");
        }
        let mut oracle = Cached::new(script_predicate(command.clone(), work.clone()));
        oracle.work = work.clone();
        oracle.external = Some(command.iter().map(|c| std::path::absolute(c).ok().filter(|p| p.exists()).map(|p| p.display().to_string()).unwrap_or_else(|| c.clone())).collect());
        Box::new(oracle)
    } else {
        if rustc.trim().is_empty() {
            die("without --test the compiler is needed: give --rustc \"rustc\" (or \"rustc +nightly\")");
        }
        let mut spec = Spec { rustc: words(&rustc), flags: words(&flags), mode: Mode::Hang { secs: compile_secs }, compile_timeout: Duration::from_secs(compile_secs), run_timeout: Duration::from_secs(run_secs) };
        let detected = detect(&spec, &src);
        let (mode, why) = match (&wanted, detected) {
            (None, Some(d)) => d,
            (None, None) => die("nothing to reduce: the file compiles, runs and behaves the same at every optimisation level. Say what to keep: --vs \"FLAGS\", --stdout TEXT, --stderr TEXT, --error TEXT, --oom GB, --hang"),
            (Some((name, arg)), detected) => {
                let from_detection = |kind: fn(&Mode) -> bool| detected.clone().filter(|(m, _)| kind(m));
                match (name.as_str(), arg) {
                    ("ice", _) => from_detection(|m| matches!(m, Mode::Crash { .. })).unwrap_or_else(|| die("--ice: the compiler does not crash on this file with these flags")),
                    ("error", Some(text)) => (Mode::Error { needle: text.clone() }, format!("the compilation fails with `{text}`")),
                    ("error", None) => from_detection(|m| matches!(m, Mode::Error { .. })).unwrap_or_else(|| die("--error: the compilation of this file does not fail")),
                    ("hang", secs) => {
                        let secs = secs.as_ref().and_then(|s| s.parse().ok()).unwrap_or(compile_secs);
                        spec.compile_timeout = Duration::from_secs(secs);
                        (Mode::Hang { secs }, format!("the compilation does not finish in {secs}s"))
                    }
                    ("oom", Some(gb)) => {
                        let mb = (gb.parse::<f64>().unwrap_or_else(|_| die("--oom needs a number of GB")) * 1024.0) as u64;
                        (Mode::Oom { mb }, format!("the compiler uses more than {gb} GB"))
                    }
                    ("vs", Some(other)) => (Mode::Differ { other_flags: words(other) }, format!("the program behaves differently with `{other}` and with `{flags}`")),
                    ("stdout", Some(text)) => (Mode::Output { stdout: true, needle: text.clone() }, format!("the program prints `{text}`")),
                    ("stderr", Some(text)) => (Mode::Output { stdout: false, needle: text.clone() }, format!("the program's stderr contains `{text}`")),
                    _ => die("this option needs a value (see --help)"),
                }
            }
        };
        log::info!("keeping: {why}");
        // the passes that compile for their own diagnostics (dead code, renames, inlining) need a program that compiles
        uses_rustc_passes = matches!(mode, Mode::Differ { .. } | Mode::Output { .. });
        spec.mode = mode;
        let spec_file = work.join("spec.cfg");
        let _ = std::fs::write(&spec_file, spec.to_text());
        Box::new(crate::rustc_oracle::oracle(spec, work.clone(), spec_file))
    };

    if !oracle.check(&src) {
        die("the file does not show the problem (the check fails on it): nothing to reduce");
    }
    set_parallel(jobs);
    install_ctrlc();
    start_heartbeat(Duration::from_secs(30));
    set_checkpoint(Some(output.clone()));
    let mut opts = Options::new(Duration::from_secs(budget));
    opts.rustc = if uses_rustc_passes { words(&rustc) } else { Vec::new() };
    opts.use_shrinkray = shrinkray;
    opts.use_treereduce = treereduce;
    let mut log = ReduceLog::default();
    let result = minimize(&*oracle, &src, &opts, &mut log, &mut |_, _, _| None);
    let _ = std::fs::write(&output, &result);
    let _ = std::fs::remove_dir_all(&work);
    log::info!("done: {} -> {} lines, written to {}", src.lines().count(), result.lines().count(), output.display());
    0
}
