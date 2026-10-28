//! Traits taken apart piece by piece: `Trait::f(..)` pointed at the only impl, generic parameters replaced by the type their trait is
//! implemented for (so the methods can be inlined), single trait methods deleted together with their impl copies, single bounds
//! (`T: A + B + C` -> `T: A + C`) deleted. Every step is kept only if the oracle still accepts the program.

use crate::control::{expired, par_checks};
use crate::inline::{generic_names, has_word, replace_word};
use crate::items::{Item, children, delete_groups, delete_ranges, item_ranges};
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::scan::{is_ident, skip_ws};
use log::info;
use std::time::Instant;

struct TraitImpl {
    item: Item,
    trait_name: String,
    ty: String,
    generic: bool,
}

fn trait_impls(src: &str, items: &[Item]) -> Vec<TraitImpl> {
    items
        .iter()
        .filter(|i| i.kw == "impl")
        .filter_map(|i| {
            let head = &src[i.start..i.header_end];
            let after = head[head.find("impl")? + 4..].trim_start();
            let (generic, rest) = if after.starts_with('<') { (true, &after[matching_angle(after)? + 1..]) } else { (false, after) };
            let (tr, ty) = rest.split_once(" for ")?;
            let tr = tr.trim().split('<').next()?.rsplit("::").next()?.trim().to_string();
            let ty = ty.split(" where ").next()?.trim().to_string();
            (!tr.is_empty() && !ty.is_empty()).then_some(TraitImpl { item: i.clone(), trait_name: tr, ty, generic })
        })
        .collect()
}

/// Index of the `>` that closes the `<` at the start of `text`.
fn matching_angle(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0i32;
    for (k, &c) in b.iter().enumerate() {
        match c {
            b'<' => depth += 1,
            b'>' if k == 0 || b[k - 1] != b'-' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

fn plain_type(ty: &str) -> bool {
    !ty.is_empty() && ty.bytes().all(|c| is_ident(c) || c == b':')
}

/// The first accepted candidate, `par_checks()` of them at a time.
fn first_accepted(it: &dyn Oracle, cands: &[String], deadline: Instant) -> Option<usize> {
    let width = par_checks();
    for (n, chunk) in cands.chunks(width).enumerate() {
        if expired(deadline) {
            return None;
        }
        let res = par_map(chunk, width, |c| it.check(c));
        if let Some(p) = res.iter().position(|&ok| ok) {
            return Some(n * width + p);
        }
    }
    None
}

fn word_positions(text: &str, word: &str) -> Vec<usize> {
    let b = text.as_bytes();
    text.match_indices(word).map(|(i, _)| i).filter(|&i| (i == 0 || !is_ident(b[i - 1])) && b.get(i + word.len()).is_none_or(|&c| !is_ident(c))).collect()
}

/// `Trait::f(..)` -> `Type::f(..)` for traits with a single, non-generic impl: the call then looks like a call of the impl's method.
pub fn specialize_trait_paths(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let items = item_ranges(src);
    let impls = trait_impls(src, &items);
    let mut sites: Vec<(usize, usize, String)> = Vec::new();
    for t in items.iter().filter(|i| i.kw == "trait") {
        let Some(name) = t.name_in(src) else { continue };
        let of: Vec<&TraitImpl> = impls.iter().filter(|i| i.trait_name == name).collect();
        let [only] = of.as_slice() else { continue };
        if only.generic || !plain_type(&only.ty) {
            continue;
        }
        for p in word_positions(src, name) {
            let after = &src[p + name.len()..];
            let before = src[..p].trim_end();
            let in_header = items.iter().any(|i| i.start <= p && p < i.header_end);
            if after.starts_with("::") && !before.ends_with("::") && !before.ends_with('<') && !in_header {
                sites.push((p, p + name.len(), only.ty.clone()));
            }
        }
    }
    if sites.is_empty() {
        return src.to_string();
    }
    sites.sort();
    let apply = |text: &str, which: &[(usize, usize, String)]| {
        let mut t = text.to_string();
        for (s, e, ty) in which.iter().rev() {
            t.replace_range(*s..*e, ty);
        }
        t
    };
    let all = apply(src, &sites);
    if it.check(&all) {
        info!("trait paths: {} call(s) pointed at the impl", sites.len());
        return all;
    }
    let mut cur = src.to_string();
    for site in sites.iter().rev().take(40) {
        if expired(deadline) {
            break;
        }
        let cand = apply(&cur, std::slice::from_ref(site));
        if it.check(&cand) {
            cur = cand;
        }
    }
    cur
}

/// Pieces of a `<...>` parameter list: (start, end) of every top-level, comma separated part, as absolute offsets.
fn param_parts(src: &str, open: usize, close: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, open + 1);
    let b = src.as_bytes();
    for k in open + 1..close {
        match b[k] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' if b[k - 1] != b'-' => depth -= 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                out.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
    }
    if !src[start..close].trim().is_empty() {
        out.push((start, close));
    }
    out
}

/// `fn f<T: Num>(..)` -> `fn f(..)` with `T` written as `i128` (the type `Num` is implemented for), for every generic type parameter.
pub fn monomorphize(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..16 {
        if expired(deadline) {
            break;
        }
        let items = item_ranges(&cur);
        let impls = trait_impls(&cur, &items);
        let traits: Vec<&str> = items.iter().filter(|i| i.kw == "trait").filter_map(|i| i.name_in(&cur)).collect();
        let mut cands: Vec<String> = Vec::new();
        for f in items.iter().filter(|i| i.kw == "fn") {
            let Some((_, name_end)) = f.name else { continue };
            let b = cur.as_bytes();
            let open = skip_ws(b, name_end);
            if b.get(open) != Some(&b'<') {
                continue;
            }
            let (_, close) = generic_names(&cur, open);
            if b.get(close) != Some(&b'>') {
                continue;
            }
            let header = &cur[f.start..f.header_end];
            let tys: Vec<&str> = traits
                .iter()
                .filter(|t| has_word(header, t))
                .flat_map(|t| impls.iter().filter(move |i| i.trait_name == *t && !i.generic && plain_type(&i.ty)).map(|i| i.ty.as_str()))
                .collect();
            let parts = param_parts(&cur, open, close);
            for (pi, &(ps, pe)) in parts.iter().enumerate() {
                let text = cur[ps..pe].trim();
                let pname: String = text.chars().take_while(|c| is_ident(*c as u8)).collect();
                if pname.is_empty() || text.starts_with("const ") || text.starts_with('\'') {
                    continue;
                }
                for ty in &tys {
                    let generics = if parts.len() == 1 {
                        String::new()
                    } else {
                        let kept: Vec<&str> = parts.iter().enumerate().filter(|(k, _)| *k != pi).map(|(_, &(s, e))| cur[s..e].trim()).collect();
                        format!("<{}>", kept.join(", "))
                    };
                    let rest = replace_word(&cur[close + 1..f.end], &pname, ty);
                    cands.push(format!("{}{}{}{}", &cur[..open], generics, rest, &cur[f.end..]));
                }
            }
        }
        cands.sort();
        cands.dedup();
        let Some(p) = first_accepted(it, &cands, deadline) else { break };
        info!("monomorphized a generic parameter");
        cur = cands[p].clone();
    }
    cur
}

/// Every method of a trait (declaration plus its copies in the impls of that trait) as one deletion group, then every trait with all
/// its impls.
pub fn remove_trait_methods(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let items = item_ranges(src);
    let impls = trait_impls(src, &items);
    let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();
    for t in items.iter().filter(|i| i.kw == "trait") {
        let Some(tname) = t.name_in(src) else { continue };
        let mut containers: Vec<&Item> = vec![t];
        containers.extend(impls.iter().filter(|i| i.trait_name == tname).map(|i| &i.item));
        let mut by_name: Vec<(String, Vec<(usize, usize)>)> = Vec::new();
        for c in &containers {
            for m in children(&items, Some(c)).into_iter().filter(|m| m.kw == "fn") {
                let Some(mn) = m.name_in(src) else { continue };
                match by_name.iter_mut().find(|(n, _)| n == mn) {
                    Some((_, v)) => v.push((m.start, m.end)),
                    None => by_name.push((mn.to_string(), vec![(m.start, m.end)])),
                }
            }
        }
        groups.extend(by_name.into_iter().map(|(_, v)| v));
        let mut whole: Vec<(usize, usize)> = containers.iter().map(|c| (c.start, c.end)).collect();
        whole.sort();
        groups.push(whole);
    }
    if groups.is_empty() {
        return src.to_string();
    }
    let cur = delete_groups(it, src, groups, deadline);
    if cur.len() < src.len() {
        info!("trait methods: {} -> {} lines", src.lines().count(), cur.lines().count());
    }
    cur
}

/// Ranges that remove one bound from the bound list at `from..to` (the text after a `:`): the bound with its `+`, or, when it is the
/// only one, the colon with it.
fn bound_deletions(src: &str, colon: usize, from: usize, to: usize, out: &mut Vec<(usize, usize)>) {
    let b = src.as_bytes();
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let (mut depth, mut start) = (0i32, from);
    for k in from..to {
        match b[k] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' if b[k - 1] != b'-' => depth -= 1,
            b')' | b']' => depth -= 1,
            b'+' if depth == 0 => {
                pieces.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
    }
    pieces.push((start, to));
    if pieces.len() == 1 {
        out.push((colon, to));
        return;
    }
    for (i, &(s, e)) in pieces.iter().enumerate() {
        out.push(if i + 1 < pieces.len() { (s, pieces[i + 1].0) } else { (pieces[i - 1].1, e) });
    }
}

/// Every `: bounds` of a generic parameter, supertrait list or where predicate in the headers of the program.
fn bound_ranges(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for item in item_ranges(src).iter().filter(|i| matches!(i.kw.as_str(), "fn" | "impl" | "trait" | "struct" | "enum")) {
        let end = item.header_end;
        let mut k = item.start;
        // parentheses hold parameters and their types: `x: T` there is no bound
        let mut paren = 0i32;
        while k < end {
            match b[k] {
                b'(' => paren += 1,
                b')' => paren -= 1,
                b':' if paren == 0 && b.get(k + 1) != Some(&b':') && (k == 0 || b[k - 1] != b':') => {
                    let (mut depth, mut e) = (0i32, k + 1);
                    while e < end {
                        match b[e] {
                            b'<' | b'(' | b'[' => depth += 1,
                            b'>' if b[e - 1] != b'-' => depth -= 1,
                            b')' | b']' => depth -= 1,
                            b',' | b'=' | b';' if depth == 0 => break,
                            b'w' if depth == 0 && src[e..].starts_with("where") && !is_ident(b[e - 1]) => break,
                            _ => {}
                        }
                        if depth < 0 {
                            break;
                        }
                        e += 1;
                    }
                    let text_end = src[..e].trim_end().len();
                    if text_end > k + 1 {
                        let from = k + 1 + (src[k + 1..].len() - src[k + 1..].trim_start().len());
                        bound_deletions(src, k, from, text_end, &mut out);
                    }
                    k = e;
                    continue;
                }
                _ => {}
            }
            k += 1;
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn remove_bounds(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let ranges = bound_ranges(src);
    if ranges.is_empty() {
        return src.to_string();
    }
    let cur = delete_ranges(it, src, ranges, deadline);
    if cur.len() < src.len() {
        info!("bounds: {} -> {} bytes", src.len(), cur.len());
    }
    drop_dangling_plus(it, cur)
}

/// Overlapping deletions can leave `T: A +` behind: valid, but a plus no reader wants.
fn drop_dangling_plus(it: &dyn Oracle, src: String) -> String {
    let b = src.as_bytes();
    let spots: Vec<usize> = (0..b.len())
        .filter(|&k| b[k] == b'+' && src[k + 1..].trim_start().starts_with(['>', ',', '{', ')']))
        .collect();
    let mut out = src.clone();
    for k in spots.into_iter().rev() {
        out.replace_range(k..k + 1, "");
    }
    if out != src && it.check(&out) { out } else { src }
}
