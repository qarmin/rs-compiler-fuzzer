//! The default reduction pipeline. Callers with knowledge of how their programs were produced plug in through `Hook`.

use crate::control::{oracle_runs, expired, interrupted, last_run_complete, par_checks, reset_deadline_flag, set_stage};
use crate::deadcode::remove_dead_code;
use crate::exprs::{drop_calls, drop_let_bindings, flatten_blocks, simplify_exprs, simplify_let_tuples, simplify_lets, strip_parens};
use crate::external::{shrinkray, shrinkray_available, treereduce};
use crate::format::format_if_interesting;
use crate::inline::{inline_calls, substitute_lets};
use crate::items::{shrink_items, shrink_items_with_users, shrink_siblings, shrink_statements};
use crate::lines::{shrink_lines, strip_comments};
use crate::modules::hoist_modules;
use crate::oracle::Oracle;
use crate::peep::peep;
use crate::structure::{remove_fields, remove_params, remove_variants_and_arms, stub_bodies, unwrap_newtypes};
use crate::tokens::shrink_tokens;
use crate::unroll::unroll;
use log::{info, warn};
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
    /// macros expanded, `catch_unwind` removed, modules hoisted
    Unrolled,
    /// before the first deletion passes
    BeforeLines,
    AfterLines,
    /// after Shrink Ray / treereduce (or the built-in passes): the program is small and often renamed
    AfterExternal,
    /// last step before formatting
    Polish,
}

/// Called at every `Stage` with the current program; returns a replacement (already checked by the hook) or `None`.
pub type Hook<'a> = &'a mut dyn FnMut(Stage, &str, Instant) -> Option<String>;

pub struct Options {
    pub budget: Duration,
    /// compiler command used to find unused items (empty switches the dead-code pass off)
    pub rustc: Vec<String>,
    pub use_shrinkray: bool,
    pub use_treereduce: bool,
    /// top-level modules that must stay modules (the caller handles them itself)
    pub keep_module: Box<dyn Fn(&str) -> bool + Sync>,
}

impl Options {
    pub fn new(budget: Duration) -> Self {
        Options { budget, rustc: vec!["rustc".into()], use_shrinkray: true, use_treereduce: true, keep_module: Box::new(|_| false) }
    }
}

fn stage(hook: Hook, st: Stage, cur: &mut String, deadline: Instant, log: &mut ReduceLog) {
    if let Some(next) = hook(st, cur, deadline) {
        if next != *cur {
            log.push(format!("{st:?}: the hook changed the program ({} -> {} lines)", cur.lines().count(), next.lines().count()));
            *cur = next;
        }
    }
}

const STEPS: usize = 10;

/// Above this many lines only the cheap passes (deletions) run; the passes that grow or probe the program wait until it is smaller.
const BIG_LINES: usize = 2000;
/// The peephole pass tries every identifier, call and group once: it is for small programs.
const PEEP_LINES: usize = 800;
/// The cheap phase ends after this long even when the program is still big.
const CHEAP_SECS: u64 = 30;

/// Step banners and one result line per pass (which pass, how much it removed), so a long run can be followed at a glance.
struct Progress {
    start: Instant,
    lines: usize,
    bytes: usize,
    n: usize,
    /// oracle calls and time at the previous RESULT line: what a pass cost
    last_runs: usize,
    last_at: Instant,
}

impl Progress {
    fn step(&mut self, name: &str) {
        self.n += 1;
        info!("==================== STEP {}/{STEPS}: {name} ====================", self.n);
    }

    /// Own passes are shown as `rs-minimizer: name`, the external tools by their own name.
    fn label(name: &str) -> String {
        if name.starts_with("shrinkray") || name.starts_with("treereduce") { name.to_string() } else { format!("rs-minimizer: {name}") }
    }

    fn pass(&self, name: &str) {
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
        self.last_runs = runs;
        self.last_at = Instant::now();
        self.lines = l;
        self.bytes = b;
    }
}

/// Shrink Ray, or the built-in expression passes when it is not installed. `until`: deadlines of the two kinds of work.
fn external_round(it: &dyn Oracle, mut cur: String, opts: &Options, pr: &mut Progress, log: &mut ReduceLog, until: (Instant, Instant)) -> String {
    if opts.use_shrinkray && shrinkray_available() {
        pr.pass("shrinkray");
        cur = shrinkray(it, &cur, until.0, log);
        pr.result(log, "shrinkray", &cur);
        return cur;
    }
    if opts.use_shrinkray {
        warn!("shrinkray not found - using the built-in passes only");
        log.push("shrinkray not installed (uv tool install shrinkray) - using the built-in passes only");
    }
    for round in 1..=4 {
        let before = cur.clone();
        pr.pass("let simplification");
        cur = simplify_lets(it, &cur, until.0);
        pr.pass("method call dropping");
        cur = drop_calls(it, &cur, until.0);
        pr.pass("expression simplification");
        cur = simplify_exprs(it, &cur, until.1);
        pr.pass("lines");
        cur = shrink_lines(it, &cur, until.1);
        pr.result(log, &format!("built-in expression passes, round {round}"), &cur);
        if cur == before || expired(until.1) {
            break;
        }
    }
    cur
}

/// Reductions of the program's structure: bodies stubbed, wrappers unwrapped, unused parameters, fields, variants and arms.
fn structure_passes(it: &dyn Oracle, mut cur: String, pr: &mut Progress, log: &mut ReduceLog, until: Instant) -> String {
    pr.pass("newtypes");
    cur = unwrap_newtypes(it, &cur, until);
    pr.result(log, "newtype structs replaced by their field", &cur);
    pr.pass("function bodies");
    cur = stub_bodies(it, &cur, until);
    pr.result(log, "function bodies replaced by `unimplemented!()`", &cur);
    pr.pass("parameters");
    cur = remove_params(it, &cur, until);
    pr.result(log, "unused parameters and their arguments", &cur);
    pr.pass("fields");
    cur = remove_fields(it, &cur, until);
    pr.result(log, "unused struct fields", &cur);
    pr.pass("variants and arms");
    cur = remove_variants_and_arms(it, &cur, until);
    pr.result(log, "enum variants and match arms", &cur);
    cur
}

/// Inlining of calls and the cleanups it makes possible, repeated while something changes. The program may grow in here; only the
/// passes that follow check the size again.
fn flatten_and_clean(it: &dyn Oracle, mut cur: String, opts: &Options, pr: &mut Progress, log: &mut ReduceLog, until: Instant) -> String {
    for round in 1..=3 {
        let before = cur.clone();
        pr.pass("inline");
        cur = inline_calls(it, &cur, &opts.rustc, until);
        pr.result(log, &format!("inline calls (round {round})"), &cur);
        pr.pass("newtypes");
        cur = unwrap_newtypes(it, &cur, until);
        pr.result(log, "newtype structs replaced by their field", &cur);
        pr.pass("parameters");
        cur = remove_params(it, &cur, until);
        pr.result(log, "unused parameters and their arguments", &cur);
        pr.pass("let bindings");
        cur = drop_let_bindings(it, &cur, until);
        pr.result(log, "unused let bindings dropped (the initialiser stays)", &cur);
        pr.pass("blocks");
        cur = flatten_blocks(it, &cur, until);
        pr.result(log, "blocks spliced into their parents", &cur);
        pr.pass("let tuples");
        cur = simplify_let_tuples(it, &cur, until);
        pr.result(log, "unused names of tuple lets", &cur);
        pr.pass("let substitution");
        cur = substitute_lets(it, &cur, until);
        pr.result(log, "single-use lets substituted", &cur);
        pr.pass("statements");
        cur = shrink_statements(it, &cur, until);
        pr.result(log, "statements", &cur);
        pr.pass("items");
        cur = shrink_items(it, &cur, until);
        pr.result(log, "items (what the inlining left unused)", &cur);
        if cur == before || expired(until) {
            break;
        }
    }
    cur
}

/// The cheap deletions: the caller's batch step, comments, items with the code that mentions them, items, lines. Repeated while a
/// round still removes a tenth of the program, until the program is small or `until` passes.
fn cheap_phase(it: &dyn Oracle, mut cur: String, pr: &mut Progress, log: &mut ReduceLog, hook: Hook, until: Instant, soft: Instant) -> String {
    for round in 1..=4 {
        let before = cur.len();
        if round == 1 {
            stage(hook, Stage::BeforeLines, &mut cur, until, log);
            pr.result(log, "caller's batch step", &cur);
        }
        pr.pass("comments");
        cur = strip_comments(it, &cur);
        pr.result(log, "comments", &cur);
        pr.pass("items with their users");
        cur = shrink_items_with_users(it, &cur, until);
        pr.result(log, "items together with the code that mentions them", &cur);
        pr.pass("siblings");
        cur = shrink_siblings(it, &cur, until);
        pr.result(log, "siblings: chunks of neighbouring items and statements", &cur);
        pr.pass("items");
        cur = shrink_items(it, &cur, until);
        pr.result(log, "items", &cur);
        pr.pass("lines");
        cur = shrink_lines(it, &cur, until);
        pr.result(log, "lines", &cur);
        let gain = before.saturating_sub(cur.len());
        let small = cur.lines().count() <= BIG_LINES;
        info!("cheap round {round}: removed {:.0}% of the bytes, {} lines left{}", 100.0 * gain as f64 / before.max(1) as f64, cur.lines().count(), if small { " (small enough for the other passes)" } else { "" });
        if gain * 10 < before || Instant::now() >= soft || expired(until) || interrupted() {
            break;
        }
    }
    cur
}

pub fn minimize(it: &dyn Oracle, src: &str, opts: &Options, log: &mut ReduceLog, hook: Hook) -> String {
    reset_deadline_flag();
    crate::control::start_clock();
    let start = Instant::now();
    let budget = opts.budget;
    let at = |num: u32, den: u32| start + budget * num / den;
    let mut cur = src.to_string();
    let mut pr = Progress { start, lines: cur.lines().count(), bytes: cur.len(), n: 0, last_runs: oracle_runs(), last_at: start };
    info!("reduction starts: {} lines, {} bytes, budget {}s, {} oracle calls at once", pr.lines, pr.bytes, budget.as_secs(), par_checks());
    info!("plan: cheap deletions first, the passes that grow or probe the program only once it has fewer than {BIG_LINES} lines (or {CHEAP_SECS}s have passed)");

    pr.step("cheap deletions: comments, items, lines (nothing is expanded or inlined yet)");
    let soft = start + Duration::from_secs(CHEAP_SECS).min(budget / 8);
    cur = cheap_phase(it, cur, &mut pr, log, hook, at(2, 5), soft);

    pr.step("dead code reported by the compiler");
    if !opts.rustc.is_empty() && !interrupted() {
        pr.pass("dead code");
        cur = remove_dead_code(it, &cur, &opts.rustc, at(9, 20));
    }
    pr.result(log, "dead code", &cur);

    pr.step("unroll: expand macros, remove catch_unwind, hoist modules (the program may grow here, so it comes after the cheap deletions)");
    pr.pass("unroll");
    cur = unroll(it, &cur);
    pr.result(log, "unroll", &cur);
    pr.pass("hoist modules");
    cur = hoist_modules(it, &cur, &*opts.keep_module, &opts.rustc);
    pr.result(log, "hoist modules", &cur);
    stage(hook, Stage::Unrolled, &mut cur, at(1, 4), log);
    stage(hook, Stage::AfterLines, &mut cur, at(2, 5), log);
    pr.result(log, "caller's flatten step", &cur);

    pr.step("structure: newtypes, function bodies, parameters, fields, variants");
    if cur.lines().count() > BIG_LINES {
        log.push(format!("structure passes skipped: {} lines, they wait for fewer than {BIG_LINES}", cur.lines().count()));
    } else if !interrupted() {
        cur = structure_passes(it, cur, &mut pr, log, at(9, 20));
    }

    pr.step("shrinkray (or the built-in expression passes without it)");
    cur = external_round(it, cur, opts, &mut pr, log, (at(9, 20), at(9, 20)));

    pr.step("flatten again, inline calls and clean up (the program may grow here, size is checked only by the passes after it)");
    if !interrupted() {
        // external reducers rename and merge: what is left is often one case, or has modules again
        pr.pass("unroll");
        cur = unroll(it, &cur);
        pr.pass("hoist modules");
        cur = hoist_modules(it, &cur, &*opts.keep_module, &opts.rustc);
        pr.result(log, "unroll and hoist modules", &cur);
        stage(hook, Stage::AfterExternal, &mut cur, at(13, 20), log);
        pr.result(log, "caller's flatten step", &cur);
    }
    if cur.lines().count() > BIG_LINES {
        log.push(format!("inlining skipped: {} lines, it waits for fewer than {BIG_LINES}", cur.lines().count()));
    } else if !interrupted() && !opts.rustc.is_empty() {
        cur = flatten_and_clean(it, cur, opts, &mut pr, log, at(13, 20));
    }

    if !interrupted() {
        pr.step("shrinkray again, on the flattened program");
        cur = external_round(it, cur, opts, &mut pr, log, (at(16, 20), at(16, 20)));
    }

    pr.step("items, calls, expressions, peephole and tokens on the flattened program");
    cur = expensive_own_passes(it, cur, &mut pr, log, at(18, 20));

    if !interrupted() && opts.use_treereduce {
        pr.step("treereduce-rust, then our own passes once more (they have the last word)");
        pr.pass("treereduce-rust");
        let before = cur.clone();
        cur = treereduce(it, &cur, at(39, 40), par_checks(), 3);
        pr.result(log, "treereduce-rust", &cur);
        if cur != before {
            // the tree-based edits make new cleanups possible: our own passes have the last word
            if !opts.rustc.is_empty() && cur.lines().count() <= BIG_LINES {
                cur = flatten_and_clean(it, cur, opts, &mut pr, log, at(39, 40));
            }
            cur = expensive_own_passes(it, cur, &mut pr, log, at(39, 40));
        }
    }

    if interrupted() {
        log.push("interrupted, final polish skipped");
        return cur;
    }
    pr.step("final polish: parentheses, caller's cosmetics, rustfmt");
    pr.pass("parentheses");
    cur = strip_parens(it, &cur);
    stage(hook, Stage::Polish, &mut cur, start + budget, log);
    pr.pass("rustfmt");
    cur = format_if_interesting(it, &cur);
    pr.result(log, "polish", &cur);
    cur
}

/// Items, method calls, expressions, lines, then the peephole replacements and token windows (many oracle calls: small programs).
fn expensive_own_passes(it: &dyn Oracle, mut cur: String, pr: &mut Progress, log: &mut ReduceLog, until: Instant) -> String {
    pr.pass("items");
    cur = shrink_items(it, &cur, until);
    pr.result(log, "items", &cur);
    pr.pass("method call dropping");
    cur = drop_calls(it, &cur, until);
    pr.pass("expression simplification");
    cur = simplify_exprs(it, &cur, until);
    pr.pass("lines");
    cur = shrink_lines(it, &cur, until);
    pr.result(log, "calls, expressions, lines", &cur);
    if cur.lines().count() > PEEP_LINES {
        log.push(format!("peephole and token passes skipped: {} lines (they run below {PEEP_LINES})", cur.lines().count()));
        return cur;
    }
    pr.pass("peephole");
    cur = peep(it, &cur, until);
    pr.result(log, "peephole replacements (operands, values, zeros)", &cur);
    pr.pass("tokens");
    cur = shrink_tokens(it, &cur, until);
    pr.result(log, "tokens", &cur);
    cur
}

/// True when every stage of the last `minimize` ended before its deadline.
pub fn complete() -> bool {
    last_run_complete()
}
