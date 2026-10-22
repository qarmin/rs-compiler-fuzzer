//! Regression range of a (reduced) finding: binary search over stable releases (1.80 .. latest) and
//! over the last year of nightlies. Probes install the toolchain with rustup, run the reproducer and
//! uninstall it again (disk). Assumes the bug, once introduced, stays in later releases.

use crate::runner::{exec, Config, Limits, Outcome};
use crate::triage::{reproduces, Repro};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// One of the three regression searches (stable releases, nightlies, the PR).
#[derive(Default, Clone, Debug)]
pub struct Part {
    /// a precise result (or "older than the search window")
    pub done: bool,
    /// the search does not apply (the bug does not reproduce on that channel)
    pub na: bool,
    /// human readable lines for STATUS.md
    pub lines: Vec<String>,
    /// one-line summary for status.cfg / the index
    pub summary: String,
    /// the points of contact for the issue's version table: (toolchain with its `rustc -V`, reproduces)
    pub edges: Vec<(String, bool)>,
    /// why it is not done (budget used up, tool missing ...)
    pub note: String,
    /// nightly: the two consecutive days between which it appeared (`YYYY-MM-DD`), input of the PR search
    pub range: Option<(String, String)>,
}

#[derive(Default, Clone, Debug)]
pub struct Bisect {
    pub stable: Part,
    pub nightly: Part,
    pub pr: Part,
    pub notes: Vec<String>,
}

/// Which searches to run (the others keep their stored result).
#[derive(Clone, Copy)]
pub struct Want {
    pub stable: bool,
    pub nightly: bool,
    pub pr: bool,
}

impl Bisect {
    pub fn lines(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for p in [&self.stable, &self.nightly] {
            v.extend(p.lines.iter().cloned());
        }
        v.extend(self.pr.lines.iter().cloned());
        v.extend(self.notes.iter().map(|n| format!("note: {n}")));
        v
    }
    pub fn summary(&self) -> String {
        let mut v: Vec<&str> = Vec::new();
        for p in [&self.stable, &self.nightly, &self.pr] {
            if !p.summary.is_empty() {
                v.push(&p.summary);
            }
        }
        v.join("; ")
    }
    pub fn edges(&self) -> Vec<(String, bool)> {
        self.stable.edges.iter().chain(self.nightly.edges.iter()).cloned().collect()
    }
    /// Findings stored before `bisect.txt` existed: rebuild what was known from `regression` / `regression_edges` / `regression_pr`.
    pub fn from_legacy(st: &crate::triage::Cfg) -> Option<Bisect> {
        let reg = st.get("regression")?;
        if reg == "unknown" || reg.is_empty() {
            return None;
        }
        let edges: Vec<(String, bool)> = st
            .get("regression_edges")
            .map(|e| e.split(" ; ").filter_map(|x| x.rsplit_once(" => ").map(|(l, y)| (l.to_string(), y == "yes"))).collect())
            .unwrap_or_default();
        let mut b = Bisect::default();
        for piece in reg.split("; ") {
            if let Some(rest) = piece.strip_prefix("stable:") {
                b.stable.summary = piece.to_string();
                b.stable.lines.push(format!("stable:{rest}"));
                b.stable.done = rest.contains("first bad");
            } else if let Some(rest) = piece.strip_prefix("nightly:") {
                b.nightly.summary = piece.to_string();
                b.nightly.lines.push(format!("nightly:{rest}"));
                b.nightly.done = rest.contains("first bad");
                if let Some(d) = rest.trim().strip_prefix("first bad nightly-") {
                    b.nightly.range = Some((date_add(d, -1), d.to_string()));
                }
            }
        }
        for (l, y) in edges {
            if l.starts_with("nightly-") { b.nightly.edges.push((l, y)) } else { b.stable.edges.push((l, y)) }
        }
        if let Some(pr) = st.get("regression_pr") {
            b.pr.lines = pr.split(" | ").map(|l| format!("- {l}")).collect();
            b.pr.summary = format!("pr: {pr}");
            b.pr.done = !b.pr.lines.is_empty();
        }
        Some(b)
    }

    /// Plain text form kept next to the finding (`bisect.txt`), so a later run can keep what is already known.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (name, p) in [("stable", &self.stable), ("nightly", &self.nightly), ("pr", &self.pr)] {
            s += &format!("[{name}]\ndone={}\nna={}\nsummary={}\nnote={}\n", p.done, p.na, p.summary, p.note);
            if let Some((a, b)) = &p.range {
                s += &format!("range={a},{b}\n");
            }
            for l in &p.lines {
                s += &format!("line={l}\n");
            }
            for (l, y) in &p.edges {
                s += &format!("edge={l} => {}\n", if *y { "yes" } else { "no" });
            }
        }
        for n in &self.notes {
            s += &format!("note-all={n}\n");
        }
        s
    }

    pub fn from_text(t: &str) -> Bisect {
        let mut b = Bisect::default();
        let mut cur: Option<&mut Part> = None;
        // borrow-friendly: collect per section first
        let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
        for l in t.lines() {
            if let Some(name) = l.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                sections.push((name.to_string(), Vec::new()));
            } else if let Some(last) = sections.last_mut() {
                last.1.push(l);
            } else if let Some(n) = l.strip_prefix("note-all=") {
                b.notes.push(n.to_string());
            }
        }
        let _ = &mut cur;
        for (name, lines) in sections {
            let mut p = Part::default();
            for l in lines {
                if let Some(x) = l.strip_prefix("done=") {
                    p.done = x == "true";
                } else if let Some(x) = l.strip_prefix("na=") {
                    p.na = x == "true";
                } else if let Some(x) = l.strip_prefix("summary=") {
                    p.summary = x.to_string();
                } else if let Some(x) = l.strip_prefix("note=") {
                    p.note = x.to_string();
                } else if let Some(x) = l.strip_prefix("range=") {
                    p.range = x.split_once(',').map(|(a, b)| (a.to_string(), b.to_string()));
                } else if let Some(x) = l.strip_prefix("line=") {
                    p.lines.push(x.to_string());
                } else if let Some(x) = l.strip_prefix("edge=") {
                    if let Some((lab, y)) = x.rsplit_once(" => ") {
                        p.edges.push((lab.to_string(), y == "yes"));
                    }
                } else if let Some(x) = l.strip_prefix("note-all=") {
                    b.notes.push(x.to_string());
                }
            }
            match name.as_str() {
                "stable" => b.stable = p,
                "nightly" => b.nightly = p,
                "pr" => b.pr = p,
                _ => {}
            }
        }
        b
    }
}

/// The search goes back as far as the program can be compiled at all: the generated programs are edition 2021, which
/// needs rustc 1.56 (nightlies from mid 2021). There is no other limit on how old a regression may be.
const OLDEST_STABLE_MINOR: i64 = 56;
const OLDEST_NIGHTLY: &str = "2021-06-01";
/// Extra time for `cargo bisect-rustc` once the nightly range is a single day.
const PR_SECONDS: u64 = 900;

fn days_between(from: &str, to: &str) -> i64 {
    let ts = |d: &str| Command::new("date").args(["-u", "-d", d, "+%s"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok()).unwrap_or(0);
    (ts(to) - ts(from)) / 86400
}

fn rustup(args: &[&str], secs: u64) -> bool {
    let mut c = Command::new("rustup");
    c.args(args);
    matches!(exec(c, Duration::from_secs(secs)), Ok(r) if r.code == Some(0))
}

fn install(tc: &str) -> bool {
    rustup(&["toolchain", "install", tc, "--profile", "minimal", "--no-self-update"], 400)
}

fn uninstall(tc: &str) {
    rustup(&["toolchain", "uninstall", tc], 120);
}

pub(crate) fn date_add(date: &str, days: i64) -> String {
    let out = Command::new("date").args(["-u", "-d", &format!("{date} {days:+} days"), "+%F"]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn rustc_version(tc: &str) -> String {
    let mut c = Command::new("rustc");
    c.arg(format!("+{tc}")).arg("-V");
    exec(c, Duration::from_secs(30)).map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string()).unwrap_or_default()
}

struct Ctx<'a> {
    src: &'a Path,
    base: &'a Config,
    bad: &'a Config,
    sig: &'a Option<String>,
    work: &'a Path,
    deadline: Instant,
    notes: Vec<String>,
    /// requested toolchain -> (toolchain actually used, `rustc -V`, reproduces)
    results: std::collections::HashMap<String, (String, String, bool, bool)>,
}

impl Ctx<'_> {
    /// Some(true) = reproduces, Some(false) = does not, None = could not be tested (install failed / out of time)
    fn probe(&mut self, tc: &str) -> Option<bool> {
        if Instant::now() > self.deadline {
            return None;
        }
        // a nightly that was never published (or is not installable) is replaced by the next day's
        let mut used = tc.to_string();
        let mut installed = install(&used);
        if !installed {
            if let Some(date) = tc.strip_prefix("nightly-") {
                for d in 1..=2 {
                    used = format!("nightly-{}", date_add(date, d));
                    installed = install(&used);
                    if installed {
                        break;
                    }
                }
            }
        }
        if !installed {
            self.notes.push(format!("{tc}: could not be installed"));
            return None;
        }
        let tc_used = used.as_str();
        let version = rustc_version(tc_used);
        let rustc = vec!["rustc".to_string(), format!("+{tc_used}")];
        let lim = Limits { compile: Duration::from_secs(300), run: Duration::from_secs(20) };
        let (r, a, b) = reproduces(&rustc, self.src, self.base, self.bad, self.sig, &lim, self.work);
        uninstall(tc_used);
        let mut ref_ok = true;
        let res = match r {
            Repro::Yes => true,
            Repro::No => {
                if let Outcome::CompileFail(e) = &a {
                    ref_ok = false;
                    self.notes.push(format!("{tc}: reference config `{}` does not compile ({})", self.base.name, e.lines().find(|l| l.starts_with("error")).unwrap_or("").chars().take(100).collect::<String>()));
                }
                let _ = &b;
                false
            }
            Repro::NotApplicable => false,
        };
        self.results.insert(tc.to_string(), (used, version, res, ref_ok));
        Some(res)
    }

    /// The probe could run the reference configuration (an older compiler that cannot compile the program says nothing).
    fn ref_ok(&self, tc: &str) -> bool {
        self.results.get(tc).map(|r| r.3).unwrap_or(true)
    }

    /// `name (rustc -V)` of a probed toolchain, for the version table.
    fn label(&self, tc: &str) -> String {
        match self.results.get(tc) {
            Some((used, ver, _, ok)) if used.starts_with(|c: char| c.is_ascii_digit()) => format!("stable {used} ({ver}){}", if *ok { "" } else { " [reference build does not compile]" }),
            Some((used, ver, _, ok)) => format!("{used} ({ver}){}", if *ok { "" } else { " [reference build does not compile]" }),
            None => tc.to_string(),
        }
    }
}

/// Smallest index in `lo+1..=hi` for which `probe` says yes, given `lo` = no and `hi` = yes.
fn search(mut lo: i64, mut hi: i64, mut probe: impl FnMut(i64) -> Option<bool>) -> (i64, i64) {
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        match probe(mid) {
            Some(true) => hi = mid,
            Some(false) => lo = mid,
            None => break,
        }
    }
    (lo, hi)
}

pub fn bisect(src: &Path, base: &Config, bad: &Config, sig: &Option<String>, work: &Path, budget: Duration, want: Want, stable_reproduces: bool, nightly_reproduces: bool, prior: &Bisect) -> Bisect {
    let mut cx = Ctx { src, base, bad, sig, work, deadline: Instant::now() + budget, notes: Vec::new(), results: Default::default() };
    let mut out = prior.clone();
    out.notes.clear();

    if want.stable {
        out.stable = Part::default();
        if !stable_reproduces {
            out.stable.na = true;
            out.stable.note = "does not reproduce on the current stable".into();
        } else {
            stable_search(&mut cx, &mut out.stable);
        }
    }
    if want.nightly {
        out.nightly = Part::default();
        if !nightly_reproduces {
            out.nightly.na = true;
            out.nightly.note = "does not reproduce on the current nightly".into();
        } else {
            nightly_search(&mut cx, &mut out.nightly);
        }
    }
    if want.pr {
        out.pr = Part::default();
        match out.nightly.range.clone() {
            Some((gt, bt)) => pr_search(src, base, bad, sig, work, &gt, &bt, Duration::from_secs(PR_SECONDS), &mut out.pr),
            None if out.nightly.na => {
                out.pr.na = true;
                out.pr.note = "does not reproduce on nightly".into();
            }
            None if out.nightly.done => {
                out.pr.na = true;
                out.pr.note = "long-standing bug (present in the oldest nightly that can compile it): not a regression".into();
            }
            None => out.pr.note = "the nightly range is not narrowed to one day yet".into(),
        }
    }
    for n in cx.notes.iter().take(6) {
        out.notes.push(n.clone());
    }
    out
}

fn stable_search(cx: &mut Ctx, part: &mut Part) {
    let latest: i64 = rustc_version("stable").split_whitespace().nth(1).and_then(|v| v.split('.').nth(1)).and_then(|m| m.parse().ok()).unwrap_or(0);
    if latest <= OLDEST_STABLE_MINOR {
        part.note = "cannot determine the latest stable release".into();
        return;
    }
    let tc = |m: i64| format!("1.{m}.0");
    let lo = OLDEST_STABLE_MINOR;
    match cx.probe(&tc(lo)) {
        Some(true) => {
            part.edges.push((cx.label(&tc(lo)), true));
            part.lines.push(format!("stable: already present in {} (the oldest release that can compile the program: not a regression)", tc(lo)));
            part.summary = format!("stable: since <= {}", tc(lo));
            part.done = true;
        }
        Some(false) => {
            let (g, b) = search(lo, latest, |m| cx.probe(&tc(m)));
            // `latest` itself was seen reproducing by the caller; label it with the installed stable
            part.edges.push((cx.label(&tc(g)), false));
            if b == latest && !cx.results.contains_key(&tc(b)) {
                cx.deadline = cx.deadline.max(Instant::now() + Duration::from_secs(120));
                cx.probe(&tc(b));
            }
            part.edges.push((cx.label(&tc(b)), true));
            if b - g == 1 {
                part.lines.push(format!("stable: **first affected release is {}** (last good: {})", tc(b), tc(g)));
                if !cx.ref_ok(&tc(g)) {
                    part.lines.push(format!("stable: caveat - {} cannot compile the program at all, so the bug may be older than it looks", tc(g)));
                }
                part.summary = format!("stable: first bad {}", tc(b));
                part.done = true;
            } else {
                part.lines.push(format!("stable: regressed after {} and no later than {} (time budget used up)", tc(g), tc(b)));
                part.summary = format!("stable: ({}, {}]", tc(g), tc(b));
                part.note = format!("range ({}, {}] not narrowed to one release (time budget used up)", tc(g), tc(b));
            }
        }
        None => part.note = "could not test the oldest release".into(),
    }
}

fn nightly_search(cx: &mut Ctx, part: &mut Part) {
    // the date of the installed nightly, e.g. "rustc 1.101.0-nightly (8006091eb 2026-10-17)"
    let v = rustc_version("nightly");
    let hi_date = v.rsplit(' ').next().map(|d| d.trim_end_matches(')').to_string()).filter(|d| d.len() == 10).unwrap_or_default();
    if hi_date.is_empty() {
        part.note = "cannot determine the date of the installed nightly".into();
        return;
    }
    // rustc reports the commit date, the `nightly-D` toolchain published the day after
    let day = |i: i64| format!("nightly-{}", date_add(&hi_date, 1 - i));
    // as far back as the program can be compiled (see OLDEST_NIGHTLY)
    let nightly_days = days_between(OLDEST_NIGHTLY, &hi_date).max(2);
    let lo = nightly_days;
    match cx.probe(&day(lo)) {
        Some(true) => {
            part.edges.push((cx.label(&day(lo)), true));
            part.lines.push(format!("nightly: already present in {} (the oldest nightly that can compile the program: not a regression)", day(lo)));
            part.summary = format!("nightly: since <= {}", day(lo));
            part.done = true;
        }
        Some(false) => {
            // j = nightly_days - days back: 0 = oldest (good) .. nightly_days = newest (bad)
            let (g, b) = search(0, nightly_days, |j| cx.probe(&day(nightly_days - j)));
            let (gt, bt) = (day(nightly_days - g), day(nightly_days - b));
            part.edges.push((cx.label(&gt), false));
            part.edges.push((if b == nightly_days { format!("{bt} ({v})") } else { cx.label(&bt) }, true));
            if b - g == 1 {
                part.lines.push(format!("nightly: **first affected: {bt}** (last good: {gt})"));
                if !cx.ref_ok(&gt) {
                    part.lines.push(format!("nightly: caveat - {gt} cannot compile the program at all, so the bug may be older than it looks"));
                }
                part.summary = format!("nightly: first bad {bt}");
                part.range = Some((gt[8..].to_string(), bt[8..].to_string()));
                part.done = true;
            } else {
                part.lines.push(format!("nightly: regressed after {gt} and no later than {bt} (time budget used up)"));
                part.summary = format!("nightly: ({gt}, {bt}]");
                part.note = format!("range ({gt}, {bt}] not narrowed to one day (time budget used up)");
            }
        }
        None => part.note = format!("could not test the oldest nightly ({})", day(lo)),
    }
}

/// Narrow a one-day nightly range down to the commit/PR with `cargo bisect-rustc` (needs it installed;
/// without it, or when it runs out of time, the part stays open and the dates are the answer).
fn pr_search(src: &Path, base: &Config, bad: &Config, sig: &Option<String>, work: &Path, start: &str, end: &str, budget: Duration, part: &mut Part) {
    let mut probe = Command::new("cargo-bisect-rustc");
    probe.arg("--version");
    if !matches!(exec(probe, Duration::from_secs(20)), Ok(r) if r.code == Some(0)) {
        part.note = "cargo-bisect-rustc is not installed".into();
        return;
    }
    let dir = work.join(format!("pr_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let it = crate::reduce::Interest { rustc: vec!["rustc".into()], a: base.clone(), b: bad.clone(), crash_sig: sig.clone(), limits: Limits { compile: Duration::from_secs(300), run: Duration::from_secs(20) }, work: dir.clone(), cache: Default::default() };
    let (cfg, script, file) = (dir.join("interest.cfg"), dir.join("test.sh"), dir.join("reduced.rs"));
    let me = std::env::current_exe().unwrap_or_else(|_| "rs-compiler-fuzzer".into());
    let _ = std::fs::write(&cfg, it.to_cfg());
    let _ = std::fs::copy(src, &file);
    // exit 0 = the bug is present = "regression" for `--regress success`
    let _ = std::fs::write(&script, format!("#!/bin/bash\nexec \"{}\" interesting --cfg \"{}\" --file \"{}\"\n", me.display(), cfg.display(), file.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
    }
    let mut c = Command::new("cargo-bisect-rustc");
    c.args(["--start", start, "--end", end, "--regress", "success"]).arg(format!("--script={}", script.display())).current_dir(&dir);
    c.args(["--timeout", "300"]);
    let out = exec(c, budget);
    let _ = std::fs::remove_dir_all(&dir);
    let (text, timed_out) = match out {
        Ok(r) => (format!("{}\n{}", String::from_utf8_lossy(&r.stdout), r.stderr), r.timed_out),
        Err(e) => {
            part.note = format!("cannot run cargo-bisect-rustc ({e})");
            return;
        }
    };
    let mut res: Vec<String> = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with("ERROR") || t.starts_with("error") {
            continue;
        }
        if (t.starts_with("Regression in") && !t.contains("nightly-")) || t.contains("rust-lang/rust/pull/") || t.contains("rust-lang/rust/compare/") || t.contains("rust-lang/rust/commit/") {
            res.push(format!("- {t}"));
        }
    }
    res.dedup();
    if res.is_empty() {
        let tail: Vec<&str> = text.lines().rev().filter(|l| !l.trim().is_empty()).take(2).collect();
        let why = tail.into_iter().rev().collect::<Vec<_>>().join(" / ").chars().take(200).collect::<String>();
        if timed_out {
            // not an answer yet: a later run tries again
            part.note = format!("cargo-bisect-rustc ran out of time after {}s ({why}); it is tried again", budget.as_secs());
        } else {
            // it was tried and finished without naming a commit: that is a result too
            part.lines = vec![format!("- regressed commit not found (cargo-bisect-rustc finished without naming one: {why})")];
            part.summary = "pr: regressed commit not found".into();
            part.done = true;
        }
        return;
    }
    res.truncate(8);
    part.summary = format!("pr: {}", res[0].trim_start_matches("- "));
    part.lines = res;
    part.done = true;
}
