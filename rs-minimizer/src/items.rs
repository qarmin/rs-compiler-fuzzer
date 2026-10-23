//! Whole items and attributes as deletion units (what treereduce/perses do on the syntax tree, here on balanced text): a
//! function, impl, struct, `use`, `mod`, ... goes in one step instead of line by line.

use crate::control::{expired, par_checks};
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::scan::{is_ident, matching, skip_ws};
use log::info;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub start: usize,
    pub end: usize,
    /// end of the header (up to the body's `{` or the closing `;`): spans of compiler warnings point into it
    pub header_end: usize,
    /// item keyword (`fn`, `struct`, `impl`, ...)
    pub kw: String,
    /// byte range of the declared name, for the keywords that have one
    pub name: Option<(usize, usize)>,
}

impl Item {
    pub fn name_in<'a>(&self, src: &'a str) -> Option<&'a str> {
        self.name.map(|(s, e)| &src[s..e])
    }

    pub fn contains(&self, other: &Item) -> bool {
        self.start <= other.start && other.end <= self.end && self.start != other.start
    }
}

/// Items directly inside `parent` (`None`: at the crate root), in textual order.
pub fn children<'a>(items: &'a [Item], parent: Option<&Item>) -> Vec<&'a Item> {
    items
        .iter()
        .filter(|i| {
            let innermost = items.iter().filter(|p| p.contains(i)).max_by_key(|p| p.start);
            match (innermost, parent) {
                (None, None) => true,
                (Some(a), Some(b)) => a.start == b.start,
                _ => false,
            }
        })
        .collect()
}

const KEYWORDS: [&str; 12] = ["fn", "struct", "enum", "impl", "trait", "mod", "use", "const", "static", "type", "union", "macro_rules"];
const PREFIXES: [&str; 5] = ["pub", "unsafe", "async", "extern", "default"];

fn skip_string(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() && b[i] != b'"' {
        if b[i] == b'\\' {
            i += 1;
        }
        i += 1;
    }
    i
}

/// Items at any nesting depth, as `start..end` ranges (start at the first prefix word, `pub`/`unsafe`/...; attributes are not
/// part of an item, see `attribute_ranges`).
pub fn item_ranges(src: &str) -> Vec<Item> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // the previous significant byte decides whether a word starts a statement or an item
    let mut prev = b';';
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'"' {
            i = skip_string(b, i) + 1;
            prev = b'"';
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < b.len() && is_ident(b[j]) {
                j += 1;
            }
            let word = &src[i..j];
            let item_position = matches!(prev, b';' | b'{' | b'}' | b']');
            if item_position && (KEYWORDS.contains(&word) || PREFIXES.contains(&word)) {
                if let Some(item) = parse_item(src, i) {
                    out.push(item);
                }
            }
            prev = b[j - 1];
            i = j;
            continue;
        }
        prev = c;
        i += 1;
    }
    out
}

fn parse_item(src: &str, start: usize) -> Option<Item> {
    let b = src.as_bytes();
    let mut i = start;
    // prefix words, then the item keyword
    let kw = loop {
        let w0 = skip_ws(b, i);
        let mut w1 = w0;
        while w1 < b.len() && is_ident(b[w1]) {
            w1 += 1;
        }
        let word = &src[w0..w1];
        if PREFIXES.contains(&word) {
            i = w1;
            // `pub(crate)`, `extern "C"`
            let n = skip_ws(b, i);
            if b.get(n) == Some(&b'(') {
                i = matching(b, n)? + 1;
            } else if b.get(n) == Some(&b'"') {
                i = skip_string(b, n) + 1;
            }
        } else if KEYWORDS.contains(&word) {
            i = w1;
            break word.to_string();
        } else {
            return None;
        }
    };
    // `const` / `unsafe` / `async` blocks and closures are not items
    let n = skip_ws(b, i);
    if matches!(kw.as_str(), "const" | "static") && matches!(b.get(n), Some(b'{') | Some(b'|')) {
        return None;
    }
    let name = if matches!(kw.as_str(), "fn" | "struct" | "enum" | "trait" | "mod" | "const" | "static" | "type" | "union") {
        let n = skip_ws(b, i);
        let mut e = n;
        while e < b.len() && is_ident(b[e]) {
            e += 1;
        }
        (e > n).then_some((n, e))
    } else {
        None
    };
    // header: up to the first `{` or `;` outside (), []
    let mut depth = 0i32;
    let mut k = i;
    while k < b.len() {
        match b[k] {
            b'"' => k = skip_string(b, k),
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'{' if depth == 0 => {
                let close = matching(b, k)?;
                let mut end = close + 1;
                if matches!(kw.as_str(), "const" | "static" | "type") {
                    let n = skip_ws(b, end);
                    if b.get(n) == Some(&b';') {
                        end = n + 1;
                    }
                }
                return Some(Item { start, end, header_end: k, kw, name });
            }
            b';' if depth == 0 => return Some(Item { start, end: k + 1, header_end: k, kw, name }),
            _ => {}
        }
        k += 1;
    }
    None
}

/// `#[...]` and `#![...]` ranges.
pub fn attribute_ranges(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i = skip_string(b, i);
        } else if b[i] == b'#' {
            let mut j = i + 1;
            if b.get(j) == Some(&b'!') {
                j += 1;
            }
            if b.get(j) == Some(&b'[') {
                if let Some(close) = matching(b, j) {
                    out.push((i, close + 1));
                    i = close;
                }
            }
        }
        i += 1;
    }
    out
}

/// Greedy deletion of ranges, last ones first, `par_checks()` candidates per round; earlier ranges keep their meaning when a
/// later one is removed.
pub(crate) fn delete_ranges(it: &dyn Oracle, src: &str, mut ranges: Vec<(usize, usize)>, deadline: Instant) -> String {
    let mut cur = src.to_string();
    ranges.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let width = par_checks();
    while !ranges.is_empty() {
        if expired(deadline) {
            break;
        }
        let batch: Vec<(usize, usize)> = ranges.iter().take(width).copied().collect();
        let cands: Vec<String> = batch.iter().map(|&(s, e)| format!("{}{}", &cur[..s], &cur[e..])).collect();
        let res = par_map(&cands, width, |c| !c.trim().is_empty() && it.check(c));
        match res.iter().position(|&ok| ok) {
            Some(p) => {
                let (s, e) = batch[p];
                cur = cands[p].clone();
                let removed = e - s;
                // what lay inside is gone, what lay after moved; what lay before is untouched
                ranges = ranges.into_iter().filter(|&(a, z)| z <= s || a >= e).map(|(a, z)| if a >= e { (a - removed, z - removed) } else { (a, z) }).collect();
            }
            None => {
                ranges.drain(..batch.len());
            }
        }
    }
    cur
}

/// Every item (and then every attribute) tried as one deletion unit, repeated while something goes.
pub fn shrink_items(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..4 {
        let before = cur.len();
        let attrs = attribute_ranges(&cur);
        if !attrs.is_empty() {
            let all: String = {
                let mut t = cur.clone();
                for &(s, e) in attrs.iter().rev() {
                    t.replace_range(s..e, "");
                }
                t
            };
            cur = if it.check(&all) { all } else { delete_ranges(it, &cur, attrs, deadline) };
        }
        let items: Vec<(usize, usize)> = item_ranges(&cur).into_iter().map(|i| (i.start, i.end)).collect();
        cur = delete_ranges(it, &cur, items, deadline);
        if cur.len() == before || expired(deadline) {
            break;
        }
    }
    if cur.len() < src.len() {
        info!("items and attributes: {} -> {} lines", src.lines().count(), cur.lines().count());
    }
    cur
}

const BLOCK_STARTS: [&str; 13] = ["if", "while", "for", "loop", "match", "unsafe", "fn", "struct", "enum", "impl", "trait", "mod", "const"];

fn statements_in(src: &str, open: usize, close: usize, out: &mut Vec<(usize, usize)>) {
    let b = src.as_bytes();
    let mut start = open + 1;
    let mut i = open + 1;
    let first = |from: usize| skip_ws(b, from);
    while i < close {
        match b[i] {
            b'"' => i = skip_string(b, i),
            b'(' | b'[' => {
                if let Some(c) = matching(b, i) {
                    i = c;
                }
            }
            b'{' => {
                let Some(c) = matching(b, i) else { break };
                statements_in(src, i, c, out);
                i = c;
                let text = src[start..].trim_start();
                let word: String = text.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                let blocky = text.starts_with('{') || BLOCK_STARTS.contains(&word.as_str()) || (word == "pub");
                let next = skip_ws(b, c + 1);
                let continues = src[next..].starts_with("else") || matches!(b.get(next), Some(b'.') | Some(b'?'));
                if blocky && !continues && b.get(next) != Some(&b';') {
                    out.push((first(start), c + 1));
                    start = c + 1;
                }
            }
            b';' => {
                out.push((first(start), i + 1));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = first(start);
    if tail < close && !src[tail..close].trim().is_empty() {
        out.push((tail, close));
    }
}

/// Statements (up to `;`, or a whole `if`/`loop`/block) of every function body, at every nesting depth.
pub fn statement_ranges(src: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for item in item_ranges(src).iter().filter(|i| i.kw == "fn" && src.as_bytes().get(i.header_end) == Some(&b'{')) {
        statements_in(src, item.header_end, item.end - 1, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Every statement tried as a deletion unit (`let ok = { .. };` that nothing reads, a call whose result is unused, ...).
pub fn shrink_statements(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let cur = delete_ranges(it, src, statement_ranges(src), deadline);
    if cur.len() < src.len() {
        info!("statements: {} -> {} lines", src.lines().count(), cur.lines().count());
    }
    cur
}

fn mentions(text: &str, word: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices(word).any(|(i, _)| (i == 0 || !is_ident(b[i - 1])) && b.get(i + word.len()).is_none_or(|&c| !is_ident(c)))
}

/// Where `(s, e)` lands after the ranges in `removed` (sorted, disjoint) are cut out; `None` if it overlaps one of them.
fn remap(range: (usize, usize), removed: &[(usize, usize)]) -> Option<(usize, usize)> {
    let mut shift = 0;
    for &(s, e) in removed {
        if range.0 < e && s < range.1 {
            return None;
        }
        if e <= range.0 {
            shift += e - s;
        }
    }
    Some((range.0 - shift, range.1 - shift))
}

/// Groups of ranges deleted together (an item and everything that mentions it), greedy from the end of the text.
pub(crate) fn delete_groups(it: &dyn Oracle, src: &str, mut groups: Vec<Vec<(usize, usize)>>, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for g in &mut groups {
        g.sort();
    }
    groups.sort_by(|a, b| b.last().map(|r| r.1).cmp(&a.last().map(|r| r.1)));
    let width = par_checks();
    let cut = |text: &str, g: &[(usize, usize)]| {
        let mut t = text.to_string();
        for &(s, e) in g.iter().rev() {
            t.replace_range(s..e, "");
        }
        t
    };
    while !groups.is_empty() {
        if expired(deadline) {
            break;
        }
        let batch: Vec<Vec<(usize, usize)>> = groups.iter().take(width).cloned().collect();
        let cands: Vec<String> = batch.iter().map(|g| cut(&cur, g)).collect();
        let res = par_map(&cands, width, |c| !c.trim().is_empty() && it.check(c));
        match res.iter().position(|&ok| ok) {
            Some(p) => {
                let removed = batch[p].clone();
                cur = cands[p].clone();
                groups = groups.into_iter().skip(p + 1).filter_map(|g| g.into_iter().map(|r| remap(r, &removed)).collect::<Option<Vec<_>>>()).collect();
            }
            None => {
                groups.drain(..batch.len());
            }
        }
    }
    cur
}

/// An item together with every statement and item that mentions its name goes in one step: a `mod case_17` and the call that
/// runs it, a function and its callers. Plain deletion has to remove the callers first, one by one.
pub fn shrink_items_with_users(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let items = item_ranges(src);
    let statements = statement_ranges(src);
    let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();
    for item in &items {
        let Some(name) = item.name_in(src) else { continue };
        if name == "main" {
            continue;
        }
        let own = (item.start, item.end);
        let outside = |s: usize, e: usize| e <= own.0 || s >= own.1;
        // items whose header mentions the name (`impl Name`, `fn f(x: Name)`) and the innermost statements that mention it
        let mut hits: Vec<(usize, usize)> = items.iter().filter(|o| outside(o.start, o.end) && mentions(&src[o.start..o.header_end], name)).map(|o| (o.start, o.end)).collect();
        let mentioning: Vec<(usize, usize)> = statements.iter().copied().filter(|&(s, e)| outside(s, e) && mentions(&src[s..e], name)).collect();
        for &(s, e) in &mentioning {
            let innermost = !mentioning.iter().any(|&(s2, e2)| (s2, e2) != (s, e) && s <= s2 && e2 <= e);
            let inside_hit_item = hits.iter().any(|&(hs, he)| hs <= s && e <= he);
            if innermost && !inside_hit_item {
                hits.push((s, e));
            }
        }
        hits.sort();
        hits.dedup();
        // an enclosing item that merely contains this one is not a user of it
        hits.retain(|&(s, e)| !(s <= own.0 && own.1 <= e));
        let mut group = vec![own];
        group.extend(hits);
        group.sort();
        // ranges of one group must not overlap each other
        if group.windows(2).all(|w| w[0].1 <= w[1].0) {
            groups.push(group);
        }
    }
    let before = src.lines().count();
    let cur = delete_groups(it, src, groups, deadline);
    if cur.len() < src.len() {
        info!("items with their users: {before} -> {} lines", cur.lines().count());
    }
    cur
}
