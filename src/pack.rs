//! The single stored package (`findings.zip` in the `Nightly` release), unpacked as a directory:
//!
//! ```text
//! findings/<strategy>__<config>__<hash>/{case.rs, reduced.rs, report.md, STATUS.md, ISSUE.md, repro.cfg, status.cfg, ...}
//! INDEX.md
//! ```
//!
//! `pack` merges freshly found directories into it. Merging is idempotent and keeps the diff between two
//! versions of the package small (a finding that is seen again only touches `status.cfg`), which keeps
//! concurrent updates from different jobs cheap to redo.

use crate::triage::{read_cfg, write_cfg, Cfg};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// Stable identity of a finding: strategy, failing config and a hash of what makes it unique
/// (the crash signature for crashes, the reduced/original program text for wrong code).
pub fn key_of(dir: &Path) -> Option<String> {
    let meta = read_cfg(&dir.join("repro.cfg"));
    let strategy = meta.get("strategy")?;
    let bad = meta.get("bad_name")?;
    let ident = match std::fs::read_to_string(dir.join("signature.txt")) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => std::fs::read_to_string(dir.join("reduced.rs")).or_else(|_| std::fs::read_to_string(dir.join("case.rs"))).ok()?,
    };
    Some(format!("{}__{}__{:010x}", sanitize(strategy), sanitize(bad), fnv(&ident) & 0xff_ffff_ffff))
}

fn class_of(dir: &Path) -> &'static str {
    let n = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = dir.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    if parent == "triaged" {
        "triaged"
    } else if parent == "known" || n.contains("__rust-") || n.contains("__known") {
        "known"
    } else if n.contains("__unreported") {
        "triaged"
    } else {
        "new"
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_file() {
            let _ = std::fs::copy(&p, to.join(e.file_name()));
        }
    }
}

fn today() -> String {
    std::process::Command::new("date").args(["-u", "+%F"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn add_word(cfg: &mut Cfg, key: &str, word: &str) {
    let cur = cfg.get(key).cloned().unwrap_or_default();
    if !cur.split(',').any(|w| w == word) {
        let new = if cur.is_empty() { word.to_string() } else { format!("{cur},{word}") };
        cfg.insert(key.to_string(), new);
    }
}

pub fn run(pkg: &Path, adds: &[PathBuf], toolchain: &str, contains: Option<&Path>) -> i32 {
    let root = pkg.join("findings");
    std::fs::create_dir_all(&root).unwrap();
    if let Some(keys) = contains {
        let mut missing = 0;
        for k in std::fs::read_to_string(keys).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !root.join(k).join("case.rs").exists() {
                eprintln!("MISSING {k}");
                missing += 1;
            }
        }
        eprintln!("pack: {} keys missing", missing);
        return (missing > 0) as i32;
    }
    let date = today();
    for add in adds {
        // accept either a finding directory or a directory of finding directories
        let mut dirs = Vec::new();
        if add.join("case.rs").exists() {
            dirs.push(add.clone());
        } else {
            for e in std::fs::read_dir(add).into_iter().flatten().flatten() {
                if e.path().join("case.rs").exists() {
                    dirs.push(e.path());
                }
            }
        }
        dirs.sort();
        for d in dirs {
            let Some(key) = key_of(&d) else { continue };
            let dest = root.join(&key);
            let mut st: Cfg;
            if dest.join("case.rs").exists() {
                st = read_cfg(&dest.join("status.cfg"));
                // a fresher triage result (reduced program, status) replaces a stale one
                let new_st = read_cfg(&d.join("status.cfg"));
                let have_reduced = dest.join("reduced.rs").exists();
                if (!have_reduced && d.join("reduced.rs").exists()) || (new_st.get("triaged_at") > st.get("triaged_at") && d.join("reduced.rs").exists()) {
                    for f in ["reduced.rs", "STATUS.md", "ISSUE.md", "report.md", "signature.txt", "known-issues.txt"] {
                        if d.join(f).exists() {
                            let _ = std::fs::copy(d.join(f), dest.join(f));
                        }
                    }
                    for (k, v) in new_st {
                        st.insert(k, v);
                    }
                }
            } else {
                copy_dir(&d, &dest);
                st = read_cfg(&dest.join("status.cfg"));
                st.insert("first_seen".into(), date.clone());
                st.insert("times_seen".into(), "0".into());
            }
            let n: u64 = st.get("times_seen").and_then(|v| v.parse().ok()).unwrap_or(0);
            st.insert("times_seen".into(), (n + 1).to_string());
            st.insert("last_seen".into(), date.clone());
            st.insert("class".into(), class_of(&d).into());
            st.insert("key".into(), key.clone());
            add_word(&mut st, "seen_on", toolchain);
            if st.get("state").map(|s| s == "fixed").unwrap_or(false) {
                // it is back
                st.insert("state".into(), "open".into());
            }
            st.entry("state".into()).or_insert_with(|| "open".into());
            write_cfg(&dest.join("status.cfg"), &st);
            println!("PACKED {key}");
        }
    }
    write_index(pkg);
    0
}

pub fn write_index(pkg: &Path) {
    let root = pkg.join("findings");
    let mut rows: Vec<(String, Cfg)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("case.rs").exists())
        .map(|e| (e.file_name().to_string_lossy().to_string(), read_cfg(&e.path().join("status.cfg"))))
        .collect();
    let rank = |c: &Cfg| match (c.get("state").map(String::as_str), c.get("verdict").map(String::as_str)) {
        (Some("fixed"), _) => 9,
        (_, Some("READY TO REPORT")) => 0,
        (_, Some("ALMOST READY")) => 1,
        (_, Some("NOT READY")) => 3,
        _ => 2,
    };
    rows.sort_by(|a, b| rank(&a.1).cmp(&rank(&b.1)).then(a.0.cmp(&b.0)));
    let mut s = String::from("# Stored findings\n\nOne row per distinct finding; every directory under `findings/` has `STATUS.md` (verdict, reproduction per channel, UB check, upstream search) and, when reduced, `reduced.rs` + `ISSUE.md`.\n\n");
    let _ = writeln!(s, "| verdict | state | class | finding | kind | reduced lines | seen | last seen | upstream |\n|---|---|---|---|---|---|---|---|---|");
    for (k, c) in &rows {
        let g = |n: &str| c.get(n).cloned().unwrap_or_else(|| "-".into());
        let up = std::fs::read_to_string(root.join(k).join("known-issues.txt")).map(|t| t.split_whitespace().take(3).collect::<Vec<_>>().join(" ")).unwrap_or_else(|_| g("upstream_search"));
        let _ = writeln!(s, "| {} | {} | {} | `{k}` | {} | {} | {}x | {} | {} |", g("verdict"), g("state"), g("class"), g("kind"), g("reduced_lines"), g("times_seen"), g("last_seen"), up.chars().take(80).collect::<String>());
    }
    let _ = writeln!(s, "\n{} findings.", rows.len());
    std::fs::write(pkg.join("INDEX.md"), s).unwrap();
}
