//! Unused items and imports as the compiler sees them (`--force-warn`: a crate-level `#![allow(dead_code)]`, as generated programs have, does not hide them): one `rustc` run lists every `dead_code` / `unused_imports` warning, the
//! items they point at are deleted (all at once, else one at a time) and the run is repeated while it finds more.

use crate::control::{expired, interrupted, par_checks};
use crate::diag::compile_diags;
use crate::items::{Item, item_ranges};
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::scan::is_ident;
use log::info;
use std::time::Instant;

const KINDS: [&str; 13] =
    ["function", "associated function", "method", "struct", "enum", "trait", "constant", "associated constant", "static", "type alias", "union", "unused import", "unused imports"];

/// Positions of the warnings that name an item the compiler considers unused.
fn unused_spots(rustc: &[String], work: &std::path::Path, src: &str) -> Vec<usize> {
    let Some(diags) = compile_diags(rustc, work, src, &["--force-warn", "dead_code", "--force-warn", "unused_imports"]) else { return Vec::new() };
    diags
        .iter()
        .filter(|d| !d.error)
        .filter(|d| {
            let msg = d.msg.as_str();
            let unused = msg.contains("is never used") || msg.contains("are never used") || msg.contains("is never constructed") || msg.starts_with("unused import");
            unused && KINDS.iter().any(|k| msg.starts_with(k))
        })
        .filter_map(|d| d.offset(src))
        .collect()
}

/// The innermost item whose header contains `offset`.
fn item_at(items: &[Item], offset: usize) -> Option<&Item> {
    items.iter().filter(|i| i.start <= offset && offset <= i.header_end).max_by_key(|i| i.start)
}

fn whole_word(text: &str, word: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices(word).any(|(i, _)| (i == 0 || !is_ident(b[i - 1])) && b.get(i + word.len()).is_none_or(|&c| !is_ident(c)))
}

/// An unused type or trait is only removable together with the `impl` blocks that mention it.
fn with_dependents(src: &str, items: &[Item], item: &Item) -> Vec<(usize, usize)> {
    let mut group = vec![(item.start, item.end)];
    let header = &src[item.start..item.header_end];
    let mut words = header.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty());
    let name = words.by_ref().skip_while(|w| !matches!(*w, "struct" | "enum" | "union" | "trait")).nth(1);
    if let Some(name) = name {
        for o in items {
            let h = src[o.start..o.header_end].trim_start();
            if o.start != item.start && (h.starts_with("impl") || h.starts_with("unsafe impl")) && whole_word(h, name) && !(o.start >= item.start && o.end <= item.end) {
                group.push((o.start, o.end));
            }
        }
    }
    group
}

fn without(src: &str, ranges: &[(usize, usize)]) -> String {
    let mut sorted: Vec<(usize, usize)> = ranges.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut out = src.to_string();
    let mut upto = usize::MAX;
    for &(s, e) in sorted.iter().rev() {
        if e > upto {
            continue; // nested in a range that is already gone
        }
        out.replace_range(s..e, "");
        upto = s;
    }
    out
}

/// `rustc` is the compiler command (program first, e.g. `["rustc", "+nightly"]`).
pub fn remove_dead_code(it: &dyn Oracle, src: &str, rustc: &[String], deadline: Instant) -> String {
    let mut cur = src.to_string();
    for round in 1..=6 {
        if expired(deadline) || interrupted() {
            break;
        }
        let items = item_ranges(&cur);
        let mut groups: Vec<Vec<(usize, usize)>> = unused_spots(rustc, &it.work_dir(), &cur)
            .into_iter()
            .filter_map(|o| item_at(&items, o).map(|i| with_dependents(&cur, &items, i)))
            .collect();
        groups.sort();
        groups.dedup();
        if groups.is_empty() {
            break;
        }
        let flat = |gs: &[&Vec<(usize, usize)>]| -> Vec<(usize, usize)> { gs.iter().flat_map(|g| g.iter().copied()).collect() };
        let all = without(&cur, &flat(&groups.iter().collect::<Vec<_>>()));
        if it.check(&all) {
            info!("dead code, round {round}: removed {} unused item(s) at once ({} -> {} lines)", groups.len(), cur.lines().count(), all.lines().count());
            cur = all;
            continue;
        }
        let cands: Vec<String> = groups.iter().map(|g| without(&cur, g)).collect();
        let res = par_map(&cands, par_checks(), |c| it.check(c));
        let kept: Vec<&Vec<(usize, usize)>> = groups.iter().zip(&res).filter(|(_, ok)| **ok).map(|(g, _)| g).collect();
        if kept.is_empty() {
            break;
        }
        // each removal alone is fine; together they may not be: apply them greedily
        let mut next = cur.clone();
        let mut applied: Vec<&Vec<(usize, usize)>> = Vec::new();
        for g in kept.iter().rev() {
            let mut with = applied.clone();
            with.push(g);
            let cand = without(&cur, &flat(&with));
            if it.check(&cand) {
                applied = with;
                next = cand;
            }
        }
        if applied.is_empty() {
            break;
        }
        info!("dead code, round {round}: removed {} of {} unused item(s)", applied.len(), groups.len());
        cur = next;
    }
    cur
}
