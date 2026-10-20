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
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
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

impl Interest {
    pub fn check(&self, src: &str) -> bool {
        let key = hash_text(src);
        if let Some(&r) = self.cache.lock().unwrap().get(&key) {
            return r;
        }
        let r = self.check_uncached(src);
        self.cache.lock().unwrap().insert(key, r);
        r
    }

    fn check_uncached(&self, src: &str) -> bool {
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
            cache: Arc::new(Mutex::new(HashMap::new())),
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


// ---------------------------------------------------------------------------------------------
// Expression-level passes (what a human does when shrinking a generated chain by hand)

/// Matching close bracket for the open bracket at `open`, skipping string literals.
fn matching(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut k = open;
    while k < b.len() {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            b'"' => {
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
    None
}

/// Start of the postfix expression (`a.b(c)[d].e`) that ends right before `end`.
fn postfix_start(b: &[u8], mut end: usize) -> usize {
    loop {
        if end == 0 {
            return 0;
        }
        let c = b[end - 1];
        if c == b')' || c == b']' {
            // jump to the matching open bracket
            let (open, close) = if c == b')' { (b'(', b')') } else { (b'[', b']') };
            let mut depth = 0i32;
            let mut k = end - 1;
            loop {
                if b[k] == close {
                    depth += 1;
                } else if b[k] == open {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if k == 0 {
                    return 0;
                }
                k -= 1;
            }
            end = k;
            // the callee / macro name in front of the bracket
            while end > 0 && (b[end - 1].is_ascii_alphanumeric() || b[end - 1] == b'_' || b[end - 1] == b':' || b[end - 1] == b'!') {
                end -= 1;
            }
            // `.name(...)` continues the chain
            if end > 0 && b[end - 1] == b'.' {
                end -= 1;
                continue;
            }
            if end > 0 && (b[end - 1] == b')' || b[end - 1] == b']') {
                continue;
            }
            return end;
        } else if c.is_ascii_alphanumeric() || c == b'_' || c == b':' {
            while end > 0 && (b[end - 1].is_ascii_alphanumeric() || b[end - 1] == b'_' || b[end - 1] == b':') {
                end -= 1;
            }
            if end > 0 && b[end - 1] == b'.' {
                end -= 1;
                continue;
            }
            return end;
        } else {
            return end;
        }
    }
}

/// Byte ranges of the top-level, comma separated arguments inside the brackets at `open..=close`.
fn top_args(src: &str, open: usize, close: usize) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut start = open + 1;
    let mut depth = 0i32;
    let mut k = open + 1;
    while k < close {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'"' => {
                k += 1;
                while k < close && b[k] != b'"' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            b',' if depth == 0 => {
                out.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    if src[start..close].trim().len() > 0 {
        out.push((start, close));
    }
    out
}

/// All single-edit simplifications of `src` as `(position, new text)`, in textual order.
/// Edits `(start, end, replacement)` of `src` (applied lazily: materialising every candidate text of a big
/// file at once needs gigabytes).
fn expr_candidates(src: &str) -> Vec<(usize, usize, String)> {
    let b = src.as_bytes();
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    let keywords = ["if", "while", "match", "for", "fn", "return", "in", "else", "let", "mut", "loop", "unsafe", "as"];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // string literals are opaque
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        // `.method(args)`: replace the whole call (receiver included) by one of its arguments
        if c == b'.' && i + 1 < b.len() && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_') {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                if let Some(close) = matching(b, j) {
                    let rs = postfix_start(b, i);
                    for (s, e) in top_args(src, j, close) {
                        let arg = src[s..e].trim();
                        out.push((rs, close + 1, format!("({arg})")));
                    }
                }
            }
            i += 1;
            continue;
        }
        // plain call `name(args)` / `Some(x)` / `bb(x)`: replace by one argument
        if (c.is_ascii_alphabetic() || c == b'_') && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.' || b[i - 1] == b':')) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
                j += 1;
            }
            let word = &src[i..j];
            if j < b.len() && b[j] == b'(' && !keywords.contains(&word) && !word.ends_with(':') {
                if let Some(close) = matching(b, j) {
                    for (s, e) in top_args(src, j, close) {
                        let arg = src[s..e].trim();
                        out.push((i, close + 1, format!("({arg})")));
                    }
                }
            }
            // `if COND { A } else { B }`  ->  `{ A }` / `{ B }`
            if word == "if" && (i < 5 || &src[i - 5..i] != "else ") {
                // the block opens at the first `{` outside brackets
                let mut depth = 0i32;
                let mut k = j;
                let mut open = None;
                while k < b.len() {
                    match b[k] {
                        b'(' | b'[' => depth += 1,
                        b')' | b']' => depth -= 1,
                        b'{' if depth == 0 => {
                            open = Some(k);
                            break;
                        }
                        b';' => break,
                        _ => {}
                    }
                    k += 1;
                }
                if let Some(o1) = open {
                    if let Some(c1) = matching(b, o1) {
                        let rest = &src[c1 + 1..];
                        let tail = rest.trim_start();
                        if tail.starts_with("else") {
                            let o2 = c1 + 1 + (rest.len() - tail.len()) + 4;
                            let o2 = o2 + src[o2..].find('{').unwrap_or(0);
                            if src[c1 + 1..o2].trim() == "else" {
                                if let Some(c2) = matching(b, o2) {
                                    out.push((i, c2 + 1, src[o1..=c1].to_string()));
                                    out.push((i, c2 + 1, src[o2..=c2].to_string()));
                                }
                            }
                        }
                    }
                }
            }
            // ` as T` casts
            if word == "as" && i > 0 && b[i - 1] == b' ' {
                let mut k = j;
                while k < b.len() && b[k] == b' ' {
                    k += 1;
                }
                let mut e = k;
                while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                    e += 1;
                }
                if e > k {
                    out.push((i - 1, e, String::new()));
                }
            }
            i = j.max(i + 1);
            continue;
        }
        // unary minus in front of a literal
        if c == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_digit() && (i == 0 || matches!(b[i - 1], b'(' | b' ' | b',' | b'=')) {
            out.push((i, i + 1, String::new()));
        }
        // integer literals: try 0 and 1 (suffix kept)
        if c.is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.')) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'_') {
                j += 1;
            }
            let digits = &src[i..j];
            let is_float = j < b.len() && b[j] == b'.' && j + 1 < b.len() && b[j + 1].is_ascii_digit();
            let hex = digits == "0" && j < b.len() && (b[j] == b'x' || b[j] == b'b' || b[j] == b'o');
            if !is_float && !hex {
                let mut k = j;
                while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_') {
                    k += 1;
                }
                let suffix = &src[j..k];
                if suffix.is_empty() || suffix.starts_with('u') || suffix.starts_with('i') {
                    for rep in ["0", "1"] {
                        if digits != rep {
                            out.push((i, k, format!("{rep}{suffix}")));
                        }
                    }
                }
                i = k.max(i + 1);
                continue;
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

/// Apply `expr_candidates` greedily, last position first, evaluating several candidates at once.
pub fn simplify_exprs(it: &Interest, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    if cur.len() > 60_000 {
        return cur; // far too many candidates; the case-level and line-level passes come first
    }
    let apply = |s: &str, e: &(usize, usize, String)| format!("{}{}{}", &s[..e.0], e.2, &s[e.1..]);
    loop {
        let mut cands = expr_candidates(&cur);
        let mut progress = false;
        while !cands.is_empty() {
            if expired(deadline) {
                return cur;
            }
            let lo = cands.len().saturating_sub(4);
            let batch: Vec<(usize, usize, String)> = cands[lo..].iter().rev().cloned().collect();
            let texts: Vec<String> = batch.iter().map(|e| apply(&cur, e)).collect();
            let res = par_map(&texts, 4, |c| c.len() < cur.len() + 8 && it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let pos = batch[p].0;
                    cur = texts[p].clone();
                    progress = true;
                    // earlier candidates keep their meaning (they only touch text before `pos`)
                    cands = expr_candidates(&cur).into_iter().filter(|c| c.0 < pos).collect();
                }
                None => cands.truncate(lo),
            }
        }
        if !progress {
            return cur;
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
    body = body.replace("use super::rt::*;", "");
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
pub fn strip_parens(it: &Interest, src: &str) -> String {
    let b = src.as_bytes();
    let mut drop: Vec<usize> = Vec::new();
    for i in 0..b.len() {
        if b[i] != b'(' {
            continue;
        }
        let Some(close) = matching(b, i) else { continue };
        let inner = &src[i + 1..close];
        let simple = !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'!' || b[i - 1] == b'>');
        let next_ok = close + 1 >= b.len() || !matches!(b[close + 1], b'.' | b'[' | b'(');
        if simple && prev_ok && (next_ok || !inner.starts_with(|c: char| c.is_ascii_digit())) && !(close + 1 < b.len() && b[close + 1] == b'(') {
            drop.push(i);
            drop.push(close);
        }
    }
    drop.sort();
    let mut out = String::with_capacity(src.len());
    for (i, ch) in src.char_indices() {
        if drop.binary_search(&i).is_err() {
            out.push(ch);
        }
    }
    if out != src && it.check(&out) { out } else { src.to_string() }
}

/// `rustfmt` the result if it stays interesting (the issue text is easier to read).
pub fn format_if_interesting(it: &Interest, src: &str) -> String {
    let dir = it.work.join(format!("fmt{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("x.rs");
    std::fs::write(&f, src).unwrap();
    let ok = Command::new("rustfmt").args(["--edition", "2021", "--config", "max_width=140,use_small_heuristics=Max"]).arg(&f).status().map(|s| s.success()).unwrap_or(false);
    let res = if ok { std::fs::read_to_string(&f).unwrap_or_default() } else { String::new() };
    let _ = std::fs::remove_dir_all(&dir);
    if !res.is_empty() && it.check(&res) { res } else { src.to_string() }
}

/// Shrink Ray (https://github.com/DRMacIver/shrinkray, `pip install shrinkray`): a generic reducer with
/// tree-sitter based passes. By far the strongest of the tools tried here on expression chains, so it
/// gets most of the time budget. Returns the input unchanged if it is not installed.
pub fn shrinkray(it: &Interest, src: &str, deadline: Instant, log: &mut ReduceLog) -> String {
    let exe = std::env::var("SHRINKRAY").unwrap_or_else(|_| "shrinkray".to_string());
    if Command::new(&exe).arg("--version").output().is_err() {
        return src.to_string();
    }
    let dir = it.work.join(format!("sr{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&dir);
    let (case, cfg, test) = (dir.join("case.rs"), dir.join("interest.cfg"), dir.join("test.sh"));
    std::fs::write(&case, src).unwrap();
    std::fs::write(&cfg, it.to_cfg()).unwrap();
    let me = std::env::current_exe().unwrap();
    std::fs::write(&test, format!("#!/bin/bash\nexec \"{}\" interesting --cfg \"{}\" --file \"$1\"\n", me.display(), cfg.display())).unwrap();
    let _ = Command::new("chmod").arg("+x").arg(&test).status();
    let remaining = deadline.saturating_duration_since(Instant::now()).as_secs().max(30);
    let out = Command::new("timeout")
        .arg("-s")
        .arg("INT")
        .arg(remaining.to_string())
        .arg(&exe)
        .args(["--ui", "basic", "--input-type", "arg", "--parallelism", "4", "--volume", "quiet", "--timeout", "300", "--memory-limit", "0", "--no-exit-on-completion"])
        .arg(&test)
        .arg(&case)
        .current_dir(&dir)
        .output();
    let res = std::fs::read_to_string(&case).unwrap_or_else(|_| src.to_string());
    if res == src {
        // nothing happened: keep the tool's last words, otherwise a broken setup is invisible
        if let Ok(o) = &out {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(6).collect();
            log.0.push(format!("shrinkray made no progress (exit {:?}): {}", o.status.code(), tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    if res.trim().is_empty() || !it.check(&res) { src.to_string() } else { res }
}

/// A batch file (many `mod case_N`) -> only the case modules (and their `go!` calls) that are needed.
/// Line-wise ddmin on a 400 KB batch is hopeless; modules are the natural unit.
pub fn shrink_cases(it: &Interest, src: &str, deadline: Instant) -> String {
    let lines: Vec<&str> = src.lines().collect();
    // (first line, last line, id) of every top-level `mod case_N { ... }`
    let mut mods: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(rest) = lines[i].strip_prefix("mod case_") {
            let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "}" {
                j += 1;
            }
            if !id.is_empty() && j < lines.len() {
                mods.push((i, j, id));
                i = j;
            }
        }
        i += 1;
    }
    if mods.len() < 2 {
        return src.to_string();
    }
    let build = |keep: &[usize]| -> String {
        let ids: Vec<&str> = keep.iter().map(|&k| mods[k].2.as_str()).collect();
        let mut out: Vec<&str> = Vec::new();
        let mut skip_until = 0usize;
        for (n, l) in lines.iter().enumerate() {
            if n < skip_until {
                continue;
            }
            if let Some(k) = mods.iter().position(|m| m.0 == n) {
                if keep.contains(&k) {
                    out.extend(&lines[mods[k].0..=mods[k].1]);
                }
                skip_until = mods[k].1 + 1;
                continue;
            }
            // `    go!(ID, case_ID);`
            if let Some(rest) = l.trim_start().strip_prefix("go!(") {
                let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !ids.contains(&id.as_str()) {
                    continue;
                }
            }
            out.push(l);
        }
        out.join("\n") + "\n"
    };
    let mut keep: Vec<usize> = (0..mods.len()).collect();
    let mut n = keep.len().div_ceil(2);
    while n >= 1 {
        let mut i = 0;
        while i < keep.len() && keep.len() > 1 {
            if expired(deadline) {
                return build(&keep);
            }
            let end = (i + n).min(keep.len());
            let cand: Vec<usize> = keep[..i].iter().chain(keep[end..].iter()).copied().collect();
            if !cand.is_empty() && it.check(&build(&cand)) {
                keep = cand;
            } else {
                i += n;
            }
        }
        if n == 1 {
            break;
        }
        n = n.div_ceil(2);
    }
    build(&keep)
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
    cur = shrink_cases(it, &cur, start + budget / 4);
    note(log, "after dropping unneeded cases", &cur);
    cur = shrink_lines(it, &cur, start + budget * 2 / 5);
    note(log, "after line shrinking", &cur);
    // drop the harness: a plain `fn main` is what a human would post
    match flatten(&cur) {
        Some(flat) if it.check(&flat) => {
            cur = flat;
            note(log, "after flattening the harness into a plain `fn main`", &cur);
        }
        Some(_) => log.0.push("flattening the harness changed the behaviour - kept the harness".into()),
        None => {}
    }
    let have_sr = {
        let exe = std::env::var("SHRINKRAY").unwrap_or_else(|_| "shrinkray".to_string());
        Command::new(exe).arg("--version").output().is_ok()
    };
    if have_sr {
        cur = shrinkray(it, &cur, start + budget * 7 / 10, log);
        note(log, "after shrinkray", &cur);
        cur = drop_calls(it, &cur, start + budget * 4 / 5);
        cur = simplify_exprs(it, &cur, start + budget * 9 / 10);
        note(log, "after expression polish", &cur);
    } else {
        log.0.push("shrinkray not installed (pip install shrinkray) - using the built-in passes only".into());
        // alternate the cheap passes until nothing changes
        for round in 1..=4 {
            let before = cur.clone();
            cur = simplify_lets(it, &cur, start + budget / 2);
            cur = drop_calls(it, &cur, start + budget / 2);
            cur = simplify_exprs(it, &cur, start + budget * 3 / 5);
            cur = shrink_lines(it, &cur, start + budget * 3 / 5);
            note(log, &format!("after expression passes (round {round})"), &cur);
            if cur == before || expired(start + budget * 3 / 5) {
                break;
            }
        }
        cur = treereduce(it, &cur, start + budget * 4 / 5, 4, 3);
        note(log, "after treereduce-rust", &cur);
        let fin = deadline.min(Instant::now() + Duration::from_secs(90));
        cur = simplify_exprs(it, &cur, fin);
        cur = shrink_lines(it, &cur, fin);
        note(log, "after final passes", &cur);
    }
    cur = strip_parens(it, &cur);
    cur = format_if_interesting(it, &cur);
    note(log, "final (rustfmt)", &cur);
    cur
}
