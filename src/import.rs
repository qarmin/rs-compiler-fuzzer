//! `import`: findings that were found by hand (or by another tool) enter the normal pipeline.
//!
//! The input is a directory (the unpacked `todo.zip` of the `Nightly` release) with pairs `NAME.rs` + `NAME.txt` at any depth:
//! `NAME.rs` is the program, `NAME.txt` holds the rustc command line that shows the problem, e.g.
//!
//! ```text
//! rustc +nightly reduced.rs -C opt-level=3 -C lto=fat -C codegen-units=1; ./reduced
//! ```
//!
//! Everything after `;` / `&&` and the source / `-o` arguments are ignored; `+channel` selects the toolchain (default nightly).
//! A second line starting with `ref:` (or `good:`) gives the reference command. Without one the reference is derived from the
//! failing command by taking out what is about optimisation (opt-level, LTO, codegen-units, llvm-args, target-cpu, `-Z`...)
//! and trying `-C opt-level=0`, then the plain defaults, until one of them behaves differently. A compiler crash needs no reference
//! besides one that compiles and runs.
//!
//! What is made of a pair is an ordinary finding directory (`manual__<flags>__<hash>`), which is then triaged in place like any other
//! (reproduction on every channel, minimisation, flags, opt levels, Miri, bisection, upstream search, issue draft).

use crate::runner::{build_and_run, crash_signature, differs, Config, Limits, Outcome};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Cmd {
    pub channel: Option<String>,
    pub flags: Vec<String>,
}

/// Words of a command line with simple quoting.
fn split_words(line: &str) -> Vec<String> {
    let (mut words, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(ch);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// The flags of a `rustc ...` command line (`None` when the line has no rustc in it).
pub fn parse_command(line: &str) -> Option<Cmd> {
    // cut at the first shell separator outside quotes: `; ./reduced`, `&& ./x`, `| head`, ` # comment`
    let mut cut = line.len();
    let mut quote = None::<char>;
    let b: Vec<(usize, char)> = line.char_indices().collect();
    for (k, &(i, c)) in b.iter().enumerate() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, ';') | (None, '|') | (None, '>') => {
                cut = i;
                break;
            }
            (None, '&') if b.get(k + 1).map(|x| x.1) == Some('&') => {
                cut = i;
                break;
            }
            (None, '#') if k == 0 || b[k - 1].1.is_whitespace() => {
                cut = i;
                break;
            }
            _ => {}
        }
    }
    let words = split_words(&line[..cut]);
    let start = words.iter().position(|w| w == "rustc" || w.ends_with("/rustc"))?;
    let mut channel = None;
    let mut flags: Vec<String> = Vec::new();
    let mut it = words[start + 1..].iter().peekable();
    while let Some(w) = it.next() {
        if let Some(c) = w.strip_prefix('+') {
            channel = Some(c.to_string());
        } else if w.ends_with(".rs") && !w.starts_with('-') {
            // the source file
        } else if matches!(w.as_str(), "-o" | "--out-dir" | "--emit" | "--crate-name") {
            it.next();
        } else if w.starts_with("--emit=") || w.starts_with("--out-dir=") || w.starts_with("--crate-name=") || (w.starts_with("-o") && w.len() > 2 && !w.starts_with("-O")) {
            // output arguments are ours
        } else if w == "-O" {
            flags.push("-C".into());
            flags.push("opt-level=2".into());
        } else if w == "-g" {
            flags.push("-C".into());
            flags.push("debuginfo=2".into());
        } else {
            flags.push(w.clone());
        }
    }
    Some(Cmd { channel, flags })
}

/// `-C opt-level=3` / `-Copt-level=3` / `-Zfoo=1` / `-Z foo=1` as a list of (prefix letter, "key=value") pairs, other flags as ("", flag).
fn pairs(flags: &[String]) -> Vec<(char, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let f = &flags[i];
        if (f == "-C" || f == "-Z") && i + 1 < flags.len() {
            out.push((f.chars().nth(1).unwrap(), flags[i + 1].clone()));
            i += 2;
            continue;
        }
        if f.len() > 2 && (f.starts_with("-C") || f.starts_with("-Z")) {
            out.push((f.chars().nth(1).unwrap(), f[2..].to_string()));
        } else {
            out.push((' ', f.clone()));
        }
        i += 1;
    }
    out
}

fn unpairs(p: &[(char, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (c, v) in p {
        if *c == ' ' {
            out.push(v.clone());
        } else {
            out.push(format!("-{c}"));
            out.push(v.clone());
        }
    }
    out
}

/// Flags that change how the code is optimised or generated; the reference build does without them.
fn about_codegen(c: char, kv: &str) -> bool {
    if c == 'Z' {
        return true;
    }
    let key = kv.split('=').next().unwrap_or("");
    c == 'C' && matches!(key, "opt-level" | "lto" | "codegen-units" | "llvm-args" | "target-cpu" | "target-feature" | "passes" | "no-prepopulate-passes" | "inline-threshold" | "embed-bitcode" | "linker-plugin-lto" | "profile-generate" | "profile-use" | "relocation-model" | "code-model" | "prefer-dynamic" | "panic" | "force-frame-pointers" | "soft-float")
}

/// Reference candidates for a failing command, most similar first.
fn reference_candidates(bad: &[String]) -> Vec<Vec<String>> {
    let kept: Vec<(char, String)> = pairs(bad).into_iter().filter(|(c, kv)| !about_codegen(*c, kv)).collect();
    let mut o0 = kept.clone();
    o0.push(('C', "opt-level=0".into()));
    let mut cands = vec![unpairs(&o0), unpairs(&kept)];
    cands.dedup();
    cands
}

/// A short name for the failing configuration: `O3-lto-fat-cgu1`.
fn name_of(flags: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (c, kv) in pairs(flags) {
        let (k, v) = kv.split_once('=').unwrap_or((kv.as_str(), ""));
        let part = match (c, k) {
            ('C', "opt-level") => format!("O{v}"),
            ('C', "lto") => if v.is_empty() || v == "yes" || v == "y" || v == "on" { "lto".to_string() } else { format!("lto-{v}") },
            ('C', "codegen-units") => format!("cgu{v}"),
            ('C', "target-cpu") => v.to_string(),
            ('C', "overflow-checks") if v == "on" || v == "yes" => "checks".to_string(),
            ('Z', "mir-opt-level") => format!("mir{v}"),
            ('Z', k) if !k.is_empty() => format!("z{}", k.chars().take(12).collect::<String>()),
            _ => continue,
        };
        parts.push(part);
    }
    let s: String = parts.join("-").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(48).collect();
    if s.is_empty() { "default".into() } else { s }
}

fn is_crash(o: &Outcome) -> bool {
    matches!(o, Outcome::CompileFail(e) if e.contains("LLVM ERROR") || e.contains("internal compiler error") || e.contains("interrupted by SIG") || e.contains("panicked at") || e.contains("Assertion") || e.contains("rustc unexpectedly") || e.contains("SIGSEGV") || e.contains("SIGILL"))
}

pub enum Outcome2 {
    /// the finding directory was written (key, directory)
    Ready(String, PathBuf),
    /// nothing to triage: why
    NotReproduced(String),
}

/// Pairs `X.rs` + `X.txt` below `root`, as stems relative to it (sorted).
pub fn list_pairs(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) && p.with_extension("txt").is_file() {
                let rel = p.with_extension("");
                out.push(rel.strip_prefix(root).unwrap_or(&rel).to_string_lossy().to_string());
            }
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v.sort();
    v
}

/// Reproduce one pair and write its finding directory below `out` (`<out>/<key>`).
pub fn prepare(root: &Path, stem: &str, out: &Path, work: &Path) -> Result<Outcome2, String> {
    let rs = root.join(format!("{stem}.rs"));
    let txt = root.join(format!("{stem}.txt"));
    let src = std::fs::read_to_string(&rs).map_err(|e| format!("{}: {e}", rs.display()))?;
    let text = std::fs::read_to_string(&txt).map_err(|e| format!("{}: {e}", txt.display()))?;
    let (mut bad_cmd, mut ref_cmd) = (None, None);
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some(r) = l.strip_prefix("ref:").or_else(|| l.strip_prefix("good:")) {
            ref_cmd = parse_command(r.trim());
        } else if bad_cmd.is_none() {
            bad_cmd = parse_command(l);
        }
    }
    let bad_cmd = bad_cmd.ok_or_else(|| format!("{}: no `rustc ...` command line found", txt.display()))?;
    if bad_cmd.flags.iter().any(|f| f.starts_with("--crate-type") || f == "--crate-type") {
        let t = bad_cmd.flags.join(" ");
        if !t.contains("crate-type=bin") && !t.contains("crate-type bin") {
            return Ok(Outcome2::NotReproduced("only executables are supported (a --crate-type other than bin has nothing to run)".into()));
        }
    }
    let channel = bad_cmd.channel.clone().unwrap_or_else(|| "nightly".into());
    let rustc = vec!["rustc".to_string(), format!("+{channel}")];
    let lim = Limits { compile: Duration::from_secs(600), run: Duration::from_secs(30) };
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let path = work.join(format!("import_{}.rs", std::process::id()));
    std::fs::write(&path, &src).map_err(|e| e.to_string())?;
    crate::runner::set_common_flags(None);
    let bad = Config::new(&name_of(&bad_cmd.flags), &bad_cmd.flags.join(" "));
    let out_bad = build_and_run(&rustc, &path, &work.join(format!("import_bad_{}", std::process::id())), &bad, &lim);
    let candidates: Vec<Vec<String>> = match &ref_cmd {
        Some(r) => vec![r.flags.clone()],
        None => reference_candidates(&bad_cmd.flags),
    };
    let mut seen = Vec::new();
    let mut found: Option<(Config, Outcome)> = None;
    for flags in candidates {
        let base = Config::new(if flags.is_empty() { "default" } else { "ref" }, &flags.join(" "));
        let out_base = build_and_run(&rustc, &path, &work.join(format!("import_base_{}", std::process::id())), &base, &lim);
        let yes = match (&out_base, &out_bad) {
            (Outcome::Ran { .. }, o) if is_crash(o) => true,
            (Outcome::Ran { .. }, Outcome::Ran { .. }) => differs(&out_base, &out_bad, &base, &bad),
            _ => false,
        };
        seen.push(format!("`{}`: {}", base.flags.join(" "), out_base.describe()));
        if yes {
            found = Some((base, out_base));
            break;
        }
    }
    let _ = std::fs::remove_file(&path);
    let Some((base, _)) = found else {
        return Ok(Outcome2::NotReproduced(format!("the failing command: {}; reference candidates: {}", out_bad.describe(), seen.join("; "))));
    };
    let dir = work.join(format!("import_dir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("case.rs"), &src).map_err(|e| e.to_string())?;
    let meta = format!("strategy=manual\nbase_name={}\nbase_flags={}\nbad_name={}\nbad_flags={}\ntoolchain={channel}\nimported_from={stem}\n", base.name, base.flags.join(" "), bad.name, bad.flags.join(" "));
    std::fs::write(dir.join("repro.cfg"), meta).map_err(|e| e.to_string())?;
    if is_crash(&out_bad) {
        if let Some(sig) = crash_signature(&out_bad) {
            std::fs::write(dir.join("signature.txt"), sig).map_err(|e| e.to_string())?;
        }
    }
    let _ = std::fs::write(dir.join("report.md"), format!("# imported by hand: `{}` vs `{}`\n\nSource: `{stem}.rs`, command: see `{stem}.txt`:\n\n```\n{}\n```\n", base.name, bad.name, text.trim()));
    let key = crate::pack::key_of(&dir).ok_or("no key")?;
    let dest = out.join(&key);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    std::fs::rename(&dir, &dest).or_else(|_| {
        std::fs::create_dir_all(&dest)?;
        for f in ["case.rs", "repro.cfg", "report.md", "signature.txt"] {
            if dir.join(f).exists() {
                std::fs::copy(dir.join(f), dest.join(f))?;
            }
        }
        Ok::<(), std::io::Error>(())
    }).map_err(|e| e.to_string())?;
    Ok(Outcome2::Ready(key, dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_taken_apart() {
        let c = parse_command("rustc +nightly reduced.rs -C opt-level=3 -Clto=fat -Zmir-opt-level=0 --edition 2024 -o out; ./reduced").unwrap();
        assert_eq!(c.channel.as_deref(), Some("nightly"));
        assert_eq!(c.flags, ["-C", "opt-level=3", "-Clto=fat", "-Zmir-opt-level=0", "--edition", "2024"]);
        let c = parse_command("rustc x.rs -O && ./x").unwrap();
        assert_eq!(c.flags, ["-C", "opt-level=2"]);
        assert!(parse_command("cargo build").is_none());
    }

    #[test]
    fn the_reference_drops_what_is_about_code_generation() {
        let flags: Vec<String> = ["--edition", "2024", "-C", "opt-level=3", "-Clto=fat", "-Zmir-opt-level=0", "-C", "overflow-checks=on"].iter().map(|s| s.to_string()).collect();
        let c = reference_candidates(&flags);
        assert_eq!(c[0], ["--edition", "2024", "-C", "overflow-checks=on", "-C", "opt-level=0"]);
        assert_eq!(c[1], ["--edition", "2024", "-C", "overflow-checks=on"]);
        assert_eq!(name_of(&flags), "O3-lto-fat-mir0-checks");
    }
}
