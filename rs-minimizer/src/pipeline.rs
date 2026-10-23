//! The reduction pipeline: one opening step (the cheap deletions every program profits from), then a scheduler that keeps running the
//! passes that remove the most per second until nothing helps, then the polish. Callers with knowledge of how their programs were
//! produced plug in through `Hook`.

use crate::bracket::{debracket, hollow, lexeme_deletions, lift_braces};
use crate::control::{effort, interrupted, last_run_complete, oracle_runs, par_checks, reset_deadline_flag, set_effort, set_stage};
use crate::deadcode::remove_dead_code;
use crate::exprs::{drop_calls, drop_let_bindings, flatten_blocks, simplify_exprs, simplify_let_tuples, simplify_lets, strip_parens};
use crate::format::format_if_interesting;
use crate::inline::{inline_calls, shorten_names, substitute_lets};
use crate::items::{shrink_items, shrink_items_with_users, shrink_pairs, shrink_siblings_step, shrink_statements};
use crate::lines::{shrink_lines, strip_comments};
use crate::modules::hoist_modules;
use crate::oracle::Oracle;
use crate::peep::peep;
use crate::structure::{remove_fields, remove_params, remove_variants_and_arms, stub_bodies, unwrap_newtypes};
use crate::tokens::shrink_tokens;
use crate::unroll::unroll;
use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Notes of a run, kept for the issue text and logged as they are written.
#[derive(Default)]
pub struct ReduceLog(pub Vec<String>);

impl ReduceLog {
    pub fn push(&mut self, line: impl Into<String>) {
        let line = line.into();
        info!("{line}");
        self.0.push(line);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    /// after a pass changed the program (a hook may collapse what the caller knows how to collapse)
    Flatten,
    /// last step before formatting
    Polish,
}

/// Called at every `Stage` with the current program; returns a replacement (checked by the pipeline) or `None`.
pub type Hook<'a> = &'a mut dyn FnMut(Stage, &str, Instant) -> Option<String>;

pub struct Options {
    pub budget: Duration,
    /// compiler command used by the passes that compile for their own diagnostics (empty switches them off)
    pub rustc: Vec<String>,
    /// top-level modules that must stay modules (the caller handles them itself)
    pub keep_module: Box<dyn Fn(&str) -> bool + Sync>,
}

impl Options {
    pub fn new(budget: Duration) -> Self {
        Options { budget, rustc: vec!["rustc".into()], keep_module: Box::new(|_| false) }
    }
}

fn stage(it: &dyn Oracle, hook: Hook, st: Stage, cur: &mut String, deadline: Instant, log: &mut ReduceLog) {
    if let Some(next) = hook(st, cur, deadline) {
        // what a hook builds is checked like everything else: a rebuilt program that lost the bug would poison every later step
        if next != *cur && !it.check(&next) {
            log.push(format!("{st:?}: the hook's program ({} lines) is not interesting - discarded", next.lines().count()));
        } else if next != *cur {
            log.push(format!("{st:?}: the hook changed the program ({} -> {} lines)", cur.lines().count(), next.lines().count()));
            *cur = next;
        }
    }
}

/// Step banners and one result line per pass (which pass, how much it removed), so a long run can be followed at a glance.
struct Progress {
    start: Instant,
    lines: usize,
    bytes: usize,
    n: usize,
    /// oracle calls and time at the previous RESULT line: what a pass cost
    last_runs: usize,
    last_at: Instant,
    initial: (usize, usize),
    runs0: usize,
    /// what each pass removed (bytes, may be negative for passes that grow the program), summed over the run
    gains: Vec<(String, i64, usize)>,
}

impl Progress {
    fn banner(&mut self, name: &str) {
        self.n += 1;
        info!("==================== {name} ====================");
    }

    /// Every pass is shown as `rs-minimizer: name`.
    fn label(name: &str) -> String {
        format!("rs-minimizer: {name}")
    }

    fn pass(&self, name: &str) {
        log::debug!("pass `{name}` starts at {:.0}s, {} lines / {} bytes, {} oracle calls so far", self.start.elapsed().as_secs_f32(), self.lines, self.bytes, oracle_runs());
        set_stage(&Self::label(name));
    }

    fn result(&mut self, log: &mut ReduceLog, tool: &str, cur: &str) {
        let tool = &Self::label(tool);
        let (l, b) = (cur.lines().count(), cur.len());
        let delta = |old: usize, new: usize| match new.cmp(&old) {
            std::cmp::Ordering::Less => format!("-{}", old - new),
            std::cmp::Ordering::Equal => "same".to_string(),
            std::cmp::Ordering::Greater => format!("+{}", new - old),
        };
        let runs = oracle_runs();
        let cost = format!("{} oracle calls in {:.1}s", runs - self.last_runs, self.last_at.elapsed().as_secs_f32());
        if l == self.lines && b == self.bytes {
            log.push(format!("RESULT [{tool}]: no change ({l} lines, {b} bytes); cost: {cost}; {:.0}s in total", self.start.elapsed().as_secs_f32()));
        } else {
            log.push(format!("RESULT [{tool}]: {} -> {l} lines ({}), {} -> {b} bytes ({}); cost: {cost}; {:.0}s in total", self.lines, delta(self.lines, l), self.bytes, delta(self.bytes, b), self.start.elapsed().as_secs_f32()));
        }
        crate::ui::pass_result(tool, self.bytes, b, runs - self.last_runs, self.last_at.elapsed().as_secs_f32());
        let removed = self.bytes as i64 - b as i64;
        match self.gains.iter_mut().find(|g| &g.0 == tool) {
            Some(g) => {
                g.1 += removed;
                g.2 += runs - self.last_runs;
            }
            None => self.gains.push((tool.to_string(), removed, runs - self.last_runs)),
        }
        self.last_runs = runs;
        self.last_at = Instant::now();
        self.lines = l;
        self.bytes = b;
    }
}

impl Progress {
    /// The last words of a run: from what to what, how long, how much work, and who did the shrinking.
    fn summary(&self, log: &mut ReduceLog, finished: bool) {
        let (l0, b0) = self.initial;
        let pct = |a: usize, b: usize| if a == 0 { 0.0 } else { 100.0 * (a as f64 - b as f64) / a as f64 };
        let secs = self.start.elapsed().as_secs_f64().max(0.001);
        let runs = oracle_runs() - self.runs0;
        log.push(format!(
            "SUMMARY{}: {l0} lines / {b0} bytes -> {} lines / {} bytes ({:.1}% fewer lines, {:.1}% fewer bytes) in {:.0}s; {runs} oracle calls ({:.1}/s)",
            if finished { "" } else { " (interrupted)" },
            self.lines,
            self.bytes,
            pct(l0, self.lines),
            pct(b0, self.bytes),
            secs,
            runs as f64 / secs
        ));
        let mut by_gain: Vec<&(String, i64, usize)> = self.gains.iter().filter(|g| g.1 != 0).collect();
        by_gain.sort_by_key(|g| std::cmp::Reverse(g.1.abs()));
        let top: Vec<String> = by_gain.iter().take(6).map(|g| format!("{} {}{} bytes ({} calls)", g.0.trim_start_matches("rs-minimizer: "), if g.1 > 0 { "-" } else { "+" }, g.1.abs(), g.2)).collect();
        log.push(format!("SUMMARY biggest contributions: {}", top.join("; ")));
        let refused = crate::control::refused_without_a_run();
        if refused > 0 {
            log.push(format!("SUMMARY {refused} candidates with unbalanced brackets were refused without a compiler run"));
        }
        let invalid = crate::control::invalid_candidates();
        if invalid > 0 {
            let reasons: Vec<String> = crate::control::rejection_reasons(5).iter().map(|(r, c)| format!("{c}x {r}")).collect();
            log.push(format!("SUMMARY candidates that did not compile: {invalid} of {} oracle calls; most frequent: {}", oracle_runs(), reasons.join(" | ")));
        }
    }
}


/// The program that came out of the last pass must still show the problem. If a pass lost it (a buggy pass, a hook, a flaky
/// oracle), the run goes back to the last program that did, instead of reducing something that is no test case any more.
fn guard(it: &dyn Oracle, cur: &mut String, good: &mut String, log: &mut ReduceLog, after: &str) -> bool {
    if *cur == *good {
        return true;
    }
    if it.recheck(cur) {
        *good = cur.clone();
        true
    } else {
        log.push(format!("GUARD: after `{after}` the program ({} lines) no longer shows the problem - going back to the last good one ({} lines)", cur.lines().count(), good.lines().count()));
        *cur = good.clone();
        false
    }
}

/// What a pass gets: the oracle, the options, its own deadline and the state some passes keep between their runs.
struct PassCtx<'a> {
    it: &'a dyn Oracle,
    opts: &'a Options,
    deadline: Instant,
    siblings_done: &'a RefCell<HashSet<u64>>,
}

struct PassDef {
    name: &'static str,
    /// the share of the program (per second) the pass is expected to remove before it has been measured
    prior: f64,
    /// only for programs up to this many lines ...
    max_lines: usize,
    /// ... or, for passes that grow the program, once this many seconds have passed
    big_after_secs: Option<u64>,
    needs_rustc: bool,
    min_effort: usize,
    run: fn(&PassCtx, &str) -> String,
}

fn passes() -> Vec<PassDef> {
    const ANY: usize = usize::MAX;
    let p = |name, prior, max_lines, needs_rustc, run: fn(&PassCtx, &str) -> String| PassDef { name, prior, max_lines, big_after_secs: None, needs_rustc, min_effort: 0, run };
    vec![
        p("comments", 0.0005, ANY, false, |c, s| strip_comments(c.it, s)),
        p("items with their users", 0.02, ANY, false, |c, s| shrink_items_with_users(c.it, s, c.deadline)),
        p("dead code", 0.05, ANY, true, |c, s| remove_dead_code(c.it, s, &c.opts.rustc, c.deadline)),
        p("siblings", 0.006, ANY, false, |c, s| shrink_siblings_step(c.it, s, c.deadline, usize::MAX, &mut c.siblings_done.borrow_mut()).0),
        p("hollow", 0.004, 3000, false, |c, s| hollow(c.it, s, c.deadline)),
        p("lexeme deletions", 0.002, 4000, false, |c, s| lexeme_deletions(c.it, s, c.deadline)),
        p("items", 0.003, 3000, false, |c, s| shrink_items(c.it, s, c.deadline)),
        p("lines", 0.001, 600, false, |c, s| shrink_lines(c.it, s, c.deadline)),
        PassDef { big_after_secs: Some(30), ..p("unroll", 0.002, 2000, false, |c, s| unroll(c.it, s)) },
        PassDef { big_after_secs: Some(30), ..p("hoist modules", 0.001, 2000, false, |c, s| hoist_modules(c.it, s, &*c.opts.keep_module, &c.opts.rustc)) },
        p("function bodies", 0.003, 2000, false, |c, s| stub_bodies(c.it, s, c.deadline)),
        p("newtypes", 0.001, 2000, false, |c, s| unwrap_newtypes(c.it, s, c.deadline)),
        p("parameters", 0.001, 2000, false, |c, s| remove_params(c.it, s, c.deadline)),
        p("fields", 0.001, 2000, false, |c, s| remove_fields(c.it, s, c.deadline)),
        p("variants and arms", 0.001, 2000, false, |c, s| remove_variants_and_arms(c.it, s, c.deadline)),
        p("statements", 0.002, 1500, false, |c, s| shrink_statements(c.it, s, c.deadline)),
        p("lift braces", 0.002, 1500, false, |c, s| lift_braces(c.it, s, c.deadline)),
        p("debracket", 0.001, 1500, false, |c, s| debracket(c.it, s, c.deadline)),
        p("method calls", 0.001, 1500, false, |c, s| drop_calls(c.it, s, c.deadline)),
        p("expressions", 0.001, 1500, false, |c, s| simplify_exprs(c.it, s, c.deadline)),
        p("let simplification", 0.0005, 1500, false, |c, s| simplify_lets(c.it, s, c.deadline)),
        p("inline", 0.001, 400, true, |c, s| inline_calls(c.it, s, &c.opts.rustc, c.deadline)),
        p("let bindings", 0.001, 400, false, |c, s| drop_let_bindings(c.it, s, c.deadline)),
        p("blocks", 0.001, 400, false, |c, s| flatten_blocks(c.it, s, c.deadline)),
        p("let tuples", 0.0005, 400, false, |c, s| simplify_let_tuples(c.it, s, c.deadline)),
        p("let substitution", 0.0005, 400, false, |c, s| substitute_lets(c.it, s, c.deadline)),
        p("peephole", 0.0005, 800, false, |c, s| peep(c.it, s, c.deadline)),
        p("tokens", 0.0003, 600, false, |c, s| shrink_tokens(c.it, s, c.deadline)),
        PassDef { min_effort: 1, ..p("pairs", 0.0002, 400, false, |c, s| shrink_pairs(c.it, s, c.deadline)) },
    ]
}

/// What a pass has done so far in this run.
#[derive(Clone, Default)]
struct Stat {
    runs: usize,
    /// moving average of the bytes it removed per second (negative for a pass that grew the program)
    rate: f64,
    /// the version of the program at which it last ran without changing anything
    idle_at: u64,
}

/// Runs the passes in the order of what they are expected to remove per second: the measured rate of the ones that have run, the
/// prior (scaled by the size of the program) for those that have not. A pass that changed nothing waits until another one changes the
/// program; when none is left the effort goes up (more variants), and when it is at its highest the run is finished.
#[allow(clippy::too_many_arguments)]
fn schedule(it: &dyn Oracle, opts: &Options, cur: &mut String, good: &mut String, pr: &mut Progress, log: &mut ReduceLog, hook: Hook, start: Instant, end: Instant) {
    let defs = passes();
    let mut stats: Vec<Stat> = vec![Stat::default(); defs.len()];
    let budget = end.saturating_duration_since(start);
    let slice = (budget / 12).clamp(Duration::from_secs(8), Duration::from_secs(240));
    let siblings_done = RefCell::new(HashSet::new());
    let mut version = 1u64;
    let mut level = effort();
    for _ in 0..800 {
        if interrupted() || Instant::now() >= end {
            break;
        }
        let lines = cur.lines().count();
        let elapsed = start.elapsed().as_secs();
        let eligible: Vec<usize> = (0..defs.len())
            .filter(|&i| {
                let d = &defs[i];
                (lines <= d.max_lines || d.big_after_secs.is_some_and(|s| elapsed >= s))
                    && (!d.needs_rustc || !opts.rustc.is_empty())
                    && level >= d.min_effort
                    && stats[i].idle_at != version
            })
            .collect();
        if eligible.is_empty() {
            if level >= 3 {
                log.push("every pass has run on this program at every effort level: finished");
                break;
            }
            level += 1;
            set_effort(level);
            log.push(format!("no pass finds anything more: trying harder (effort {level})"));
            for s in stats.iter_mut() {
                s.idle_at = 0;
            }
            continue;
        }
        let score = |i: usize| if stats[i].runs == 0 { defs[i].prior * cur.len() as f64 } else { stats[i].rate };
        let pick = *eligible.iter().max_by(|&&a, &&b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
        let def = &defs[pick];
        let before = cur.clone();
        let pass_start = Instant::now();
        let runs_before = oracle_runs();
        pr.pass(def.name);
        let ctx = PassCtx { it, opts, deadline: (Instant::now() + slice).min(end), siblings_done: &siblings_done };
        let mut next = (def.run)(&ctx, cur);
        let secs = pass_start.elapsed().as_secs_f64().max(0.001);
        let st = &mut stats[pick];
        st.runs += 1;
        if next != *cur {
            // the pass says it found something: the program must still show the problem, checked from scratch
            let saved = std::mem::replace(cur, next.clone());
            if !guard(it, cur, good, log, def.name) {
                *cur = saved;
                st.idle_at = version;
                st.rate = 0.0;
                pr.result(log, def.name, cur);
                continue;
            }
            stage(it, hook, Stage::Flatten, &mut next, end, log);
            if next != *cur {
                *cur = next;
                *good = cur.clone();
            }
            st.rate = 0.5 * st.rate + 0.5 * (before.len() as f64 - cur.len() as f64) / secs;
            version += 1;
        } else {
            st.rate *= 0.5;
            st.idle_at = version;
        }
        let _ = runs_before;
        pr.result(log, def.name, cur);
    }
}

pub fn minimize(it: &dyn Oracle, src: &str, opts: &Options, log: &mut ReduceLog, hook: Hook) -> String {
    reset_deadline_flag();
    crate::control::start_clock();
    let start = Instant::now();
    let budget = opts.budget;
    let mut cur = src.to_string();
    let mut good = src.to_string();
    crate::control::set_current_size(src.lines().count(), src.len());
    crate::control::require_nesting(src);
    let mut pr = Progress { start, lines: cur.lines().count(), bytes: cur.len(), n: 0, last_runs: oracle_runs(), last_at: start, initial: (cur.lines().count(), cur.len()), runs0: oracle_runs(), gains: Vec::new() };
    info!("system: {}", crate::control::describe_system());
    info!("compiler for the passes that compile: {}", if opts.rustc.is_empty() { "none (they are off)".to_string() } else { opts.rustc.join(" ") });
    info!("reduction starts: {} lines, {} bytes, budget {}s, {} oracle calls at once", pr.lines, pr.bytes, budget.as_secs(), par_checks());
    // a bug that shows in some runs only (addresses, timing, load) makes every verdict a coin toss: say so before spending hours
    let stable = (0..3).filter(|_| it.recheck(&cur)).count();
    if stable == 3 {
        info!("the starting program is interesting in 3 of 3 runs");
    } else {
        warn!("the starting program is interesting in only {stable} of 3 runs - the problem is flaky or the setup differs from the one that found it. {}", it.explain(&cur));
        log.push(format!("the starting program was interesting in {stable} of 3 runs"));
    }
    info!("plan: the pass that removed the most per second goes next (bigger programs: deletions of whole items and statements; smaller ones: expressions, tokens)");

    pr.banner("passes, best yield first");
    schedule(it, opts, &mut cur, &mut good, &mut pr, log, hook, start, start + budget * 19 / 20);

    if interrupted() {
        log.push("interrupted, final polish skipped");
        pr.summary(log, false);
        return cur;
    }
    pr.banner("final polish: parentheses, caller's cosmetics, short names, rustfmt");
    pr.pass("parentheses");
    cur = strip_parens(it, &cur);
    stage(it, hook, Stage::Polish, &mut cur, start + budget, log);
    // a run that found nothing smaller than the input hands the input back (formatting or growing passes must not make it worse)
    if (cur.lines().count(), cur.len()) >= (src.lines().count(), src.len()) && it.recheck(src) {
        log.push("nothing smaller than the input was found: the input is kept as it is");
        cur = src.to_string();
        pr.result(log, "polish", &cur);
        pr.summary(log, true);
        return cur;
    }
    if cur.lines().count() <= 200 {
        pr.pass("short names");
        cur = shorten_names(it, &cur, start + budget);
        pr.result(log, "declared names replaced by single letters", &cur);
    }
    pr.pass("rustfmt");
    let plain = cur.clone();
    cur = format_if_interesting(it, &cur);
    // rustfmt spreads long lines: fine for a short program, not for one that is still big
    if cur.lines().count() > 60 && cur.lines().count() > plain.lines().count() * 2 {
        cur = plain;
    }
    pr.result(log, "polish", &cur);
    guard(it, &mut cur, &mut good, log, "the final polish");
    pr.summary(log, true);
    cur
}

/// True when every stage of the last `minimize` ended before its deadline.
pub fn complete() -> bool {
    last_run_complete()
}
