use crate::control::{expired, par_checks};
use crate::oracle::Oracle;
use crate::par::par_map;
use log::info;
use std::time::Instant;

/// Drop chunks of lines, halving the chunk size (ddmin); `par_checks()` neighbouring chunks are tried at the same time.
pub fn shrink_lines(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    let mut n = (lines.len() / 2).max(1);
    let width = par_checks();
    while n >= 1 {
        let mut i = 0;
        while i < lines.len() {
            if expired(deadline) {
                return lines.join("\n");
            }
            // chunks i, i+n, i+2n, ... are tried at the same time; the first one that works is dropped
            let starts: Vec<usize> = (0..width).map(|k| i + k * n).take_while(|&s| s < lines.len()).collect();
            let cands: Vec<String> = starts.iter().map(|&s| lines[..s].iter().chain(lines[(s + n).min(lines.len())..].iter()).cloned().collect::<Vec<_>>().join("\n")).collect();
            let res = par_map(&cands, width, |c| !c.is_empty() && it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let s = starts[p];
                    lines.drain(s..(s + n).min(lines.len()));
                    i = s;
                }
                None => i += width * n,
            }
        }
        if n == 1 {
            break;
        }
        n /= 2;
    }
    lines.join("\n")
}

/// Comments and blank lines: they never matter to a reducer, so all of them go in one step.
pub fn strip_comments(it: &dyn Oracle, src: &str) -> String {
    let cand: Vec<String> = src.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//")).map(|l| l.to_string()).collect();
    let cand = cand.join("\n") + "\n";
    if cand != src && it.check(&cand) {
        info!("comments and blank lines removed ({} -> {} lines)", src.lines().count(), cand.lines().count());
        cand
    } else {
        src.to_string()
    }
}
