//! Test-case reduction. A candidate is "interesting" when the two configurations of a finding still
//! disagree in the same way (output differs, or `bad` still fails to compile with the same crash
//! signature). Passes, from coarse to fine:
//!   1. drop chunks of lines (ddmin),
//!   2. replace `let x: T = <expr>;` right-hand sides by a literal,
//!   3. delete `.method(args)` segments of call chains (for the generated code, most wrapping/
//!      saturating/... methods return their receiver's type, so deleting the call still type-checks),
//!   4. `treereduce-rust` (syntax-tree delta debugging; external tool, optional),
//! followed by passes 2-3 again. Everything is bounded by a deadline.

use crate::runner::{build_and_run, par_map, Config, Limits, Outcome};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
}

impl Interest {
    pub fn check(&self, src: &str) -> bool {
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = self.work.join(format!("r{}_{}", std::process::id(), id));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("main.rs");
        std::fs::write(&path, src).unwrap();
        let cfgs = [&self.a, &self.b];
        let outs = par_map(&cfgs, 2, |c| build_and_run(&self.rustc, &path, &dir.join(format!("bin_{}", c.name)), c, &self.limits));
        let _ = std::fs::remove_dir_all(&dir);
        match &self.crash_sig {
            Some(sig) => matches!(&outs[0], Outcome::Ran { .. }) && crate::runner::crash_signature(&outs[1]).as_deref() == Some(sig.as_str()),
            None => match (&outs[0], &outs[1]) {
                (Outcome::Ran { .. }, Outcome::Ran { .. }) => outs[0].key() != outs[1].key(),
                _ => false,
            },
        }
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
        }
    }
}

fn expired(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

/// Pass 1: remove chunks of lines, halving the chunk size.
pub fn shrink_lines(it: &Interest, src: &str, deadline: Instant) -> String {
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    let mut n = (lines.len() / 2).max(1);
    while n >= 1 {
        let mut i = 0;
        while i < lines.len() {
            if expired(deadline) {
                return lines.join("\n");
            }
            let end = (i + n).min(lines.len());
            let cand: Vec<String> = lines[..i].iter().chain(lines[end..].iter()).cloned().collect();
            if !cand.is_empty() && it.check(&cand.join("\n")) {
                lines = cand;
            } else {
                i += n;
            }
        }
        if n == 1 {
            break;
        }
        n /= 2;
    }
    lines.join("\n")
}

/// Pass 2: `let name: ty = EXPR;`  ->  `let name: ty = 0;` (or `false`)
pub fn simplify_lets(it: &Interest, src: &str, deadline: Instant) -> String {
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    for i in 0..lines.len() {
        if expired(deadline) {
            break;
        }
        let l = lines[i].trim_start();
        let Some(rest) = l.strip_prefix("let ") else { continue };
        let Some((head, expr)) = rest.split_once(" = ") else { continue };
        let Some((_, ty)) = head.split_once(": ") else { continue };
        if !expr.trim_end().ends_with(';') || expr.trim() == "0;" {
            continue;
        }
        let lit = match ty.trim() {
            "bool" => "false",
            t if t.starts_with('f') => "0.0",
            t if t.starts_with(|c: char| c == 'i' || c == 'u') => "0",
            _ => continue,
        };
        let indent = &lines[i][..lines[i].len() - lines[i].trim_start().len()];
        let mut cand = lines.clone();
        cand[i] = format!("{indent}let {head} = {lit};");
        if it.check(&cand.join("\n")) {
            lines = cand;
        }
    }
    lines.join("\n")
}

/// Byte ranges of `.ident(<balanced args>)` segments (turbofish allowed).
fn call_segments(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' && i + 1 < b.len() && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_') {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                let mut depth = 0i32;
                let mut k = j;
                while k < b.len() {
                    match b[k] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        b'"' => {
                            // skip string literals
                            k += 1;
                            while k < b.len() && b[k] != b'"' {
                                if b[k] == b'\\' {
                                    k += 1;
                                }
                                k += 1;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if depth == 0 && k < b.len() {
                    out.push((i, k + 1));
                }
            }
        }
        i += 1;
    }
    out
}

/// Pass 3: delete one `.method(..)` segment at a time, from the end of the file backwards. Two
/// candidates are evaluated concurrently.
pub fn drop_calls(it: &Interest, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    loop {
        let mut segs = call_segments(&cur);
        let mut progress = false;
        while !segs.is_empty() {
            if expired(deadline) {
                return cur;
            }
            let lo = segs.len().saturating_sub(2);
            let batch: Vec<(usize, usize)> = segs[lo..].iter().rev().copied().collect();
            let cands: Vec<String> = batch.iter().map(|&(s, e)| format!("{}{}", &cur[..s], &cur[e..])).collect();
            let res = par_map(&cands, 2, |c| it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let (s, _) = batch[p];
                    cur = cands[p].clone();
                    progress = true;
                    // only segments lying entirely before the removed one keep valid offsets
                    segs.retain(|&(ss, ee)| ee <= s && ss < s);
                }
                None => segs.truncate(lo),
            }
        }
        if !progress {
            return cur;
        }
    }
}

/// External syntax-aware reducer. Returns the input unchanged if the tool is missing/fails.
pub fn treereduce(it: &Interest, src: &str, deadline: Instant, jobs: usize, passes: usize) -> String {
    if Command::new("treereduce-rust").arg("--version").output().is_err() {
        return src.to_string();
    }
    let dir = it.work.join(format!("tr{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&dir);
    let (inp, out, cfg) = (dir.join("in.rs"), dir.join("out.rs"), dir.join("interest.cfg"));
    std::fs::write(&inp, src).unwrap();
    std::fs::write(&cfg, it.to_cfg()).unwrap();
    let me = std::env::current_exe().unwrap();
    let remaining = deadline.saturating_duration_since(Instant::now()).as_secs().max(30);
    let status = Command::new("timeout")
        .arg(remaining.to_string())
        .arg("treereduce-rust")
        .args(["-s", &inp.display().to_string(), "-o", &out.display().to_string(), "-j", &jobs.to_string(), "--passes", &passes.to_string(), "--min-reduction", "1", "-q"])
        .args(["--timeout", "400", "--"])
        .arg(&me)
        .args(["interesting", "--cfg", &cfg.display().to_string(), "--file", "@@"])
        .status();
    let res = match status {
        Ok(_) => std::fs::read_to_string(&out).unwrap_or_else(|_| src.to_string()),
        Err(_) => src.to_string(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    // a reducer result that is not interesting any more (timeouts, flakiness) is discarded
    if it.check(&res) { res } else { src.to_string() }
}

pub struct ReduceLog(pub Vec<String>);

/// Full pipeline. `budget` bounds the whole thing.
pub fn reduce_all(it: &Interest, src: &str, budget: Duration, log: &mut ReduceLog) -> String {
    let start = Instant::now();
    let deadline = start + budget;
    let mut cur = src.to_string();
    let note = |log: &mut ReduceLog, what: &str, cur: &str| {
        log.0.push(format!("{what}: {} lines / {} bytes ({:.0}s)", cur.lines().count(), cur.len(), start.elapsed().as_secs_f32()));
    };
    note(log, "start", &cur);
    cur = shrink_lines(it, &cur, start + budget / 4);
    note(log, "after line shrinking", &cur);
    cur = simplify_lets(it, &cur, start + budget / 3);
    note(log, "after let simplification", &cur);
    cur = drop_calls(it, &cur, start + budget / 2);
    note(log, "after dropping method calls", &cur);
    cur = treereduce(it, &cur, start + budget * 3 / 4, 4, 3);
    note(log, "after treereduce-rust", &cur);
    cur = drop_calls(it, &cur, deadline);
    cur = shrink_lines(it, &cur, deadline);
    note(log, "final", &cur);
    cur
}

