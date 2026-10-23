//! Deletions that each worked on their own, tried together (Shrink Ray's patch applier does the same): when most candidates of a
//! batch succeed, one more oracle call replaces many rounds.

use crate::oracle::Oracle;

/// `winners` are indices of candidates that were accepted individually. `build` makes the text for a subset of them (or `None` when
/// they cannot be combined, e.g. overlapping ranges). The whole set is tried first, then its first half; the result is the subset that
/// was accepted together with its text. `None` when neither works (the caller falls back to the first winner).
pub fn merge_winners(it: &dyn Oracle, winners: &[usize], build: impl Fn(&[usize]) -> Option<String>) -> Option<(Vec<usize>, String)> {
    if winners.len() < 2 {
        return None;
    }
    for take in [winners.len(), winners.len() / 2] {
        if take < 2 {
            break;
        }
        let subset = &winners[..take];
        if let Some(text) = build(subset) {
            if !text.trim().is_empty() && it.check(&text) {
                return Some((subset.to_vec(), text));
            }
        }
    }
    None
}

/// The union of ranges as disjoint, sorted ranges; `None` if two of them overlap partially (a candidate that lies inside another is
/// fine, the outer one covers it).
pub fn disjoint_union(sets: &[&[(usize, usize)]]) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = sets.iter().flat_map(|s| s.iter().copied()).collect();
    all.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for r in all {
        match out.last_mut() {
            Some(l) if r.0 < l.1 => l.1 = l.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// `src` without the given disjoint, sorted ranges.
pub fn without_ranges(src: &str, ranges: &[(usize, usize)]) -> String {
    let mut out = src.to_string();
    for &(s, e) in ranges.iter().rev() {
        out.replace_range(s..e, "");
    }
    out
}
