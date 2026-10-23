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

/// Hash of a program ignoring whitespace, to recognise the same reduced program found twice.
pub fn text_hash(s: &str) -> String {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    format!("{:016x}", fnv(&compact))
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `finding.json`: the standard machine readable description of a stored finding (derived from
/// `repro.cfg` + `status.cfg`, rewritten whenever those change).
pub fn write_finding_json(dir: &Path) {
    let meta = read_cfg(&dir.join("repro.cfg"));
    let st = read_cfg(&dir.join("status.cfg"));
    let g = |m: &Cfg, k: &str| m.get(k).cloned().unwrap_or_default();
    let num = |m: &Cfg, k: &str| m.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let list = |s: &str| format!("[{}]", s.split(',').filter(|x| !x.is_empty()).map(json_str).collect::<Vec<_>>().join(", "));
    let flags = |s: &str| format!("[{}]", s.split_whitespace().map(json_str).collect::<Vec<_>>().join(", "));
    let repro: Vec<String> = st.iter().filter_map(|(k, v)| k.strip_prefix("repro.").map(|ch| format!("    {}: {}", json_str(ch), json_str(v)))).collect();
    let mut s = String::from("{\n");
    let mut field = |k: &str, v: String| {
        s.push_str(&format!("  {}: {},\n", json_str(k), v));
    };
    field("schema", "2".into());
    field("fuzzer_id", num(&st, "fuzzer_id").to_string());
    field("key", json_str(&g(&st, "key")));
    field("strategy", json_str(&g(&meta, "strategy")));
    field("kind", json_str(&g(&st, "kind")));
    field("class", json_str(&g(&st, "class")));
    field("state", json_str(&g(&st, "state")));
    field("progress", json_str(&g(&st, "progress")));
    field("base", format!("{{\"name\": {}, \"flags\": {}}}", json_str(&g(&meta, "base_name")), flags(&g(&st, "min_base_flags"))));
    field("bad", format!("{{\"name\": {}, \"flags\": {}}}", json_str(&g(&meta, "bad_name")), flags(&g(&st, "min_bad_flags"))));
    field("original_flags", format!("{{\"base\": {}, \"bad\": {}}}", flags(&g(&meta, "base_flags")), flags(&g(&meta, "bad_flags"))));
    field("common_flags", flags(&g(&st, "min_common_flags")));
    field("signature", json_str(std::fs::read_to_string(dir.join("signature.txt")).unwrap_or_default().trim()));
    field("original_lines", num(&st, "original_lines").to_string());
    field("reduced_lines", num(&st, "reduced_lines").to_string());
    field("reduced_hash", json_str(&g(&st, "reduced_hash")));
    field("first_seen", json_str(&g(&st, "first_seen")));
    field("last_seen", json_str(&g(&st, "last_seen")));
    field("triaged_at", json_str(&g(&st, "triaged_at")));
    field("last_verified", json_str(&g(&st, "last_verified")));
    field("times_seen", num(&st, "times_seen").to_string());
    field("seen_on", list(&g(&st, "seen_on")));
    field("miri", json_str(&g(&st, "miri")));
    field("upstream_search", json_str(&g(&st, "upstream_search")));
    field("regression", json_str(&g(&st, "regression")));
    field("regression_edges", json_str(&g(&st, "regression_edges")));
    s.push_str(&format!("  \"repro\": {{\n{}\n  }},\n", repro.join(",\n")));
    let mut st2 = st.clone();
    crate::steps::backfill(&mut st2, dir);
    let today = today();
    let h = st2.get("context_hash").or_else(|| st2.get("reduced_hash")).cloned().unwrap_or_default();
    let steps_json: Vec<String> = crate::steps::STEPS
        .iter()
        .map(|d| {
            let cur = crate::steps::current(&st2, d.id, &h, &today);
            let raw = crate::steps::get(&st2, d.id);
            let state = match (&cur, &raw) {
                (Some(c), _) => c.status.clone(),
                (None, Some(r)) if r.status == "partial" => "partial".to_string(),
                _ => "todo".to_string(),
            };
            let (note, date) = cur.as_ref().or(raw.as_ref()).map(|x| (x.note.clone(), x.date.clone())).unwrap_or_default();
            format!("    {}: {{\"title\": {}, \"state\": {}, \"version\": {}, \"date\": {}, \"note\": {}}}", json_str(d.id), json_str(d.title), json_str(&state), d.version, json_str(&date), json_str(&note))
        })
        .collect();
    s.push_str(&format!("  \"steps\": {{\n{}\n  }},\n", steps_json.join(",\n")));
    s.push_str("  \"files\": {\"case\": \"case.rs\", \"reduced\": \"reduced.rs\", \"issue\": \"ISSUE.md\", \"status\": \"STATUS.md\", \"report\": \"report.md\"}\n}\n");
    let _ = std::fs::write(dir.join("finding.json"), s);
}

/// Stable shard assignment by directory name (the stale list shrinks while shards work on it).
pub fn name_hash(s: &str) -> u64 {
    fnv(s)
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// Stable identity of a finding: strategy, failing config and a hash of what makes it unique
/// (the crash signature for crashes, the reduced/original program text for wrong code).
pub fn key_of(dir: &Path) -> Option<String> {
    // a stored finding keeps its key for ever (re-triage changes reduced.rs, not the identity)
    if let Some(k) = read_cfg(&dir.join("status.cfg")).get("key").filter(|k| !k.is_empty()) {
        return Some(k.clone());
    }
    let meta = read_cfg(&dir.join("repro.cfg"));
    let strategy = meta.get("strategy")?;
    let bad = meta.get("bad_name")?;
    let ident = match std::fs::read_to_string(dir.join("signature.txt")) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => std::fs::read_to_string(dir.join("case.rs")).ok()?,
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

/// Keys of findings that were removed because they no longer reproduce (`fixed.txt`: key, date, rustc, first seen).
pub fn fixed_keys(pkg: &Path) -> std::collections::HashSet<String> {
    std::fs::read_to_string(pkg.join("fixed.txt")).unwrap_or_default().lines().filter_map(|l| l.split('\t').next()).map(String::from).collect()
}

pub fn run(pkg: &Path, adds: &[PathBuf], toolchain: &str, contains: Option<&Path>) -> i32 {
    let root = pkg.join("findings");
    std::fs::create_dir_all(&root).unwrap();
    if let Some(keys) = contains {
        let mut missing = 0;
        let fixed = fixed_keys(pkg);
        for k in std::fs::read_to_string(keys).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !root.join(k).join("case.rs").exists() && !fixed.contains(k) {
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
            let mut key = key;
            // the same reduced program under another key (e.g. found again by another seed) is the same finding
            let new_hash = read_cfg(&d.join("status.cfg")).get("reduced_hash").cloned().unwrap_or_default();
            if !new_hash.is_empty() && !root.join(&key).join("case.rs").exists() {
                let bad = read_cfg(&d.join("repro.cfg")).get("bad_name").cloned().unwrap_or_default();
                for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                    let other = read_cfg(&e.path().join("status.cfg"));
                    if other.get("reduced_hash") == Some(&new_hash) && read_cfg(&e.path().join("repro.cfg")).get("bad_name") == Some(&bad) {
                        key = e.file_name().to_string_lossy().to_string();
                        break;
                    }
                }
            }
            let dest = root.join(&key);
            // a triage that was running while the finding got removed must not bring it back
            if toolchain == "retriage" && !dest.join("case.rs").exists() && fixed_keys(pkg).contains(&key) {
                println!("SKIPPED {key} (removed: no longer reproduces)");
                continue;
            }
            let mut st: Cfg;
            if dest.join("case.rs").exists() {
                st = read_cfg(&dest.join("status.cfg"));
                // a fresher triage result (reduced program, status) replaces a stale one
                let new_st = read_cfg(&d.join("status.cfg"));
                let have_reduced = dest.join("reduced.rs").exists();
                let mut a = st.clone();
                let mut b = new_st.clone();
                crate::steps::backfill(&mut a, &dest);
                crate::steps::backfill(&mut b, &d);
                // the result with more finished checklist steps (then more minimisation runs) wins
                let better = crate::steps::score(&b, &date) > crate::steps::score(&a, &date);
                if d.join("reduced.rs").exists() && (better || (!have_reduced && crate::steps::score(&b, &date) == crate::steps::score(&a, &date))) {
                    for f in ["reduced.rs", "STATUS.md", "ISSUE.md", "ISSUE_TITLE.txt", "report.md", "signature.txt", "known-issues.txt", "bisect.txt", "search.txt"] {
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
            if toolchain != "retriage" {
                let n: u64 = st.get("times_seen").and_then(|v| v.parse().ok()).unwrap_or(0);
                st.insert("times_seen".into(), (n + 1).to_string());
                st.insert("last_seen".into(), date.clone());
            }
            st.insert("class".into(), class_of(&d).into());
            st.insert("key".into(), key.clone());
            if toolchain != "retriage" {
                add_word(&mut st, "seen_on", toolchain);
            }
            if st.get("state").map(|s| s == "fixed").unwrap_or(false) {
                // it is back
                st.insert("state".into(), "open".into());
            }
            st.entry("state".into()).or_insert_with(|| "open".into());
            write_cfg(&dest.join("status.cfg"), &st);
            write_finding_json(&dest);
            println!("PACKED {key}");
        }
    }
    write_index(pkg);
    0
}

/// `rust-lang/rust#164603` -> markdown link; other text is kept.
fn linkify(text: &str) -> String {
    text.split_whitespace()
        .map(|w| match w.split_once('#') {
            Some((repo, n)) if repo.contains('/') && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("[{w}](https://github.com/{repo}/issues/{n})"),
            _ => w.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    let today = today();
    let rank = |c: &Cfg, dir: &str| {
        let mut c2 = c.clone();
        crate::steps::backfill(&mut c2, &root.join(dir));
        if c2.get("state").map(String::as_str) == Some("fixed") { 99 } else { 100 - crate::steps::progress(&c2, &today).0 }
    };
    rows.sort_by(|a, b| rank(&a.1, &a.0).cmp(&rank(&b.1, &b.0)).then(a.0.cmp(&b.0)));
    let mut s = String::from("# Stored findings\n\nOne row per distinct finding. Every directory under `findings/` has `finding.json` (with the checklist), `STATUS.md` (checklist, reproduction per channel, minimisation, UB check, upstream search) and, once triaged, `reduced.rs` + `ISSUE.md`. The checklist says what was done and what is still open; steps are redone when the tool gets better at them or when the reduced program changes.\n\n");
    let _ = writeln!(s, "| steps done | open steps | state | class | finding | kind | reduced lines | minimisation | seen | last seen | upstream |\n|---|---|---|---|---|---|---|---|---|---|---|");
    for (k, c) in &rows {
        let mut c2 = c.clone();
        crate::steps::backfill(&mut c2, &root.join(k));
        let g = |n: &str| c.get(n).cloned().unwrap_or_else(|| "-".into());
        let (done, total) = crate::steps::progress(&c2, &today);
        let open = crate::steps::open_steps(&c2, &today);
        let _ = &open;
        let next = { let ids = crate::steps::open_ids(&c2, &today); if ids.is_empty() { "none - ready for a human to review and report".to_string() } else { ids.join(", ") } };
        let up = std::fs::read_to_string(root.join(k).join("known-issues.txt")).map(|t| linkify(&t)).unwrap_or_else(|_| linkify(&g("upstream_search")));
        let _ = writeln!(s, "| {done}/{total} | {} | {} | {} | `{k}` | {} | {} | {} | {}x | {} | {} |", next.replace('|', "/"), g("state"), g("class"), g("kind"), g("reduced_lines"), g("min_status"), g("times_seen"), g("last_seen"), up);
    }
    for (k, _) in &rows {
        write_finding_json(&root.join(k));
    }
    let _ = writeln!(s, "\n{} findings.", rows.len());
    let gone = std::fs::read_to_string(pkg.join("fixed.txt")).unwrap_or_default();
    if !gone.trim().is_empty() {
        let _ = writeln!(s, "\n## Removed: no longer reproduce on the newest nightly\n\n| finding | removed | checked with | first seen |\n|---|---|---|---|");
        for l in gone.lines().rev().take(40) {
            let c: Vec<&str> = l.split('\t').collect();
            let _ = writeln!(s, "| `{}` | {} | {} | {} |", c.first().unwrap_or(&""), c.get(1).unwrap_or(&""), c.get(2).unwrap_or(&""), c.get(3).unwrap_or(&""));
        }
    }
    std::fs::write(pkg.join("INDEX.md"), s).unwrap();
}
