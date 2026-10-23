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

fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(l) if r.0 <= l.1 => l.1 = l.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// Groups of ranges deleted together (an item and everything that mentions it): ddmin over the groups, chunks of neighbouring
/// groups first, then smaller ones, down to single groups.
pub(crate) fn delete_groups(it: &dyn Oracle, src: &str, mut groups: Vec<Vec<(usize, usize)>>, deadline: Instant) -> String {
    let mut cur = src.to_string();
    // neighbours in the text are neighbours in the list
    groups.sort_by_key(|g| g.iter().map(|r| r.0).min().unwrap_or(0));
    let width = par_checks();
    let cut = |text: &str, merged: &[(usize, usize)]| {
        let mut t = text.to_string();
        for &(s, e) in merged.iter().rev() {
            t.replace_range(s..e, "");
        }
        t
    };
    let mut n = (groups.len() / 2).max(1);
    loop {
        let mut i = groups.len() as isize - n as isize;
        while i >= 0 {
            if expired(deadline) {
                return cur;
            }
            let starts: Vec<isize> = (0..width as isize).map(|k| i - k * n as isize).take_while(|&s| s >= 0).collect();
            let merged: Vec<Vec<(usize, usize)>> = starts.iter().map(|&s| merge_ranges(groups[s as usize..s as usize + n].iter().flatten().copied().collect())).collect();
            let cands: Vec<String> = merged.iter().map(|m| cut(&cur, m)).collect();
            let res = par_map(&cands, width, |c| !c.trim().is_empty() && it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let s = starts[p] as usize;
                    let removed = merged[p].clone();
                    cur = cands[p].clone();
                    groups.drain(s..s + n);
                    groups = groups.into_iter().filter_map(|g| g.into_iter().map(|r| remap(r, &removed)).collect::<Option<Vec<_>>>()).collect();
                    i = (s as isize).min(groups.len() as isize - n as isize);
                }
                None => i = starts.last().copied().unwrap_or(0) - n as isize,
            }
        }
        if n == 1 || groups.is_empty() {
            return cur;
        }
        n = (n / 2).max(1);
    }
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

/// Statements of every block of every function body, one list per block (siblings).
fn block_statement_lists(src: &str, open: usize, close: usize, out: &mut Vec<Vec<(usize, usize)>>) {
    let b = src.as_bytes();
    let mut mine: Vec<(usize, usize)> = Vec::new();
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
                block_statement_lists(src, i, c, out);
                i = c;
                let text = src[start..].trim_start();
                let word: String = text.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                let blocky = text.starts_with('{') || BLOCK_STARTS.contains(&word.as_str()) || word == "pub";
                let next = skip_ws(b, c + 1);
                let continues = src[next..].starts_with("else") || matches!(b.get(next), Some(b'.') | Some(b'?'));
                if blocky && !continues && b.get(next) != Some(&b';') {
                    mine.push((first(start), c + 1));
                    start = c + 1;
                }
            }
            b';' => {
                mine.push((first(start), i + 1));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = first(start);
    if tail < close && !src[tail..close].trim().is_empty() {
        mine.push((tail, close));
    }
    if mine.len() >= 2 {
        out.push(mine);
    }
}

/// An item's range including the attributes in front of it.
fn with_attributes(attrs: &[(usize, usize)], src: &str, start: usize) -> usize {
    let mut s = start;
    while let Some(&(a, _)) = attrs.iter().find(|&&(a, e)| a < s && e <= s && src[e..s].trim().is_empty()) {
        s = a;
    }
    s
}

/// Lists of siblings: the items of the crate root, of every `mod` / `impl` / `trait` body, and the statements of every block.
/// Statement lists come first (they use the items, so they have to go before the items can), then module bodies, then the crate root.
fn sibling_lists(src: &str) -> Vec<(u8, Vec<(usize, usize)>)> {
    let items = item_ranges(src);
    let attrs = attribute_ranges(src);
    let b = src.as_bytes();
    let range_of = |i: &Item| (with_attributes(&attrs, src, i.start), i.end);
    let mut lists: Vec<(u8, Vec<(usize, usize)>)> = Vec::new();
    let mut push_items = |parent: Option<&Item>| {
        let kids: Vec<(usize, usize)> = children(&items, parent).into_iter().filter(|k| k.kw != "use").map(&range_of).collect();
        if kids.len() >= 2 {
            lists.push((if parent.is_some() { 1 } else { 2 }, kids));
        }
    };
    push_items(None);
    for item in items.iter().filter(|i| matches!(i.kw.as_str(), "mod" | "impl" | "trait") && b.get(i.header_end) == Some(&b'{')) {
        push_items(Some(item));
    }
    for item in items.iter().filter(|i| i.kw == "fn" && b.get(i.header_end) == Some(&b'{')) {
        let mut found = Vec::new();
        block_statement_lists(src, item.header_end, item.end - 1, &mut found);
        lists.extend(found.into_iter().map(|l| (0, l)));
    }
    lists
}

fn list_key(src: &str, list: &[(usize, usize)]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &(s, e) in list {
        for byte in src[s..e].bytes().chain(std::iter::once(0xff)) {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// Name bound by a plain `let name = ..;` / `let mut name = ..;` statement.
fn let_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("let ")?.trim_start();
    let rest = rest.strip_prefix("mut ").map(str::trim_start).unwrap_or(rest);
    let name: &str = &rest[..rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len())];
    (!name.is_empty() && name != "_" && rest[name.len()..].trim_start().starts_with(['=', ':'])).then_some(name)
}

/// Statements that only existed for the removed ones: a `let x = ..;` whose name no remaining statement behind it mentions. One
/// oracle call then removes a result together with everything that computed it (a backward slice).
fn cascade(src: &str, list: &[(usize, usize)], removed: &mut Vec<bool>) {
    loop {
        let mut changed = false;
        for j in (0..list.len()).rev() {
            if removed[j] {
                continue;
            }
            let Some(name) = let_name(&src[list[j].0..list[j].1]) else { continue };
            let used = (j + 1..list.len()).any(|k| !removed[k] && mentions(&src[list[k].0..list[k].1], name));
            if !used {
                removed[j] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// ddmin over one list of siblings: chunks of consecutive siblings are deleted, halving the chunk size. For statement lists a
/// deleted statement takes the `let`s that only fed it with it.
fn ddmin_siblings(it: &dyn Oracle, cur: &mut String, mut list: Vec<(usize, usize)>, with_cascade: bool, deadline: Instant) {
    let width = par_checks();
    // what ddmin deletes: all siblings, or for statement lists only the statements that are not plain `let`s (the lets go with
    // the statements that used them, see `cascade`)
    let units_of = |cur: &str, list: &[(usize, usize)]| -> Vec<usize> {
        let roots: Vec<usize> = (0..list.len()).filter(|&j| let_name(&cur[list[j].0..list[j].1]).is_none()).collect();
        if with_cascade && roots.len() >= 2 { roots } else { (0..list.len()).collect() }
    };
    let mut units = units_of(cur, &list);
    let mut n = (units.len() / 2).max(1);
    loop {
        let mut i = units.len() as isize - n as isize;
        while i >= 0 {
            if expired(deadline) {
                return;
            }
            let starts: Vec<isize> = (0..width as isize).map(|k| i - k * n as isize).take_while(|&s| s >= 0).collect();
            let plans: Vec<Vec<bool>> = starts
                .iter()
                .map(|&s| {
                    let mut removed = vec![false; list.len()];
                    for &u in &units[s as usize..s as usize + n] {
                        removed[u] = true;
                    }
                    if with_cascade {
                        cascade(cur, &list, &mut removed);
                    }
                    removed
                })
                .collect();
            let cut = |removed: &[bool]| {
                let mut t = cur.clone();
                for (j, &r) in removed.iter().enumerate().rev() {
                    if r {
                        t.replace_range(list[j].0..list[j].1, "");
                    }
                }
                t
            };
            let cands: Vec<String> = plans.iter().map(|p| cut(p)).collect();
            let res = par_map(&cands, width, |c| !c.trim().is_empty() && it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let removed = &plans[p];
                    *cur = cands[p].clone();
                    let mut shift = 0;
                    let mut kept: Vec<(usize, usize)> = Vec::new();
                    for (j, &(a, z)) in list.iter().enumerate() {
                        if removed[j] {
                            shift += z - a;
                        } else {
                            kept.push((a - shift, z - shift));
                        }
                    }
                    list = kept;
                    if list.len() < 2 {
                        return;
                    }
                    units = units_of(cur, &list);
                    // the units before the removed ones are the same units
                    i = starts[p].min(units.len() as isize - n as isize);
                }
                None => i = starts.last().copied().unwrap_or(0) - n as isize,
            }
        }
        if n == 1 || units.len() < 2 {
            return;
        }
        n = (n / 2).max(1);
    }
}

/// Delta debugging on the structure: for every list of siblings (the items of a module, the statements of a block) chunks of
/// neighbours are deleted at once, largest lists first. Far fewer oracle calls than deleting one item or line at a time.
pub fn shrink_siblings(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    let mut done: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for _ in 0..400 {
        if expired(deadline) {
            break;
        }
        let mut lists = sibling_lists(&cur);
        lists.sort_by_key(|(prio, l)| (*prio, std::cmp::Reverse(l.last().map(|r| r.1).unwrap_or(0) - l.first().map(|r| r.0).unwrap_or(0))));
        let Some((prio, list)) = lists.into_iter().find(|(_, l)| !done.contains(&list_key(&cur, l))) else { break };
        done.insert(list_key(&cur, &list));
        let before = cur.len();
        ddmin_siblings(it, &mut cur, list.clone(), prio == 0, deadline);
        if cur.len() != before {
            // deleted statements leave empty lines behind; they carry no meaning
            cur = cur.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n") + "\n";
            info!("siblings ({} items or statements in a list): {} -> {} bytes", list.len(), before, cur.len());
            // what is left of the list is finished; the lists nested in its survivors are found by the next round
        }
        let rest = sibling_lists(&cur);
        for (_, l) in rest {
            if l.len() <= 1 {
                done.insert(list_key(&cur, &l));
            }
        }
    }
    cur
}
