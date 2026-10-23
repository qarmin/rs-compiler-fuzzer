//! Reduction of fuzzer findings: the oracle (two configurations still disagree, or the compiler still crashes the same way) and
//! the steps that depend on how batches are generated (a single `mod case_N` left becomes a plain `fn main`). Dropping cases is left
//! to the generic passes: items together with the code that mentions them did better than a case-aware ddmin. The generic passes live in `rs-minimizer`.

use crate::runner::{build_and_run, Config, Limits, Outcome};
use log::info;
use rs_minimizer::{Oracle, Options, Stage};
use rs_minimizer::scan::matching;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use rs_minimizer::control::{best_so_far, install_ctrlc as install_sigint, last_run_complete, set_checkpoint, set_parallel, start_heartbeat, INTERRUPTED};
pub use rs_minimizer::external::shrinkray_available;
pub use rs_minimizer::ReduceLog;

static N: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub struct Interest {
    pub rustc: Vec<String>,
    pub a: Config,
    pub b: Config,
    /// If set: `b` must fail to compile with exactly this (normalised) crash signature.
    pub crash_sig: Option<String>,
    pub limits: Limits,
    pub work: PathBuf,
    /// results by hash of the candidate text: the passes regenerate many identical candidates
    pub cache: Arc<Mutex<HashMap<u64, bool>>>,
}

fn hash_text(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Oracle for Interest {
    fn check(&self, src: &str) -> bool {
        Interest::check(self, src)
    }

    fn recheck(&self, src: &str) -> bool {
        self.check_uncached(src)
    }

    fn explain(&self, src: &str) -> String {
        let dir = self.work.join(format!("x{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("main.rs");
        let _ = std::fs::write(&path, src);
        let describe = |c: &Config| {
            let out = build_and_run(&self.rustc, &path, &dir.join(format!("bin_{}", c.name)), c, &self.limits);
            let extra = match &out {
                Outcome::CompileFail(e) => format!(" ({})", e.lines().find(|l| l.contains("error")).unwrap_or("").chars().take(160).collect::<String>()),
                Outcome::Ran { stdout, .. } => format!(" (stdout hash {:x})", hash_text(stdout)),
                _ => String::new(),
            };
            format!("{} [{}]: {}{extra}", c.name, c.flags.join(" "), out.describe())
        };
        let text = format!("{} | {} | compiler `{}`", describe(&self.a), describe(&self.b), self.rustc.join(" "));
        let _ = std::fs::remove_dir_all(&dir);
        text
    }

    fn work_dir(&self) -> PathBuf {
        self.work.clone()
    }

    fn external_command(&self, dir: &std::path::Path) -> Option<Vec<String>> {
        let cfg = dir.join("interest.cfg");
        std::fs::write(&cfg, self.to_cfg()).ok()?;
        let me = std::env::current_exe().ok()?;
        Some(vec![me.display().to_string(), "interesting".into(), "--cfg".into(), cfg.display().to_string(), "--file".into()])
    }
}

impl Interest {
    pub fn check(&self, src: &str) -> bool {
        let key = hash_text(src);
        if let Some(&r) = self.cache.lock().unwrap().get(&key) {
            return r;
        }
        rs_minimizer::control::oracle_started();
        let r = self.check_uncached(src);
        rs_minimizer::control::oracle_finished(src, r);
        self.cache.lock().unwrap().insert(key, r);
        r
    }

    fn check_uncached(&self, src: &str) -> bool {
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = self.work.join(format!("r{}_{}", std::process::id(), id));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("main.rs");
        std::fs::write(&path, src).unwrap();
        let run = |c: &Config| build_and_run(&self.rustc, &path, &dir.join(format!("bin_{}", c.name)), c, &self.limits);
        // the reference build first: a candidate that does not even compile and run there is not interesting, and the (usually much
        // slower) second build is skipped. Most candidates of a reduction are such.
        let first = run(&self.a);
        let interesting = if !matches!(first, Outcome::Ran { .. }) {
            false
        } else {
            let second = run(&self.b);
            match &self.crash_sig {
                Some(sig) => crate::runner::crash_signature(&second).as_deref() == Some(sig.as_str()),
                None => matches!(second, Outcome::Ran { .. }) && first.key() != second.key(),
            }
        };
        let _ = std::fs::remove_dir_all(&dir);
        interesting
    }

    /// Serialise for the `interesting` subcommand used as treereduce's oracle.
    pub fn to_cfg(&self) -> String {
        format!(
            "rustc={}\na_name={}\na_flags={}\nb_name={}\nb_flags={}\nsig={}\ncompile={}\nrun={}\nwork={}\n",
            self.rustc.join(" "),
            self.a.name,
            self.a.flags.join(" "),
            self.b.name,
            self.b.flags.join(" "),
            self.crash_sig.clone().unwrap_or_default(),
            self.limits.compile.as_secs(),
            self.limits.run.as_secs(),
            self.work.display()
        )
    }

    pub fn from_cfg(text: &str) -> Interest {
        let m: std::collections::HashMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
        let g = |k: &str| m.get(k).copied().unwrap_or("").to_string();
        let sig = g("sig");
        Interest {
            rustc: g("rustc").split_whitespace().map(String::from).collect(),
            a: Config::new(&g("a_name"), &g("a_flags")),
            b: Config::new(&g("b_name"), &g("b_flags")),
            crash_sig: if sig.is_empty() { None } else { Some(sig) },
            limits: Limits { compile: Duration::from_secs(g("compile").parse().unwrap_or(120)), run: Duration::from_secs(g("run").parse().unwrap_or(10)) },
            work: PathBuf::from(g("work")),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}




/// Turn the generator's harness into a plain program: the single remaining `mod case_N` becomes the
/// crate root, `fn run(h: &mut H)` becomes `fn main()`, `h.put(&x)` becomes `println!("{:?}", x)`,
/// `bb` is `std::hint::black_box`. Returns None when the file does not have the expected shape.
pub fn flatten(src: &str) -> Option<String> {
    let b = src.as_bytes();
    let m = src.find("mod case_")?;
    let open = m + src[m..].find('{')?;
    let close = matching(b, open)?;
    let mut body = src[open + 1..close].to_string();
    body = body.replace("use super::rt::*;", "").replace("use super::*;", "");
    let run = body.find("pub fn run(h: &mut H)")?;
    body = format!("{}fn main(){}", &body[..run], &body[run + "pub fn run(h: &mut H)".len()..]);
    body = body.replace("h.put(&", "println!(\"{:?}\", ");
    body = body.replace("pub fn ", "fn ");
    // dedent by one level
    let text: Vec<String> = body.lines().map(|l| l.strip_prefix("    ").unwrap_or(l).to_string()).filter(|l| !l.trim().is_empty()).collect();
    Some(format!("use std::hint::black_box as bb;\n{}\n", text.join("\n")))
}

/// Remove redundant parentheses around plain identifiers / literals (all at once; kept only if the
/// result is still interesting).
/// The reducers turn `println!("{:?}", x)` into `print!("{}", x)`; an issue reads better with one line per value, so put the
/// newlines back (kept only if the program is still interesting).
pub fn prefer_println(it: &dyn Oracle, src: &str) -> String {
    if !src.contains("print!(") {
        return src.to_string();
    }
    let cand = src.replace("print!(", "println!(");
    if it.check(&cand) { cand } else { src.to_string() }
}

/// `use std::hint::black_box as a;` + `let a = a(1);` reads badly (the reducers shorten names): call it `black_box`
/// again when the program stays interesting.
pub fn unalias_black_box(it: &dyn Oracle, src: &str) -> String {
    let Some(p) = src.find("use std::hint::black_box as ") else { return src.to_string() };
    let rest = &src[p + "use std::hint::black_box as ".len()..];
    let alias: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if alias.is_empty() || alias == "black_box" || !rest[alias.len()..].starts_with(';') {
        return src.to_string();
    }
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    out.push_str(&src[..p]);
    out.push_str("use std::hint::black_box;");
    let mut i = p + "use std::hint::black_box as ".len() + alias.len() + 1;
    while i < src.len() {
        if src[i..].starts_with(&alias) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.' || b[i - 1] == b':')) && src.as_bytes().get(i + alias.len()) == Some(&b'(') {
            out.push_str("black_box");
            i += alias.len();
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    if it.check(&out) { out } else { src.to_string() }
}

/// Full pipeline: the generic passes of `rs-minimizer` with the batch-specific steps plugged in. `budget` bounds the whole thing.
pub fn reduce_all(it: &Interest, src: &str, budget: Duration, log: &mut ReduceLog) -> String {
    let mut opts = Options::new(budget);
    opts.rustc = it.rustc.clone();
    opts.keep_module = Box::new(|name| name.starts_with("case_"));
    let mut hook = |stage: Stage, cur: &str, _deadline: Instant| -> Option<String> {
        match stage {
            // a single case left: drop the harness, a plain `fn main` is what a human would post
            Stage::AfterLines | Stage::AfterExternal => match flatten(cur) {
                Some(flat) if flat != cur && it.check(&flat) => {
                    info!("flattened the harness into a plain `fn main` ({} -> {} lines)", cur.lines().count(), flat.lines().count());
                    Some(flat)
                }
                Some(_) if cur.contains("mod case_") => {
                    info!("flattening the harness changes the behaviour (or several cases are left) - kept the harness");
                    None
                }
                _ => None,
            },
            Stage::Polish => Some(prefer_println(it, &unalias_black_box(it, cur))),
            Stage::Unrolled | Stage::BeforeLines => None,
        }
    };
    rs_minimizer::minimize(it, src, &opts, log, &mut hook)
}
