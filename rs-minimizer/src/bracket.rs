//! Bracket and repetition passes taken over from Shrink Ray (`hollow`, `debracket`, `lift_braces`, `lexeme_based_deletions`): they know
//! nothing about Rust, only about balanced brackets and repeated token sequences, which is why they find what the structural passes
//! do not understand.

use crate::control::effort;
use crate::items::delete_groups;
use crate::oracle::Oracle;
use crate::tokens::tokenize;
use log::info;
use std::collections::HashMap;
use std::time::Instant;

type Group = Vec<(usize, usize)>;

const MAX_GROUPS: usize = 4000;

/// `(open, close)` byte positions of every balanced `()`, `[]` or `{}` pair, strings skipped.
fn bracket_pairs(src: &str) -> Vec<(usize, usize, u8)> {
    let b = src.as_bytes();
    let mut stack: Vec<(usize, u8)> = Vec::new();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => stack.push((i, b[i])),
            b')' | b']' | b'}' => {
                let want = match b[i] {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                if let Some(pos) = stack.iter().rposition(|&(_, o)| o == want) {
                    let (open, kind) = stack[pos];
                    stack.truncate(pos);
                    out.push((open, i, kind));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out.sort();
    out
}

/// String literals' contents `(start, end)`.
fn string_contents(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let s = i + 1;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            if i > s {
                out.push((s, i));
            }
        }
        i += 1;
    }
    out
}

fn run(it: &dyn Oracle, src: &str, mut groups: Vec<Group>, what: &str, deadline: Instant) -> String {
    groups.truncate(MAX_GROUPS);
    if groups.is_empty() {
        return src.to_string();
    }
    let n = groups.len();
    let out = delete_groups(it, src, groups, deadline);
    if out.len() < src.len() {
        info!("{what}: {n} candidates, {} -> {} bytes", src.len(), out.len());
    }
    out
}

/// The contents of `[]`, `{}`, `()` and strings emptied (smallest regions first): the skeleton of brackets stays.
pub fn hollow(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut regions: Vec<(usize, usize)> = string_contents(src);
    regions.extend(bracket_pairs(src).into_iter().filter(|&(o, c, _)| c > o + 1).map(|(o, c, _)| (o + 1, c)));
    regions.retain(|&(s, e)| e - s >= 4);
    regions.sort_by_key(|&(s, e)| (e - s, s));
    run(it, src, regions.into_iter().map(|r| vec![r]).collect(), "hollow", deadline)
}

/// A bracket pair removed, its contents kept (`(x + y)` -> `x + y`).
pub fn debracket(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let groups: Vec<Group> = bracket_pairs(src).into_iter().map(|(o, c, _)| vec![(o, o + 1), (c, c + 1)]).collect();
    run(it, src, groups, "debracket", deadline)
}

/// `{ a { b } c }` -> `{ b }`: the content of a block replaced by the content of a block inside it.
pub fn lift_braces(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let braces: Vec<(usize, usize)> = bracket_pairs(src).into_iter().filter(|p| p.2 == b'{').map(|(o, c, _)| (o, c)).collect();
    let mut groups: Vec<Group> = Vec::new();
    for &(po, pc) in &braces {
        // direct children: blocks inside that no other block inside `p` encloses
        let inside: Vec<&(usize, usize)> = braces.iter().filter(|&&(o, c)| o > po && c < pc).collect();
        for &&(co, cc) in &inside {
            if !inside.iter().any(|&&(o, c)| o < co && cc < c) {
                groups.push(vec![(po + 1, co + 1), (cc, pc)]);
            }
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.iter().map(|r| r.1 - r.0).sum::<usize>()));
    run(it, src, groups, "lift braces", deadline)
}

/// Regions between repeated token sequences (`let t3 = ...; h.put(&t3);` repeats): where the code repeats itself, the repeats are
/// likely independent units. Longer sequences and bigger regions first.
pub fn lexeme_deletions(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let toks = tokenize(src);
    if toks.len() < 20 || toks.len() > 40_000 {
        return src.to_string();
    }
    let mut occurrences: HashMap<&[(usize, usize)], Vec<usize>> = HashMap::new();
    let _ = &mut occurrences;
    // n-grams of 1..=4 tokens, positions of each
    let mut by_ngram: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for n in 1..=4usize {
        for i in 0..toks.len().saturating_sub(n) {
            let text = &src[toks[i].0..toks[i + n - 1].1];
            if text.len() < 2 || !text.chars().any(|c| c.is_alphanumeric()) {
                continue;
            }
            by_ngram.entry(format!("{n}:{text}")).or_default().push((toks[i].0, toks[i + n - 1].1));
        }
    }
    let mut intervals: Vec<((usize, usize), usize)> = Vec::new();
    for (key, pos) in &by_ngram {
        if pos.len() < 3 {
            continue;
        }
        let n: usize = key.split(':').next().and_then(|v| v.parse().ok()).unwrap_or(1);
        // non-overlapping occurrences
        let mut last_end = 0;
        let spaced: Vec<&(usize, usize)> = pos.iter().filter(|p| if p.0 >= last_end { last_end = p.1; true } else { false }).collect();
        for w in spaced.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            if b > a && b - a >= 16 {
                intervals.push(((a, b), n));
            }
        }
    }
    intervals.sort_by(|x, y| y.1.cmp(&x.1).then((y.0.1 - y.0.0).cmp(&(x.0.1 - x.0.0))).then(x.0.cmp(&y.0)));
    intervals.dedup_by_key(|i| i.0);
    if effort() > 0 {
        let mut v: Vec<_> = intervals.iter().map(|i| vec![i.0]).collect();
        crate::control::shuffled(&mut v, effort() as u64);
        return run(it, src, v, "lexeme deletions (shuffled)", deadline);
    }
    run(it, src, intervals.into_iter().map(|i| vec![i.0]).collect(), "lexeme deletions", deadline)
}
