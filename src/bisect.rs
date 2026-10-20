//! Regression range of a (reduced) finding: binary search over stable releases (1.80 .. latest) and
//! over the last year of nightlies. Probes install the toolchain with rustup, run the reproducer and
//! uninstall it again (disk). Assumes the bug, once introduced, stays in later releases.

use crate::runner::{exec, Config, Limits, Outcome};
use crate::triage::{reproduces, Repro};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub struct Bisect {
    /// human readable lines for STATUS.md
    pub lines: Vec<String>,
    /// one-line summary for status.cfg / the index
    pub summary: String,
    /// the points of contact for the issue's version table: (toolchain with its `rustc -V`, reproduces)
    pub edges: Vec<(String, bool)>,
}

const OLDEST_STABLE_MINOR: u32 = 80;
const NIGHTLY_DAYS: i64 = 365;

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

fn date_add(date: &str, days: i64) -> String {
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
    results: std::collections::HashMap<String, (String, String, bool)>,
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
        let res = match r {
            Repro::Yes => true,
            Repro::No => {
                if let Outcome::CompileFail(e) = &a {
                    self.notes.push(format!("{tc}: reference config `{}` does not compile ({})", self.base.name, e.lines().find(|l| l.starts_with("error")).unwrap_or("").chars().take(100).collect::<String>()));
                }
                let _ = &b;
                false
            }
            Repro::NotApplicable => false,
        };
        self.results.insert(tc.to_string(), (used, version, res));
        Some(res)
    }

    /// `name (rustc -V)` of a probed toolchain, for the version table.
    fn label(&self, tc: &str) -> String {
        match self.results.get(tc) {
            Some((used, ver, _)) => format!("{used} ({ver})"),
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

pub fn bisect(src: &Path, base: &Config, bad: &Config, sig: &Option<String>, work: &Path, budget: Duration, stable_reproduces: bool, nightly_reproduces: bool) -> Bisect {
    let mut cx = Ctx { src, base, bad, sig, work, deadline: Instant::now() + budget, notes: Vec::new(), results: Default::default() };
    let mut edges: Vec<(String, bool)> = Vec::new();
    let mut lines = Vec::new();
    let mut summary = Vec::new();

    if stable_reproduces {
        let latest: i64 = rustc_version("stable").split_whitespace().nth(1).and_then(|v| v.split('.').nth(1)).and_then(|m| m.parse().ok()).unwrap_or(0);
        if latest > OLDEST_STABLE_MINOR as i64 {
            let tc = |m: i64| format!("1.{m}.0");
            let lo = OLDEST_STABLE_MINOR as i64;
            match cx.probe(&tc(lo)) {
                Some(true) => {
                    edges.push((cx.label(&tc(lo)), true));
                    lines.push(format!("stable: already present in {} (oldest version tested)", tc(lo)));
                    summary.push(format!("stable: since <= {}", tc(lo)));
                }
                Some(false) => {
                    let (g, b) = search(lo, latest, |m| cx.probe(&tc(m)));
                    // `latest` itself was seen reproducing by the caller; label it with the installed stable
                    edges.push((cx.label(&tc(g)), false));
                    edges.push((if b == latest { format!("stable {} ({})", tc(b), rustc_version("stable")) } else { cx.label(&tc(b)) }, true));
                    if b - g == 1 {
                        lines.push(format!("stable: **first affected release is {}** (last good: {})", tc(b), tc(g)));
                        summary.push(format!("stable: first bad {}", tc(b)));
                    } else {
                        lines.push(format!("stable: regressed after {} and no later than {} (time budget used up)", tc(g), tc(b)));
                        summary.push(format!("stable: ({}, {}]", tc(g), tc(b)));
                    }
                }
                None => lines.push("stable: not bisected (could not test the oldest release)".into()),
            }
        }
    }

    if nightly_reproduces {
        // the date of the installed nightly, e.g. "rustc 1.101.0-nightly (8006091eb 2026-10-17)"
        let v = rustc_version("nightly");
        let hi_date = v.rsplit(' ').next().map(|d| d.trim_end_matches(')').to_string()).filter(|d| d.len() == 10).unwrap_or_default();
        if !hi_date.is_empty() {
            // rustc reports the commit date, the `nightly-D` toolchain published the day after
            let day = |i: i64| format!("nightly-{}", date_add(&hi_date, 1 - i));
            let lo = NIGHTLY_DAYS;
            match cx.probe(&day(0 + lo)) {
                Some(true) => {
                    edges.push((cx.label(&day(lo)), true));
                    lines.push(format!("nightly: already present in {} (a year back, oldest tested)", day(lo)));
                    summary.push("nightly: older than a year".into());
                }
                Some(false) => {
                    // j = NIGHTLY_DAYS - days back: 0 = oldest (good) .. NIGHTLY_DAYS = newest (bad)
                    let (g, b) = search(0, NIGHTLY_DAYS, |j| cx.probe(&day(NIGHTLY_DAYS - j)));
                    let (gt, bt) = (day(NIGHTLY_DAYS - g), day(NIGHTLY_DAYS - b));
                    edges.push((cx.label(&gt), false));
                    edges.push((if b == NIGHTLY_DAYS { format!("{bt} ({v})") } else { cx.label(&bt) }, true));
                    if b - g == 1 {
                        lines.push(format!("nightly: **first affected: {bt}** (last good: {gt}); `cargo bisect-rustc --start {} --end {}` narrows it to a PR", &gt[8..], &bt[8..]));
                        summary.push(format!("nightly: first bad {bt}"));
                    } else {
                        lines.push(format!("nightly: regressed after {gt} and no later than {bt} (time budget used up)"));
                        summary.push(format!("nightly: ({gt}, {bt}]"));
                    }
                }
                None => lines.push("nightly: not bisected (could not test a year-old nightly)".into()),
            }
        }
    }
    for n in cx.notes.iter().take(6) {
        lines.push(format!("note: {n}"));
    }
    Bisect { lines, summary: summary.join("; "), edges }
}
