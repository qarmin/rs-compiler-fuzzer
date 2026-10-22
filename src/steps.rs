//! The checklist of a stored finding: which of the steps on the way to a good upstream report are done, which are still open.
//!
//! Every step has a `version`. When the tool gets better at a step (a smarter reducer, a more exact bisection ...), the version is
//! bumped here and every finding that was checked with the older version has that step open again. Steps whose result depends on the
//! program text (`code: true`) are also open again as soon as the reduced program changes (their `hash` no longer matches): a
//! new minimisation that touches the code invalidates everything that was derived from the old code.
//!
//! Stored in `status.cfg` as `step.<id>=<status>;v=<version>;h=<hash of reduced.rs>;d=<date>;n=<note>`.

use crate::triage::Cfg;
use std::path::Path;

pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    pub version: u32,
    /// the result depends on the reduced program (invalidated when it changes)
    pub code: bool,
    /// repeat after this many days (0 = never)
    pub max_age_days: u32,
}

/// Version of the minimiser: bump it when a pass is added or improved. The step `minimized` is then open again and the
/// convergence count starts from zero (the next runs continue from the previous result).
/// 2: `print!` is turned back into `println!` at the end
pub const REDUCE_VERSION: u32 = 2;

pub const STEPS: &[Step] = &[
    Step { id: "reproduced", title: "Reproduced on stable, beta and nightly (reduced program)", version: 1, code: true, max_age_days: 0 },
    Step { id: "minimize_cases", title: "Batch reduced to the cases that matter (a single case if one is enough)", version: 1, code: false, max_age_days: 0 },
    Step { id: "minimized", title: "Code minimised (every pass finished, a second run changed nothing)", version: REDUCE_VERSION, code: false, max_age_days: 0 },
    Step { id: "flags", title: "Shortest command line (fewest flags that still show it)", version: 1, code: true, max_age_days: 0 },
    Step { id: "opt_levels", title: "Other optimisation levels tried (which ones show the bug and which do not)", version: 1, code: true, max_age_days: 0 },
    Step { id: "ub_check", title: "Checked for undefined behaviour (Miri)", version: 2, code: true, max_age_days: 0 },
    Step { id: "mir_llvm", title: "MIR optimisations or LLVM? (-Zmir-opt-level=0)", version: 1, code: true, max_age_days: 0 },
    Step { id: "regression", title: "Regression or long-standing bug? (decides whether the bisection below applies)", version: 1, code: true, max_age_days: 0 },
    Step { id: "bisect_stable", title: "Regression: first stable release that has it", version: 2, code: true, max_age_days: 0 },
    Step { id: "bisect_nightly", title: "Regression: first nightly that has it", version: 2, code: true, max_age_days: 0 },
    Step { id: "bisect_pr", title: "Regression: offending commit/PR (cargo bisect-rustc; \"not found\" is a result too)", version: 2, code: true, max_age_days: 0 },
    Step { id: "upstream", title: "Searched upstream for duplicates", version: 1, code: true, max_age_days: 14 },
    Step { id: "issue_draft", title: "ISSUE.md draft written", version: 1, code: true, max_age_days: 0 },
];

pub fn step(id: &str) -> &'static Step {
    STEPS.iter().find(|s| s.id == id).expect("unknown step")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// `done`, `partial`, `failed`, `n/a` or `todo`
    pub status: String,
    pub version: u32,
    pub hash: String,
    pub date: String,
    pub note: String,
}

impl State {
    pub fn new(status: &str, hash: &str, date: &str, note: &str) -> State {
        State { status: status.into(), version: 0, hash: hash.into(), date: date.into(), note: note.replace(';', ",").replace('\n', " ") }
    }
}

fn key(id: &str) -> String {
    format!("step.{id}")
}

pub fn get(st: &Cfg, id: &str) -> Option<State> {
    let v = st.get(&key(id))?;
    let mut parts = v.splitn(2, ';');
    let status = parts.next()?.to_string();
    let mut s = State { status, ..Default::default() };
    for kv in parts.next().unwrap_or("").split(';') {
        match kv.split_once('=') {
            Some(("v", x)) => s.version = x.parse().unwrap_or(0),
            Some(("h", x)) => s.hash = x.to_string(),
            Some(("d", x)) => s.date = x.to_string(),
            Some(("n", x)) => s.note = x.to_string(),
            _ => {}
        }
    }
    // the note is last and may contain '=' only (';' was replaced when it was written)
    Some(s)
}

/// Store a step result with the current version of the step.
pub fn set(st: &mut Cfg, id: &str, mut s: State) {
    s.version = step(id).version;
    st.insert(key(id), format!("{};v={};h={};d={};n={}", s.status, s.version, s.hash, s.date, s.note));
}

fn age_days(date: &str, today: &str) -> i64 {
    let ts = |d: &str| {
        std::process::Command::new("date").args(["-u", "-d", d, "+%s"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok()).unwrap_or(0)
    };
    (ts(today) - ts(date)) / 86400
}

/// A stored result that does not have to be computed again.
pub fn is_current(id: &str, s: &State, reduced_hash: &str, today: &str) -> bool {
    let def = step(id);
    if !matches!(s.status.as_str(), "done" | "n/a" | "failed") || s.version != def.version {
        return false;
    }
    if def.code && !s.hash.is_empty() && s.hash != reduced_hash {
        return false;
    }
    if def.max_age_days > 0 && !s.date.is_empty() && age_days(&s.date, today) > def.max_age_days as i64 {
        return false;
    }
    true
}

pub fn current(st: &Cfg, id: &str, reduced_hash: &str, today: &str) -> Option<State> {
    get(st, id).filter(|s| is_current(id, s, reduced_hash, today))
}

fn ctx_hash(st: &Cfg) -> String {
    st.get("context_hash").or_else(|| st.get("reduced_hash")).cloned().unwrap_or_default()
}

/// Results derived from older versions of the tool carry no program hash; once the program changes they cannot be trusted.
pub fn forget_unhashed(st: &mut Cfg) {
    for s in STEPS.iter().filter(|s| s.code) {
        if matches!(get(st, s.id), Some(x) if x.hash.is_empty()) {
            st.remove(&key(s.id));
        }
    }
}

/// Steps that count as finished (done or not applicable).
pub fn is_complete(id: &str, st: &Cfg, reduced_hash: &str, today: &str) -> bool {
    matches!(current(st, id, reduced_hash, today), Some(s) if s.status == "done" || s.status == "n/a")
}

/// `(finished, total)`.
pub fn progress(st: &Cfg, today: &str) -> (usize, usize) {
    let h = ctx_hash(st);
    (STEPS.iter().filter(|s| is_complete(s.id, st, &h, today)).count(), STEPS.len())
}

/// Ids of the steps still open.
pub fn open_ids(st: &Cfg, today: &str) -> Vec<&'static str> {
    let h = ctx_hash(st);
    STEPS.iter().filter(|s| !is_complete(s.id, st, &h, today)).map(|s| s.id).collect()
}

/// Titles of the steps still open, with the reason when known.
pub fn open_steps(st: &Cfg, today: &str) -> Vec<(&'static str, String)> {
    let h = ctx_hash(st);
    STEPS
        .iter()
        .filter(|s| !is_complete(s.id, st, &h, today))
        .map(|s| {
            let why = match get(st, s.id) {
                None => "not done yet".to_string(),
                Some(x) if x.version != s.version => "redone: the tool got better at this step".to_string(),
                Some(x) if s.code && !x.hash.is_empty() && x.hash != h => "redone: the reduced program changed".to_string(),
                Some(x) if x.status == "partial" => format!("started: {}", x.note),
                Some(x) if x.status == "failed" => format!("failed: {}", x.note),
                Some(x) if s.max_age_days > 0 && !matches!(x.status.as_str(), "todo") => format!("older than {} days, repeated", s.max_age_days),
                Some(x) => if x.note.is_empty() { x.status } else { x.note },
            };
            (s.title, why)
        })
        .collect()
}

/// A finding that failed a step for good (e.g. it reports undefined behaviour) is not worth more work.
pub fn has_failure(st: &Cfg) -> bool {
    STEPS.iter().any(|s| matches!(get(st, s.id), Some(x) if x.status == "failed"))
}

/// Markdown checklist for `STATUS.md` / the report.
pub fn checklist_md(st: &Cfg, today: &str) -> String {
    let h = ctx_hash(st);
    let (done, total) = progress(st, today);
    let mut s = format!("### Checklist ({done}/{total} steps done)\n\n");
    for def in STEPS {
        let cur = current(st, def.id, &h, today);
        let (mark, note, date) = match (&cur, get(st, def.id)) {
            (Some(c), _) if c.status == "done" => ("[x]", c.note.clone(), c.date.clone()),
            (Some(c), _) if c.status == "n/a" => ("[x]", format!("not applicable: {}", c.note), c.date.clone()),
            (Some(c), _) => ("[!]", c.note.clone(), c.date.clone()),
            (None, Some(x)) if x.status == "partial" => ("[~]", x.note.clone(), x.date.clone()),
            (None, Some(x)) if x.version != def.version => ("[ ]", "to be done again (the tool got better at this step)".to_string(), String::new()),
            (None, Some(x)) if def.code && !x.hash.is_empty() && x.hash != h => ("[ ]", "to be done again (the reduced program changed)".to_string(), String::new()),
            (None, Some(x)) => ("[ ]", x.note.clone(), String::new()),
            (None, None) => ("[ ]", String::new(), String::new()),
        };
        let tail = match (note.is_empty(), date.is_empty()) {
            (true, true) => String::new(),
            (false, true) => format!(" - {note}"),
            (true, false) => format!(" ({date})"),
            (false, false) => format!(" - {note} ({date})"),
        };
        s.push_str(&format!("- {mark} {}{tail}\n", def.title));
    }
    let reported = st.get("state").map(|v| v == "reported" || v == "fixed").unwrap_or(false);
    let url = st.get("reported_url").cloned().unwrap_or_default();
    s.push_str(&format!("- {} Reported upstream (manual){}\n", if reported { "[x]" } else { "[ ]" }, if url.is_empty() { String::new() } else { format!(" - {url}") }));
    s
}

/// Compare two results of the same finding: more finished steps is better (used when two jobs triaged it concurrently).
pub fn score(st: &Cfg, today: &str) -> (usize, u32) {
    (progress(st, today).0, st.get("min_runs").and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Findings stored before the checklist existed: derive the steps from the fields the older triage wrote.
pub fn backfill(st: &mut Cfg, dir: &Path) {
    if st.keys().any(|k| k.starts_with("step.")) || !st.contains_key("triaged_at") {
        return;
    }
    let date = st.get("triaged_at").cloned().unwrap_or_default();
    // no program hash on purpose: such results are trusted until the program changes (see `forget_unhashed`)
    let hash = String::new();
    let g = |st: &Cfg, k: &str| st.get(k).cloned().unwrap_or_default();
    let put = |st: &mut Cfg, id: &str, status: &str, note: String| {
        set(st, id, State::new(status, &hash, &date, &note));
    };
    let repro_yes: Vec<String> = st.iter().filter_map(|(k, v)| k.strip_prefix("repro.").filter(|_| v.starts_with("yes")).map(String::from)).collect();
    if !repro_yes.is_empty() {
        put(st, "reproduced", "done", format!("reproduces on {}", repro_yes.join(", ")));
    }
    let reg = g(st, "regression");
    let ms = g(st, "min_status");
    if ms.starts_with("converged") {
        put(st, "minimized", "done", ms.clone());
    } else if !ms.is_empty() {
        put(st, "minimized", "partial", ms.clone());
    }
    if st.contains_key("min_bad_flags") {
        put(st, "flags", "done", format!("reference: `{}`, failing: `{}`", g(st, "min_base_flags"), g(st, "min_bad_flags")));
    }
    let miri = g(st, "miri");
    if miri.starts_with("ok") {
        put(st, "ub_check", "done", "Miri found no undefined behaviour".into());
    } else if miri.starts_with("ub") {
        put(st, "ub_check", "failed", miri.clone());
    } else if miri.starts_with("skipped: not applicable") {
        put(st, "ub_check", "n/a", "compiler crash".into());
    }
    if let Ok(r) = std::fs::read_to_string(dir.join("reduced.rs")) {
        let n = r.matches("mod case_").count();
        if n <= 1 {
            put(st, "minimize_cases", "done", if n == 0 { "reduced to a plain program".into() } else { "a single case".into() });
        } else {
            put(st, "minimize_cases", "partial", format!("{n} cases of the batch are still needed together"));
        }
    }
    if reg.contains("first bad") {
        put(st, "regression", "done", "regression".into());
    }
    let mo = g(st, "mir_opt");
    if mo.starts_with("llvm") || mo.starts_with("mir") {
        put(st, "mir_llvm", "done", mo.clone());
    }
    // results that stopped at the old search windows (1.80 / one year) are not final answers
    if reg.contains("stable:") && !reg.contains("since <= 1.80.0") {
        put(st, "bisect_stable", "done", reg.split("; ").find(|p| p.starts_with("stable:")).unwrap_or("").to_string());
    }
    if reg.contains("nightly:") && !reg.contains("older than a year") {
        put(st, "bisect_nightly", "done", reg.split("; ").find(|p| p.starts_with("nightly:")).unwrap_or("").to_string());
    }
    if st.contains_key("regression_pr") {
        put(st, "bisect_pr", "done", g(st, "regression_pr"));
    }
    let up = g(st, "upstream_search");
    if !up.is_empty() && up != "not performed" {
        put(st, "upstream", "done", up);
    }
    if dir.join("ISSUE.md").exists() {
        put(st, "issue_draft", "done", String::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(id: &str, status: &str, hash: &str, date: &str) -> Cfg {
        let mut st = Cfg::new();
        set(&mut st, id, State::new(status, hash, date, "note"));
        st
    }

    #[test]
    fn code_steps_are_reset_when_the_program_changes() {
        let st = cfg_with("mir_llvm", "done", "aaa", "2026-10-01");
        assert!(current(&st, "mir_llvm", "aaa", "2026-10-02").is_some());
        assert!(current(&st, "mir_llvm", "bbb", "2026-10-02").is_none(), "a new reduced program invalidates the step");
    }

    #[test]
    fn minimisation_itself_does_not_depend_on_the_program_hash() {
        let st = cfg_with("minimized", "done", "", "2026-10-01");
        assert!(current(&st, "minimized", "whatever", "2026-10-02").is_some());
    }

    #[test]
    fn a_new_tool_version_reopens_the_step() {
        let mut st = cfg_with("bisect_pr", "done", "aaa", "2026-10-01");
        let old = st.get("step.bisect_pr").unwrap().replace("v=2", "v=0");
        st.insert("step.bisect_pr".into(), old);
        assert!(current(&st, "bisect_pr", "aaa", "2026-10-02").is_none());
        let open = open_steps(&st, "2026-10-02");
        assert!(open.iter().any(|(t, why)| t.contains("PR") && why.contains("tool got better")));
    }

    #[test]
    fn partial_and_failed_steps_are_not_complete_and_the_search_expires() {
        let st = cfg_with("minimized", "partial", "", "2026-10-01");
        assert!(!is_complete("minimized", &st, "", "2026-10-02"));
        let failed = cfg_with("ub_check", "failed", "aaa", "2026-10-01");
        assert!(has_failure(&failed) && !is_complete("ub_check", &failed, "aaa", "2026-10-02"));
        let up = cfg_with("upstream", "done", "aaa", "2026-10-01");
        assert!(is_complete("upstream", &up, "aaa", "2026-10-10"));
        assert!(!is_complete("upstream", &up, "aaa", "2026-11-01"), "the search is repeated after 14 days");
    }

    #[test]
    fn legacy_findings_get_a_checklist_without_hashes() {
        let mut st = Cfg::new();
        for (k, v) in [("triaged_at", "2026-10-20"), ("min_status", "converged"), ("miri", "ok: fine"), ("repro.stable", "yes (x)"), ("min_bad_flags", "-C opt-level=1")] {
            st.insert(k.into(), v.into());
        }
        backfill(&mut st, std::path::Path::new("/nonexistent"));
        assert!(is_complete("minimized", &st, "any", "2026-10-21"));
        assert!(is_complete("ub_check", &st, "any", "2026-10-21"));
        assert!(!is_complete("bisect_pr", &st, "any", "2026-10-21"));
        forget_unhashed(&mut st);
        assert!(get(&st, "ub_check").is_none() && get(&st, "minimized").is_some());
    }
}
