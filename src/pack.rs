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

use crate::triage::{Cfg, read_cfg, write_cfg};
use log::{info, warn};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

type Row = (String, Cfg);

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
    let list = |s: &str| {
        format!(
            "[{}]",
            s.split(',')
                .filter(|x| !x.is_empty())
                .map(json_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let flags = |s: &str| {
        format!(
            "[{}]",
            s.split_whitespace()
                .map(json_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let repro: Vec<String> = st
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("repro.")
                .map(|ch| format!("    {}: {}", json_str(ch), json_str(v)))
        })
        .collect();
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
    field(
        "base",
        format!(
            "{{\"name\": {}, \"flags\": {}}}",
            json_str(&g(&meta, "base_name")),
            flags(&g(&st, "min_base_flags"))
        ),
    );
    field(
        "bad",
        format!(
            "{{\"name\": {}, \"flags\": {}}}",
            json_str(&g(&meta, "bad_name")),
            flags(&g(&st, "min_bad_flags"))
        ),
    );
    field(
        "original_flags",
        format!(
            "{{\"base\": {}, \"bad\": {}}}",
            flags(&g(&meta, "base_flags")),
            flags(&g(&meta, "bad_flags"))
        ),
    );
    field("common_flags", flags(&g(&st, "min_common_flags")));
    field(
        "signature",
        json_str(
            std::fs::read_to_string(dir.join("signature.txt"))
                .unwrap_or_default()
                .trim(),
        ),
    );
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
    field("regression", json_str(&g(&st, "regression")));
    field("regression_edges", json_str(&g(&st, "regression_edges")));
    s.push_str(&format!("  \"repro\": {{\n{}\n  }},\n", repro.join(",\n")));
    let mut st2 = st.clone();
    crate::steps::backfill(&mut st2, dir);
    let today = today();
    let h = st2
        .get("context_hash")
        .or_else(|| st2.get("reduced_hash"))
        .cloned()
        .unwrap_or_default();
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
    s.push_str(&format!(
        "  \"steps\": {{\n{}\n  }},\n",
        steps_json.join(",\n")
    ));
    s.push_str("  \"files\": {\"case\": \"case.rs\", \"reduced\": \"reduced.rs\", \"issue\": \"ISSUE.md\", \"status\": \"STATUS.md\", \"report\": \"report.md\"}\n}\n");
    let _ = std::fs::write(dir.join("finding.json"), s);
}

/// Stable shard assignment by directory name (the stale list shrinks while shards work on it).
pub fn name_hash(s: &str) -> u64 {
    fnv(s)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Stable identity of a finding: strategy, failing config and a hash of what makes it unique
/// (the crash signature for crashes, the reduced/original program text for wrong code).
pub fn key_of(dir: &Path) -> Option<String> {
    // a stored finding keeps its key for ever (re-triage changes reduced.rs, not the identity)
    if let Some(k) = read_cfg(&dir.join("status.cfg"))
        .get("key")
        .filter(|k| !k.is_empty())
    {
        return Some(k.clone());
    }
    let meta = read_cfg(&dir.join("repro.cfg"));
    let strategy = meta.get("strategy")?;
    let bad = meta.get("bad_name")?;
    let ident = match std::fs::read_to_string(dir.join("signature.txt")) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => std::fs::read_to_string(dir.join("case.rs")).ok()?,
    };
    Some(format!(
        "{}__{}__{:010x}",
        sanitize(strategy),
        sanitize(bad),
        fnv(&ident) & 0xff_ffff_ffff
    ))
}

/// A Miri mismatch that only comes from the unspecified sign of a NaN (Miri randomises it, the hardware does not): `to_bits()` of
/// a float that is NaN. Stored by generators that did not canonicalise NaNs yet; such a finding says nothing about any compiler.
fn nan_sign_miri_mismatch(dir: &Path) -> bool {
    let bad = read_cfg(&dir.join("repro.cfg")).get("bad_name").cloned().unwrap_or_default();
    if !bad.starts_with("miri") {
        return false;
    }
    let src = std::fs::read_to_string(dir.join("case.rs")).unwrap_or_default();
    // the shared prelude (`impl Feed for f64/f32`) always mentions is_nan(), so it must not count as a guard
    let body: String = src.lines().filter(|l| !l.contains("impl Feed for f")).collect::<Vec<_>>().join("\n");
    body.contains("NAN") && body.contains(".to_bits()") && !body.contains("is_nan()")
}

fn class_of(dir: &Path) -> &'static str {
    let n = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
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
    std::process::Command::new("date")
        .args(["-u", "+%F"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn add_word(cfg: &mut Cfg, key: &str, word: &str) {
    let cur = cfg.get(key).cloned().unwrap_or_default();
    if !cur.split(',').any(|w| w == word) {
        let new = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur},{word}")
        };
        cfg.insert(key.to_string(), new);
    }
}

/// Keys of findings that stopped reproducing (the directories of `old_findings/`).
pub fn fixed_keys(pkg: &Path) -> std::collections::HashSet<String> {
    std::fs::read_dir(pkg.join("old_findings"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

fn nightly_version() -> String {
    std::process::Command::new("rustc")
        .args(["+nightly", "-V"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Move `findings/<key>` to `old_findings/<key>`: it does not show up on the newest nightly any more (`state` says why).
pub fn archive(pkg: &Path, key: &str, state: &str, date: &str, rustc: &str) {
    let (from, to) = (
        pkg.join("findings").join(key),
        pkg.join("old_findings").join(key),
    );
    let mut st = read_cfg(&from.join("status.cfg"));
    st.insert("state".into(), state.into());
    st.insert("fixed_on".into(), date.into());
    st.insert("fixed_rustc".into(), rustc.into());
    write_cfg(&from.join("status.cfg"), &st);
    let _ = std::fs::create_dir_all(pkg.join("old_findings"));
    let _ = std::fs::remove_dir_all(&to);
    if std::fs::rename(&from, &to).is_err() {
        copy_dir(&from, &to);
        let _ = std::fs::remove_dir_all(&from);
    }
}

/// Sort out the package: the former `fixed.txt` becomes stubs in `old_findings/`, and every finding whose last check on the newest nightly
/// found nothing moves to `old_findings/` - at once, whoever stored it and whenever the check ran.
fn migrate(pkg: &Path) {
    let date = today();
    if let Ok(text) = std::fs::read_to_string(pkg.join("fixed.txt")) {
        for l in text.lines() {
            let c: Vec<&str> = l.split('\t').collect();
            let Some(key) = c.first().filter(|k| !k.is_empty()) else {
                continue;
            };
            if pkg.join("findings").join(key).join("case.rs").exists()
                || pkg.join("old_findings").join(key).exists()
            {
                continue;
            }
            let dir = pkg.join("old_findings").join(key);
            let _ = std::fs::create_dir_all(&dir);
            let mut st = Cfg::new();
            st.insert("state".into(), "fixed".into());
            st.insert("fixed_on".into(), c.get(1).unwrap_or(&"").to_string());
            st.insert("fixed_rustc".into(), c.get(2).unwrap_or(&"").to_string());
            st.insert("first_seen".into(), c.get(3).unwrap_or(&"").to_string());
            st.insert("stub".into(), "yes (only the key was kept)".into());
            write_cfg(&dir.join("status.cfg"), &st);
        }
        let _ = std::fs::remove_file(pkg.join("fixed.txt"));
    }
    let mut ver = String::new();
    let mut keys: Vec<String> = std::fs::read_dir(pkg.join("findings"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    keys.sort();
    for k in keys {
        let dir = pkg.join("findings").join(&k);
        if nan_sign_miri_mismatch(&dir) {
            println!("DROPPED {k} (Miri mismatch caused by the unspecified sign of a NaN)");
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        }
        let st = read_cfg(&dir.join("status.cfg"));
        if st.get("repro.nightly").map(String::as_str) == Some("no") {
            if ver.is_empty() {
                ver = nightly_version();
            }
            let on = st
                .get("last_verified")
                .cloned()
                .unwrap_or_else(|| date.clone());
            println!("ARCHIVED {k} (does not reproduce on the newest nightly)");
            archive(pkg, &k, "fixed", &on, &ver);
        }
    }
}

pub fn run(pkg: &Path, adds: &[PathBuf], toolchain: &str, contains: Option<&Path>) -> i32 {
    let root = pkg.join("findings");
    std::fs::create_dir_all(&root).unwrap();
    if let Some(keys) = contains {
        let mut missing = 0;
        let fixed = fixed_keys(pkg);
        for k in std::fs::read_to_string(keys)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            if !root.join(k).join("case.rs").exists() && !fixed.contains(k) {
                warn!("MISSING {k}");
                missing += 1;
            }
        }
        info!("pack: {} keys missing", missing);
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
            if nan_sign_miri_mismatch(&d) {
                println!("IGNORED {key} (Miri mismatch caused by the unspecified sign of a NaN)");
                continue;
            }
            let mut key = key;
            // the same reduced program under another key (e.g. found again by another seed) is the same finding
            let new_hash = read_cfg(&d.join("status.cfg"))
                .get("reduced_hash")
                .cloned()
                .unwrap_or_default();
            if !new_hash.is_empty() && !root.join(&key).join("case.rs").exists() {
                let bad = read_cfg(&d.join("repro.cfg"))
                    .get("bad_name")
                    .cloned()
                    .unwrap_or_default();
                for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                    let other = read_cfg(&e.path().join("status.cfg"));
                    if other.get("reduced_hash") == Some(&new_hash)
                        && read_cfg(&e.path().join("repro.cfg")).get("bad_name") == Some(&bad)
                    {
                        key = e.file_name().to_string_lossy().to_string();
                        break;
                    }
                }
            }
            let dest = root.join(&key);
            // A finding that was fixed (it lives in old_findings/ and in fixed.txt) comes back only when the newest nightly
            // shows it again; stable or beta finding it again, or a triage that was running meanwhile, must not bring it back.
            if !dest.join("case.rs").exists() && fixed_keys(pkg).contains(&key) {
                if toolchain == "nightly" || toolchain == "import" {
                    let _ = std::fs::remove_dir_all(pkg.join("old_findings").join(&key));
                    println!("REOPENED {key} (reproduces on the newest nightly again)");
                } else {
                    println!("SKIPPED {key} (fixed on the newest nightly)");
                    continue;
                }
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
                if d.join("reduced.rs").exists()
                    && (better
                        || (!have_reduced
                            && crate::steps::score(&b, &date) == crate::steps::score(&a, &date)))
                {
                    // every file the triage wrote (culprit.txt, variants.txt, miri.txt, search_llvm.txt ... included); the identity of the
                    // finding (case.rs, repro.cfg), its merged status.cfg and finding.json stay as they are
                    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                        let name = e.file_name().to_string_lossy().to_string();
                        if e.path().is_file()
                            && !["case.rs", "repro.cfg", "status.cfg", "finding.json"]
                                .contains(&name.as_str())
                        {
                            let _ = std::fs::copy(e.path(), dest.join(&name));
                        }
                    }
                    // results the new triage did not produce (a culprit that is not there any more ...) must not outlive their step
                    for f in [
                        "culprit.txt",
                        "variants.txt",
                        "miri.txt",
                        "search.txt",
                        "search_llvm.txt",
                        "bisect.txt",
                        "LLVM_ISSUE.md",
                    ] {
                        if !d.join(f).exists() {
                            let _ = std::fs::remove_file(dest.join(f));
                        }
                    }
                    for (k, v) in new_st {
                        st.insert(k, v);
                    }
                }
                // whatever the triage found out that the stored copy does not have yet is kept, even when the stored result is the better one
                // (only for the same reduced program: the files belong to it)
                let same_program = a.contains_key("reduced_hash")
                    && a.get("reduced_hash") == b.get("reduced_hash");
                for f in [
                    "culprit.txt",
                    "variants.txt",
                    "miri.txt",
                    "search.txt",
                    "search_llvm.txt",
                    "bisect.txt",
                    "LLVM_ISSUE.md",
                ] {
                    if same_program && d.join(f).exists() && !dest.join(f).exists() {
                        let _ = std::fs::copy(d.join(f), dest.join(f));
                    }
                }
            } else {
                copy_dir(&d, &dest);
                st = read_cfg(&dest.join("status.cfg"));
                st.insert("first_seen".into(), date.clone());
                st.insert("times_seen".into(), "0".into());
            }
            if toolchain != "retriage" {
                let n: u64 = st
                    .get("times_seen")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                st.insert("times_seen".into(), (n + 1).to_string());
                st.insert("last_seen".into(), date.clone());
            }
            st.insert("class".into(), class_of(&d).into());
            st.insert("key".into(), key.clone());
            if toolchain != "retriage" {
                add_word(&mut st, "seen_on", toolchain);
            }
            if toolchain == "nightly" || toolchain == "import" {
                // it reproduces on the newest nightly right now
                st.insert("repro.nightly".into(), "yes".into());
                st.insert("nightly_misses".into(), "0".into());
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
    migrate(pkg);
    write_index(pkg);
    0
}

/// `yes`/`no` from the note of the step `llvm_issue`; `N/A` when it does not apply (not an LLVM culprit, crash, cross target); `?` when not verified yet.
fn fixed_on_llvm_trunk(st: &Cfg) -> &'static str {
    let Some(s) = crate::steps::get(st, "llvm_issue") else {
        return "open";
    };
    match s.status.as_str() {
        "n/a" => "N/A",
        "done" if s.note.contains("FIXED ON TRUNK") => "yes",
        "done" if s.note.contains("STILL WRONG ON TRUNK") => "no",
        "done" if s.note.starts_with("not verified") => {
            "N/A (neither the pass nor the whole opt + llc pipeline reproduces it)"
        }
        _ => "open",
    }
}

/// The optimisation pass that changes the result: from `culprit.txt`, or - when that file is not there - from the note of the step
/// `pass_bisect` ("first LLVM pass that matters: 1844 of 1993: stack-coloring on reduced::main"). `None` when no pass was isolated.
fn pass_of(dir: &Path, st: &Cfg) -> Option<(String, String)> {
    let t = std::fs::read_to_string(dir.join("culprit.txt")).unwrap_or_default();
    let get = |key: &str| {
        t.lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let (kind, name) = (get("kind="), get("name="));
    if !name.is_empty() && get("n=").parse::<usize>().unwrap_or(0) > 0 {
        return Some((kind, name));
    }
    let note = crate::steps::get(st, "pass_bisect")?.note;
    let rest = note.strip_prefix("first ")?;
    let (kind, rest) = rest.split_once(" pass that matters: ")?;
    let name = rest
        .split(": ")
        .nth(1)?
        .split(" on ")
        .next()?
        .trim()
        .to_string();
    (!name.is_empty()).then(|| (kind.to_lowercase(), name))
}

pub fn write_index(pkg: &Path) {
    let root = pkg.join("findings");
    let mut rows: Vec<(String, Cfg)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("case.rs").exists())
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                read_cfg(&e.path().join("status.cfg")),
            )
        })
        .collect();
    let today = today();
    let rank = |c: &Cfg, dir: &str| {
        let mut c2 = c.clone();
        crate::steps::backfill(&mut c2, &root.join(dir));
        if c2.get("state").map(String::as_str) == Some("fixed") {
            99
        } else {
            100 - crate::steps::progress(&c2, &today).0
        }
    };
    rows.sort_by(|a, b| rank(&a.1, &a.0).cmp(&rank(&b.1, &b.0)).then(a.0.cmp(&b.0)));
    let mut s = format!(
        "# Stored findings\n\nOne row per distinct finding. The `target` column says what it is compiled for: `host` is plain `rustc file.rs`, anything else needs `rustc --target <triple> ... file.rs && ./file` (e.g. `i686-unknown-linux-gnu`, run natively on x86_64 with gcc-multilib). Every directory under `findings/` has `finding.json` (with the checklist), `STATUS.md` (checklist, reproduction per channel, minimisation, UB check) and, once triaged, `reduced.rs` + `ISSUE.md`. The checklist says what was done and what is still open; steps are redone when the tool gets better at them or when the reduced program changes.\n\n## Steps of every finding\n\nEvery finding goes through exactly these steps, in this order; `steps done` below counts them, and every other column is the result of one of them.\n\n{}\n",
        crate::steps::list_md(),
    );
    let groups = same_bug_groups(&root, &rows, &today);
    let folded: std::collections::HashSet<&str> = groups.iter().flat_map(|(_, m)| m.iter().map(String::as_str)).collect();
    rows.retain(|(k, _)| !folded.contains(k.as_str()));
    let finished = |k: &str, c: &Cfg| {
        let mut c2 = c.clone();
        crate::steps::backfill(&mut c2, &root.join(k));
        let (d, t) = crate::steps::progress(&c2, &today);
        d == t
    };
    let (full, partial): (Vec<Row>, Vec<Row>) =
        rows.iter().cloned().partition(|(k, c)| finished(k, c));
    let table = |s: &mut String, set: &[(String, Cfg)]| {
        let _ = writeln!(
            s,
            "| steps done | open steps | finding | same bug | target | kind | reduced lines | fixed in llvm trunk | minimisation | seen | last seen |\n|---|---|---|---|---|---|---|---|---|---|---|"
        );
        for (k, c) in set {
            let mut c2 = c.clone();
            crate::steps::backfill(&mut c2, &root.join(k));
            let g = |n: &str| c.get(n).cloned().unwrap_or_else(|| "-".into());
            let (done, total) = crate::steps::progress(&c2, &today);
            let next = {
                let ids = crate::steps::open_ids(&c2, &today);
                if ids.is_empty() {
                    "none".to_string()
                } else {
                    ids.join(", ")
                }
            };
            let _ = writeln!(
                s,
                "| {done}/{total} | {} | `{k}` | {} | {} | {} | {} | {} | {} | {}x | {} |",
                next.replace('|', "/"),
                groups.iter().find(|(r, _)| r == k).map_or("-".to_string(), |(_, m)| format!("+{}", m.len())),
                target_of(c),
                g("kind"),
                g("reduced_lines"),
                fixed_on_llvm_trunk(&c2),
                g("min_status"),
                g("times_seen"),
                g("last_seen")
            );
        }
    };
    for (k, _) in &rows {
        write_finding_json(&root.join(k));
    }
    let _ = writeln!(
        s,
        "## Fully tested ({} finding(s), every step done)\n\nEvery step is done for these. Findings that are very likely the same bug are folded into one row (see below).\n",
        full.len()
    );
    table(&mut s, &full);
    if !groups.is_empty() {
        let _ = writeln!(
            s,
            "\n## Folded duplicates ({} finding(s) in {} group(s))\n\nFindings with the same kind, target, culprit pass and regressing commit are most likely one bug. Only the best of each group is in the tables (`same bug` shows how many are folded into it); the others are still in `findings/`.\n",
            groups.iter().map(|(_, m)| m.len()).sum::<usize>(),
            groups.len()
        );
        for (rep, members) in &groups {
            let label = rows
                .iter()
                .find(|(k, _)| k == rep)
                .map(|(k, c)| {
                    let pass = pass_of(&root.join(k), c).map(|(_, p)| p).unwrap_or_default();
                    let pr = c.get("regression_pr").cloned().unwrap_or_default();
                    let sha = pr
                        .split("/commit/")
                        .nth(1)
                        .map(|r| r.chars().take_while(char::is_ascii_hexdigit).collect::<String>())
                        .unwrap_or_default();
                    let commit = crate::bisect::commit_label(&pr, &sha);
                    if commit.is_empty() { pass } else { format!("{pass}, {commit}") }
                })
                .unwrap_or_default();
            let _ = writeln!(
                s,
                "- `{rep}` ({label}) also: {}",
                members.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ")
            );
        }
    }
    let _ = writeln!(
        s,
        "\n## Not fully tested yet ({} finding(s))\n\nThe checklist is not complete for these: the next retriage runs work on them, closest to completion first. Do not report them yet.\n",
        partial.len()
    );
    table(&mut s, &partial);
    std::fs::write(pkg.join("INDEX.md"), s).unwrap();
    write_old_index(pkg);
}

/// Findings that are very likely one bug: the same kind, target, culprit pass and regressing commit. Returns `(representative, folded members)`
/// per group of two or more; the representative is the one with the most steps done, then the shortest reduced program. A finding without
/// a culprit pass is never folded (nothing says it is the same bug).
fn same_bug_groups(root: &Path, rows: &[(String, Cfg)], today: &str) -> Vec<(String, Vec<String>)> {
    type Member = (String, (usize, usize));
    let mut by_key: std::collections::BTreeMap<String, Vec<Member>> = Default::default();
    for (k, c) in rows {
        let mut c2 = c.clone();
        crate::steps::backfill(&mut c2, &root.join(k));
        if c2.get("state").map(String::as_str) == Some("fixed") {
            continue;
        }
        let Some((pass_kind, pass)) = pass_of(&root.join(k), c) else {
            continue;
        };
        let commit = c
            .get("regression_pr")
            .and_then(|pr| pr.split_whitespace().find(|w| w.starts_with("https://github.com/rust-lang/rust/commit/")))
            .unwrap_or("");
        let key = format!(
            "{}|{}|{pass_kind}:{pass}|{commit}",
            c.get("kind").map_or("", String::as_str),
            target_of(c)
        );
        let lines = c.get("reduced_lines").and_then(|l| l.parse().ok()).unwrap_or(usize::MAX);
        let done = crate::steps::progress(&c2, today).0;
        by_key.entry(key).or_default().push((k.clone(), (usize::MAX - done, lines)));
    }
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (_, mut members) in by_key {
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let rep = members.remove(0).0;
        out.push((rep, members.into_iter().map(|m| m.0).collect()));
    }
    out.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    out
}

/// The compilation target a finding needs: from `--target` in any of its flag sets, else the host.
fn target_of(c: &Cfg) -> String {
    for key in [
        "min_bad_flags",
        "min_base_flags",
        "min_common_flags",
        "bad_flags",
        "base_flags",
    ] {
        let Some(flags) = c.get(key) else { continue };
        let words: Vec<&str> = flags.split_whitespace().collect();
        for (i, w) in words.iter().enumerate() {
            if let Some(t) = w.strip_prefix("--target=") {
                return format!("`{t}`");
            }
            if *w == "--target"
                && let Some(t) = words.get(i + 1)
            {
                return format!("`{t}`");
            }
        }
    }
    "host (x86_64-unknown-linux-gnu)".to_string()
}

/// `old_findings/INDEX.md`: findings that stopped reproducing on the newest nightly (kept apart from the current ones).
fn write_old_index(pkg: &Path) {
    let dir = pkg.join("old_findings");
    if !dir.exists() {
        return;
    }
    let mut rows: Vec<(String, Cfg, bool)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                read_cfg(&e.path().join("status.cfg")),
                e.path().join("case.rs").exists(),
            )
        })
        .collect();
    rows.sort_by(|a, b| {
        b.1.get("fixed_on")
            .cmp(&a.1.get("fixed_on"))
            .then(a.0.cmp(&b.0))
    });
    let mut s = String::from(
        "# Old findings\n\nThese stopped reproducing on the newest nightly (most are fixed in LLVM / rustc). They are not triaged any more. A finding that the nightly shows again moves back to `findings/`.\n\n| finding | state | no longer reproduces since | checked with | first seen | files kept |\n|---|---|---|---|---|---|\n",
    );
    for (k, st, kept) in &rows {
        let g = |n: &str| st.get(n).cloned().unwrap_or_default();
        let _ = writeln!(
            s,
            "| `{k}` | {} | {} | {} | {} | {} |",
            g("state"),
            g("fixed_on"),
            g("fixed_rustc"),
            g("first_seen"),
            if *kept { "yes" } else { "key only" }
        );
    }
    let _ = std::fs::write(dir.join("INDEX.md"), s);
}

#[cfg(test)]
mod nan_tests {
    use super::*;

    fn finding(bad: &str, src: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nan_sign_{}_{}", std::process::id(), bad));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("repro.cfg"), format!("bad_name={bad}\n")).unwrap();
        std::fs::write(d.join("case.rs"), src).unwrap();
        d
    }

    #[test]
    fn miri_mismatch_on_nan_bits_is_ignored() {
        let old = "let v = f64::NAN; h.put(&v.to_bits());";
        assert!(nan_sign_miri_mismatch(&finding("miri-mismatch", old)));
        assert!(!nan_sign_miri_mismatch(&finding("O3", old)));
        let fixed = "let v = f64::NAN; h.put(&if v.is_nan() { 1 } else { v.to_bits() });";
        assert!(!nan_sign_miri_mismatch(&finding("miri-mismatch-b", fixed)));
        let with_prelude = format!(
            "impl Feed for f64 {{ fn feed_into(&self, h: &mut H) {{ h.feed(if self.is_nan() {{ 1 }} else {{ self.to_bits() }}) }} }}\n{old}"
        );
        assert!(nan_sign_miri_mismatch(&finding("miri-mismatch-c", &with_prelude)));
    }
}
