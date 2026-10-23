//! A dashboard for a reduction that runs in a terminal (not interactive: Ctrl+C is the only key): size now and at the start, the size
//! over time as a graph, the passes that ran last, and the last reduction as a diff. Drawn in ASCII with ANSI colours on the alternate
//! screen; the log goes to the log file meanwhile. `--ui basic` (and every run without a terminal, like CI) keeps the plain log.

use crate::control;
use std::collections::{HashMap, VecDeque};
use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Plain,
    Bold,
    Dim,
    Cyan,
    Red,
    Green,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    pub style: Style,
}

fn line(text: impl Into<String>, style: Style) -> Line {
    Line { text: text.into(), style }
}

#[derive(Clone)]
pub struct PassLine {
    pub name: String,
    pub before: usize,
    pub after: usize,
    pub calls: usize,
    pub secs: f32,
}

#[derive(Default)]
pub struct Dash {
    pub title: String,
    pub budget_secs: u64,
    pub initial: (usize, usize),
    pub log_file: String,
    pub out_file: String,
    /// `(seconds since the start, bytes)`
    pub samples: Vec<(f32, usize)>,
    pub passes: VecDeque<PassLine>,
}

/// What changes from frame to frame and is read from the run itself.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub elapsed: f32,
    pub lines: usize,
    pub bytes: usize,
    pub oracle_runs: usize,
    pub pass: String,
    pub stopping: bool,
    pub change: Option<(String, String)>,
}

static DASH: Mutex<Option<Dash>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static ON_SCREEN: AtomicBool = AtomicBool::new(false);

/// Should the dashboard be shown? `mode`: `auto` (a terminal), `tui` (always) or `basic` (never).
pub fn wanted(mode: &str) -> bool {
    match mode {
        "tui" => true,
        "basic" => false,
        _ => std::io::stdout().is_terminal() && std::io::stderr().is_terminal(),
    }
}

fn kb(bytes: usize) -> String {
    if bytes >= 10_000 { format!("{:.1} KB", bytes as f64 / 1000.0) } else { format!("{bytes} B") }
}

fn clock(secs: f32) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Lines of `before` that are gone and lines of `after` that are new, in text order (a line that moved counts as neither).
pub fn diff_lines(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let mut count: HashMap<&str, i64> = HashMap::new();
    for l in before.lines() {
        *count.entry(l).or_insert(0) += 1;
    }
    for l in after.lines() {
        *count.entry(l).or_insert(0) -= 1;
    }
    let mut left = count.clone();
    let removed: Vec<String> = before
        .lines()
        .filter(|l| {
            let c = left.get_mut(l).unwrap();
            if *c > 0 {
                *c -= 1;
                true
            } else {
                false
            }
        })
        .map(String::from)
        .collect();
    let mut left = count;
    let added: Vec<String> = after
        .lines()
        .filter(|l| {
            let c = left.get_mut(l).unwrap();
            if *c < 0 {
                *c += 1;
                true
            } else {
                false
            }
        })
        .map(String::from)
        .collect();
    (removed, added)
}

fn fit(text: &str, width: usize) -> String {
    let t: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if t.chars().count() <= width {
        t
    } else {
        let mut cut: String = t.chars().take(width.saturating_sub(3)).collect();
        cut.push_str("...");
        cut
    }
}

fn boxed(title: &str, width: usize) -> Line {
    let head = format!("+- {title} ");
    let fill = width.saturating_sub(head.chars().count() + 1);
    line(format!("{head}{}+", "-".repeat(fill)), Style::Cyan)
}

fn row(text: &str, width: usize, style: Style) -> Line {
    let inner = width.saturating_sub(4);
    let t = fit(text, inner);
    line(format!("| {t}{} |", " ".repeat(inner.saturating_sub(t.chars().count()))), style)
}

/// The size-over-time graph: one column per slice of the run, the height of the bar is the size on a log scale.
pub fn graph(samples: &[(f32, usize)], span_secs: f32, cols: usize, rows: usize) -> Vec<String> {
    let mut out = vec![" ".repeat(cols); rows];
    if samples.is_empty() || cols == 0 || rows == 0 {
        return out;
    }
    let max = samples.iter().map(|s| s.1).max().unwrap_or(1).max(2) as f64;
    let min = samples.iter().map(|s| s.1).min().unwrap_or(1).max(1) as f64;
    let (lo, hi) = (min.ln(), max.ln().max(min.ln() + 0.1));
    let mut last = samples[0].1;
    let mut k = 0;
    for c in 0..cols {
        let t = span_secs * (c as f32 + 1.0) / cols as f32;
        while k < samples.len() && samples[k].0 <= t {
            last = samples[k].1;
            k += 1;
        }
        if t > samples.last().map(|s| s.0).unwrap_or(0.0) + span_secs / cols as f32 {
            break;
        }
        let h = (((last.max(1) as f64).ln() - lo) / (hi - lo) * (rows as f64 - 1.0)).round() as usize + 1;
        for r in 0..h.min(rows) {
            let row = &mut out[rows - 1 - r];
            row.replace_range(c..c + 1, "#");
        }
    }
    out
}

/// The whole frame for a terminal of `width` x `height` characters.
pub fn render(d: &Dash, s: &Snapshot, width: usize, height: usize) -> Vec<Line> {
    let width = width.clamp(50, 200);
    let height = height.max(20);
    let mut out: Vec<Line> = Vec::new();
    let remaining = (d.budget_secs as f32 - s.elapsed).max(0.0);
    out.push(line(fit(&format!("rs-minimizer  {}   elapsed {}  of {}  ({} left)", d.title, clock(s.elapsed), clock(d.budget_secs as f32), clock(remaining)), width), Style::Bold));
    // size
    let pct = |a: usize, b: usize| if a == 0 { 0.0 } else { 100.0 * (a as f64 - b as f64) / a as f64 };
    out.push(boxed("size", width));
    out.push(row(&format!("now    {} lines, {}    ({:.1}% fewer lines, {:.1}% fewer bytes than the start)", s.lines, kb(s.bytes), pct(d.initial.0, s.lines), pct(d.initial.1, s.bytes)), width, Style::Plain));
    out.push(row(&format!("start  {} lines, {}", d.initial.0, kb(d.initial.1)), width, Style::Dim));
    let rate = s.oracle_runs as f32 / s.elapsed.max(0.1);
    out.push(row(&format!("oracle calls {} ({:.1}/s)    now running: {}{}", s.oracle_runs, rate, if s.pass.is_empty() { "-" } else { s.pass.trim_start_matches("rs-minimizer: ") }, if s.stopping { "    STOPPING: finishing the running calls" } else { "" }), width, if s.stopping { Style::Red } else { Style::Plain }));
    // graph
    let gh = ((height.saturating_sub(12)) / 3).clamp(5, 14);
    out.push(boxed("size over time (bytes, log scale)", width));
    let cols = width - 14;
    let span = s.elapsed.max(1.0);
    let top = d.samples.iter().map(|x| x.1).max().unwrap_or(0);
    let bottom = d.samples.iter().map(|x| x.1).min().unwrap_or(0);
    for (i, g) in graph(&d.samples, span, cols, gh).into_iter().enumerate() {
        let label = if i == 0 { kb(top) } else if i == gh - 1 { kb(bottom) } else { String::new() };
        out.push(line(format!("| {label:>8} |{g}|"), Style::Green));
    }
    out.push(line(format!("|          0s{:>w$}|", clock(span), w = cols + 1), Style::Dim));
    // passes
    let passes: Vec<&PassLine> = d.passes.iter().rev().take(7).collect();
    out.push(boxed("last passes", width));
    for p in &passes {
        let delta = p.before as i64 - p.after as i64;
        let text = format!("{:<28} {:>10}  {:>6} calls  {:>6.1}s", p.name.trim_start_matches("rs-minimizer: "), if delta > 0 { format!("-{}", kb(delta as usize)) } else if delta < 0 { format!("+{}", kb((-delta) as usize)) } else { "no change".to_string() }, p.calls, p.secs);
        out.push(row(&text, width, if delta > 0 { Style::Green } else if delta < 0 { Style::Red } else { Style::Dim }));
    }
    for _ in passes.len()..3 {
        out.push(row("", width, Style::Plain));
    }
    // diff of the last change
    let used = out.len() + 2;
    let free = height.saturating_sub(used).max(4);
    out.push(boxed("last reduction (diff of the last accepted change)", width));
    match &s.change {
        None => out.push(row("(nothing yet)", width, Style::Dim)),
        Some((before, after)) => {
            let (removed, added) = diff_lines(before, after);
            out.push(row(&format!("{} lines removed, {} lines added", removed.len(), added.len()), width, Style::Dim));
            let room = free.saturating_sub(2);
            let show_added = if added.is_empty() { 0 } else { (room / 3).max(1).min(added.len()) };
            let show_removed = room.saturating_sub(show_added).min(removed.len());
            for l in removed.iter().take(show_removed) {
                out.push(row(&format!("- {}", l.trim()), width, Style::Red));
            }
            if removed.len() > show_removed {
                out.push(row(&format!("  ... {} more removed lines", removed.len() - show_removed), width, Style::Dim));
            }
            for l in added.iter().take(show_added) {
                out.push(row(&format!("+ {}", l.trim()), width, Style::Green));
            }
        }
    }
    out.push(line(format!("+{}+", "-".repeat(width - 2)), Style::Cyan));
    out.push(line(fit(&format!("Ctrl+C: stop and keep the best program ({}); twice: leave at once.   log: {}", d.out_file, d.log_file), width), Style::Dim));
    out.truncate(height);
    out
}

fn terminal_size() -> (usize, usize) {
    let tty = std::fs::File::open("/dev/tty");
    let out = std::process::Command::new("stty").arg("size").stdin(tty.map(std::process::Stdio::from).unwrap_or(std::process::Stdio::null())).output();
    if let Ok(o) = out {
        let t = String::from_utf8_lossy(&o.stdout).to_string();
        let mut it = t.split_whitespace().filter_map(|v| v.parse::<usize>().ok());
        if let (Some(rows), Some(cols)) = (it.next(), it.next()) {
            return (cols, rows);
        }
    }
    (100, 40)
}

fn paint(frame: &[Line], width: usize) -> String {
    let mut s = String::from("\x1b[H");
    for l in frame {
        let code = match l.style {
            Style::Plain => "",
            Style::Bold => "\x1b[1m",
            Style::Dim => "\x1b[2m",
            Style::Cyan => "\x1b[36m",
            Style::Red => "\x1b[31m",
            Style::Green => "\x1b[32m",
        };
        let t = fit(&l.text, width);
        s.push_str(&format!("{code}{t}\x1b[0m\x1b[K\n"));
    }
    s.push_str("\x1b[J");
    s
}

/// Shows the dashboard until `stop`. The caller has set up the checkpoint file (the diff comes from the accepted changes).
pub fn start(title: &str, budget: Duration, initial: (usize, usize), out_file: &str, log_file: &str) {
    control::set_ui(true);
    *DASH.lock().unwrap() = Some(Dash { title: title.to_string(), budget_secs: budget.as_secs(), initial, log_file: log_file.to_string(), out_file: out_file.to_string(), samples: vec![(0.0, initial.1)], passes: VecDeque::new() });
    ACTIVE.store(true, Ordering::SeqCst);
    ON_SCREEN.store(true, Ordering::SeqCst);
    let started = Instant::now();
    print!("\x1b[?1049h\x1b[?25l\x1b[2J");
    let _ = std::io::stdout().flush();
    let handle = std::thread::spawn(move || {
        let mut size = terminal_size();
        let mut last_size = Instant::now();
        while ACTIVE.load(Ordering::SeqCst) {
            if last_size.elapsed() > Duration::from_secs(3) {
                size = terminal_size();
                last_size = Instant::now();
            }
            let (lines, bytes) = control::current_size();
            let snap = Snapshot { elapsed: started.elapsed().as_secs_f32(), lines, bytes, oracle_runs: control::oracle_runs(), pass: control::current_pass(), stopping: control::interrupted(), change: control::last_change() };
            let frame = {
                let mut g = DASH.lock().unwrap();
                let Some(d) = g.as_mut() else { break };
                if bytes > 0 && d.samples.last().is_none_or(|l| l.1 != bytes || snap.elapsed - l.0 > 5.0) {
                    d.samples.push((snap.elapsed, bytes));
                }
                render(d, &snap, size.0, size.1)
            };
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(paint(&frame, size.0.clamp(50, 200)).as_bytes());
            let _ = out.flush();
            drop(out);
            std::thread::sleep(Duration::from_millis(400));
        }
    });
    *THREAD.lock().unwrap() = Some(handle);
}

/// A finished pass, for the list of the last passes.
pub fn pass_result(name: &str, before: usize, after: usize, calls: usize, secs: f32) {
    if let Some(d) = DASH.lock().unwrap().as_mut() {
        d.passes.push_back(PassLine { name: name.to_string(), before, after, calls, secs });
        while d.passes.len() > 30 {
            d.passes.pop_front();
        }
    }
}

/// Leaves the alternate screen and shows the cursor again (also called when the second Ctrl+C ends the process).
pub fn restore_terminal() {
    if ON_SCREEN.swap(false, Ordering::SeqCst) {
        print!("\x1b[?25h\x1b[?1049l");
        let _ = std::io::stdout().flush();
    }
}

/// Ends the dashboard and prints `final_lines` on the normal screen.
pub fn stop(final_lines: &[String]) {
    ACTIVE.store(false, Ordering::SeqCst);
    if let Some(h) = THREAD.lock().unwrap().take() {
        let _ = h.join();
    }
    restore_terminal();
    control::set_ui(false);
    for l in final_lines {
        println!("{l}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diff_lists_removed_and_added_lines_and_ignores_the_unchanged() {
        let (removed, added) = diff_lines("a\nb\nc\nc\n", "a\nc\nd\n");
        assert_eq!(removed, vec!["b", "c"]);
        assert_eq!(added, vec!["d"]);
        assert_eq!(diff_lines("x\n", "x\n"), (vec![], vec![]));
    }

    #[test]
    fn the_graph_has_the_requested_shape_and_falls_as_the_size_does() {
        let rows = graph(&[(0.0, 100_000), (5.0, 1_000), (10.0, 10)], 10.0, 20, 6);
        assert_eq!(rows.len(), 6);
        assert!(rows.iter().all(|r| r.chars().count() == 20));
        let height = |col: usize| rows.iter().filter(|r| r.as_bytes()[col] == b'#').count();
        assert!(height(0) > height(19), "{rows:?}");
        assert_eq!(graph(&[], 10.0, 5, 3), vec!["     "; 3]);
    }
}
