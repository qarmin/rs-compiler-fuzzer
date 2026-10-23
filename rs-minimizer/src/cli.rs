use handsome_logger::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode, WriteLogger};
use crate::control::{install_ctrlc, set_checkpoint, set_parallel, start_heartbeat};
use crate::{Cached, Options, ReduceLog, minimize, script_predicate};
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "\
reduce - reduce a Rust program while a test command keeps succeeding

USAGE:
  rs-compiler-fuzzer reduce INPUT.rs --test CMD [options]

The test command gets the path of a candidate file as its last argument and exits with 0 when the candidate still shows the
problem. Example: rs-compiler-fuzzer reduce ice.rs --test ./check.sh -o small.rs

OPTIONS:
  --test CMD         test command (split at spaces); required
  -o, --output FILE  where the result goes (default: INPUT.min.rs); it is also the checkpoint, rewritten on every improvement
  --budget SECS      time limit (default 600)
  --jobs N           parallel test runs (default: number of cores)
  --rustc CMD        compiler used to find unused items (default `rustc`, empty string switches the pass off)
  --no-shrinkray     do not use Shrink Ray
  --no-treereduce    do not use treereduce-rust
  --log FILE         also write the log to FILE
";

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

/// The command line of the standalone reducer (`args` without the program name); returns the exit code.
pub fn run(args: Vec<String>) -> i32 {
    let (mut input, mut test, mut output, mut log_file) = (None::<PathBuf>, None::<String>, None::<PathBuf>, None::<PathBuf>);
    let mut budget = 600u64;
    let mut jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut rustc = "rustc".to_string();
    let (mut shrinkray, mut treereduce) = (true, true);
    let mut it = args.iter();
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
            "--rustc" => rustc = val(),
            "--log" => log_file = Some(PathBuf::from(val())),
            "--no-shrinkray" => shrinkray = false,
            "--no-treereduce" => treereduce = false,
            other if !other.starts_with('-') && input.is_none() => input = Some(PathBuf::from(other)),
            other => die(&format!("unknown option `{other}`")),
        }
    }
    let input = input.unwrap_or_else(|| die("INPUT.rs is required (see --help)"));
    let test = test.unwrap_or_else(|| die("--test CMD is required (see --help)"));
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
    let command: Vec<String> = test.split_whitespace().map(String::from).collect();
    if command.is_empty() {
        die("--test is empty");
    }
    let work = std::env::temp_dir().join(format!("rs-minimizer-{}", std::process::id()));
    let mut oracle = Cached::new(script_predicate(command.clone(), work.clone()));
    oracle.work = work.clone();
    oracle.external = Some(command.iter().map(|c| std::path::absolute(c).ok().filter(|p| p.exists()).map(|p| p.display().to_string()).unwrap_or_else(|| c.clone())).collect());

    use crate::Oracle;
    if !oracle.check(&src) {
        die("the test command does not succeed on the input: nothing to reduce");
    }
    set_parallel(jobs);
    install_ctrlc();
    start_heartbeat(Duration::from_secs(30));
    set_checkpoint(Some(output.clone()));
    let mut opts = Options::new(Duration::from_secs(budget));
    opts.rustc = rustc.split_whitespace().map(String::from).collect();
    opts.use_shrinkray = shrinkray;
    opts.use_treereduce = treereduce;
    let mut log = ReduceLog::default();
    let result = minimize(&oracle, &src, &opts, &mut log, &mut |_, _, _| None);
    let _ = std::fs::write(&output, &result);
    let _ = std::fs::remove_dir_all(&work);
    log::info!("done: {} -> {} lines, written to {}", src.lines().count(), result.lines().count(), output.display());
    0
}
