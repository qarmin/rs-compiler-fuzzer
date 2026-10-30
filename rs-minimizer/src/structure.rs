//! Structural simplifications that need more than one edit: a newtype struct replaced by its field, function bodies stubbed out,
//! unused parameters and fields removed together with their arguments and initialisers, enum variants and match arms deleted.

use crate::control::{expired, par_checks};
use crate::exprs::greedy_edits;
use crate::inline::{all_defs_of, find_sites};
use crate::items::item_ranges;
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::scan::{is_ident, matching, skip_ws, top_args};
use log::info;
use std::time::Instant;

fn whole_words(text: &str, word: &str) -> Vec<usize> {
    let b = text.as_bytes();
    text.match_indices(word).map(|(i, _)| i).filter(|&i| (i == 0 || !is_ident(b[i - 1])) && b.get(i + word.len()).is_none_or(|&c| !is_ident(c))).collect()
}

/// How `.0` of the wrapper is rewritten: gone (reads through a reference still work), or `*x` (a write through a reference).
#[derive(Clone, Copy)]
struct Access {
    write: bool,
    read: bool,
    /// decide per occurrence: dereference when the identifier was declared as a reference
    auto: bool,
}

/// Was the nearest declaration of `ident` before `pos` a reference (`ident: &T`, `let ident = &..`)?
fn declared_as_reference(src: &str, pos: usize, ident: &str) -> bool {
    let b = src.as_bytes();
    let mut best: Option<(usize, bool)> = None;
    for at in whole_words(&src[..pos], ident) {
        let after = src[at + ident.len()..].trim_start();
        let before = src[..at].trim_end();
        if let Some(rest) = after.strip_prefix(':').filter(|r| !r.starts_with(':')) {
            best = Some((at, rest.trim_start().starts_with('&')));
        } else if before.ends_with("let") || before.ends_with("let mut") || before.ends_with("mut") && before[..before.len() - 3].trim_end().ends_with("let") {
            let eq = src[at..].find('=').map(|p| at + p);
            if let Some(eq) = eq {
                let init = src[eq + 1..].trim_start();
                best = Some((at, init.starts_with('&')));
            }
        }
        let _ = b;
    }
    best.is_some_and(|(_, r)| r)
}

/// `struct W(T);` removed: `W(x)` becomes `(x)`, the type `W` becomes `T` and `x.0` becomes `x` or `*x` (a wrapper that only
/// carries a value).
fn unwrap_newtype(src: &str, item: &crate::items::Item, deref: Access) -> Option<String> {
    let name = item.name_in(src)?;
    let header = &src[item.start..item.header_end];
    let open = header.find('(')?;
    let ty_text = header[open + 1..].trim_end().strip_suffix(')')?.trim();
    let ty = ty_text.strip_prefix("pub ").unwrap_or(ty_text).trim();
    if ty.is_empty() || ty.contains(',') {
        return None;
    }
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i == item.start {
            i = item.end;
            continue;
        }
        if b[i] == b'"' {
            let mut k = i + 1;
            while k < b.len() && b[k] != b'"' {
                if b[k] == b'\\' {
                    k += 1;
                }
                k += 1;
            }
            let e = (k + 1).min(b.len());
            out.push_str(&src[i..e]);
            i = e;
            continue;
        }
        if src[i..].starts_with(name) && (i == 0 || !is_ident(b[i - 1])) && b.get(i + name.len()).is_none_or(|&c| !is_ident(c)) {
            let ctor = b.get(i + name.len()) == Some(&b'(');
            out.push_str(if ctor { "" } else { ty });
            i += name.len();
            continue;
        }
        if src[i..].starts_with(".0") && b.get(i + 2).is_none_or(|&c| !is_ident(c)) && i > 0 {
            let mut k = i;
            while k > 0 && is_ident(b[k - 1]) {
                k -= 1;
            }
            if k < i && !b[k].is_ascii_digit() {
                // `ident.0`: the identifier is already in `out`
                let rest = src[i + 2..].trim_start();
                let writes = (rest.starts_with('=') && !rest.starts_with("==")) || ["+=", "-=", "*=", "/=", "%=", "^=", "|=", "&=", "<<=", ">>="].iter().any(|o| rest.starts_with(o));
                let ident = &src[k..i];
                let as_ref = deref.auto && declared_as_reference(src, i, ident);
                if as_ref || (!deref.auto && if writes { deref.write } else { deref.read }) {
                    let at = out.len() - (i - k);
                    out.insert(at, '*');
                }
                i += 2;
                continue;
            }
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    Some(out)
}

pub fn unwrap_newtypes(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..4 {
        let items = item_ranges(&cur);
        let access = [Access { write: false, read: false, auto: true }, Access { write: false, read: false, auto: false }, Access { write: true, read: false, auto: false }, Access { write: true, read: true, auto: false }];
        let cands: Vec<String> = items
            .iter()
            .filter(|i| i.kw == "struct" && cur.as_bytes().get(i.header_end) == Some(&b';'))
            .flat_map(|i| access.iter().filter_map(|a| unwrap_newtype(&cur, i, *a)).collect::<Vec<_>>())
            .collect();
        if cands.is_empty() || expired(deadline) {
            break;
        }
        let res = par_map(&cands, par_checks(), |c| it.check(c));
        match res.iter().position(|&ok| ok) {
            Some(p) => {
                info!("newtype struct unwrapped ({} -> {} bytes)", cur.len(), cands[p].len());
                cur = cands[p].clone();
            }
            None => break,
        }
    }
    cur
}

/// Function bodies replaced by `unimplemented!()` (it type-checks for any return type and, unlike `loop {}`, a program that runs
/// the body fails at once instead of hanging); what only the body used can then go.
pub fn stub_bodies(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let b = src.as_bytes();
    let edits: Vec<(usize, usize, String)> = item_ranges(src)
        .iter()
        .filter(|i| i.kw == "fn" && b.get(i.header_end) == Some(&b'{') && i.end - i.header_end > 12 && i.name_in(src) != Some("main"))
        .map(|i| (i.header_end, i.end, "{ unimplemented!() }".to_string()))
        .collect();
    greedy_edits(it, src, edits, deadline)
}

/// Removal of element `i` of a comma separated list given as `ranges` (as `top_args` reports them, up to but not including the
/// comma). The last element leaves the comma of its predecessor behind: a trailing comma is fine in every list we cut from.
fn list_removal(src: &str, ranges: &[(usize, usize)], i: usize) -> (usize, usize) {
    let (s, e) = ranges[i];
    let trailing_comma = src.as_bytes().get(e) == Some(&b',');
    if i + 1 < ranges.len() {
        (s, ranges[i + 1].0)
    } else if trailing_comma {
        (s, e + 1)
    } else {
        (s, e)
    }
}

fn apply_removals(src: &str, mut cuts: Vec<(usize, usize)>) -> String {
    cuts.sort();
    cuts.dedup();
    let mut out = src.to_string();
    for &(s, e) in cuts.iter().rev() {
        out.replace_range(s..e, "");
    }
    out
}

/// A parameter that nothing reads goes, with the matching argument of every call.
pub fn remove_params(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..40 {
        if expired(deadline) {
            break;
        }
        let defs = all_defs_of(&cur);
        let mut cands: Vec<String> = Vec::new();
        for d in defs.iter().filter(|d| d.name != "main" && !d.in_trait && defs.iter().filter(|o| o.name == d.name).count() == 1) {
            let b = cur.as_bytes();
            let (ns, ne) = d.item.name.unwrap();
            let open = skip_ws(b, ne);
            let open = if b.get(open) == Some(&b'<') { cur[open..].find('(').map(|p| open + p).unwrap_or(open) } else { open };
            let Some(close) = (b.get(open) == Some(&b'(')).then(|| matching(b, open)).flatten() else { continue };
            let mut header_args = top_args(&cur, open, close);
            let offset = usize::from(d.recv.is_some());
            let sites = find_sites(&cur, d);
            if header_args.len() != d.params.len() + offset || (sites.is_empty() && !d.params.is_empty() && ns == 0) {
                continue;
            }
            header_args = header_args.into_iter().collect();
            for pi in (0..d.params.len()).rev() {
                let mut cuts = vec![list_removal(&cur, &header_args[offset..], pi)];
                for s in &sites {
                    cuts.push(list_removal(&cur, &s.arg_ranges, pi));
                }
                cands.push(apply_removals(&cur, cuts));
            }
        }
        if cands.is_empty() {
            break;
        }
        let mut accepted = None;
        for chunk in cands.chunks(par_checks()) {
            let res = par_map(chunk, par_checks(), |c| it.check(c));
            if let Some(p) = res.iter().position(|&ok| ok) {
                accepted = Some(chunk[p].clone());
                break;
            }
            if expired(deadline) {
                break;
            }
        }
        match accepted {
            Some(next) => {
                info!("a parameter and its arguments removed ({} -> {} bytes)", cur.len(), next.len());
                cur = next;
            }
            None => break,
        }
    }
    cur
}

/// Named struct fields nobody reads go, with their initialisers in struct literals.
pub fn remove_fields(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..40 {
        if expired(deadline) {
            break;
        }
        let items = item_ranges(&cur);
        let b = cur.as_bytes();
        let mut cands: Vec<String> = Vec::new();
        for item in items.iter().filter(|i| i.kw == "struct" && b.get(i.header_end) == Some(&b'{')) {
            let Some(name) = item.name_in(&cur) else { continue };
            let fields = top_args(&cur, item.header_end, item.end - 1);
            for (fi, &(fs, fe)) in fields.iter().enumerate().rev() {
                let text = cur[fs..fe].trim();
                let fname: String = text.trim_start_matches("pub ").trim().chars().take_while(|c| is_ident(*c as u8)).collect();
                if fname.is_empty() {
                    continue;
                }
                let mut cuts = vec![list_removal(&cur, &fields, fi)];
                for at in whole_words(&cur, name) {
                    let after = skip_ws(b, at + name.len());
                    if b.get(after) != Some(&b'{') || (at >= item.start && at < item.end) {
                        continue;
                    }
                    let before = cur[..at].trim_end();
                    if before.ends_with("struct") || before.ends_with("impl") {
                        continue;
                    }
                    let Some(c) = matching(b, after) else { continue };
                    let entries = top_args(&cur, after, c);
                    if let Some(ei) = entries.iter().position(|&(s, e)| {
                        let t = cur[s..e].trim();
                        t == fname || t.strip_prefix(fname.as_str()).is_some_and(|r| r.trim_start().starts_with(':'))
                    }) {
                        cuts.push(list_removal(&cur, &entries, ei));
                    }
                }
                cands.push(apply_removals(&cur, cuts));
            }
        }
        if cands.is_empty() {
            break;
        }
        let mut accepted = None;
        for chunk in cands.chunks(par_checks()) {
            let res = par_map(chunk, par_checks(), |c| it.check(c));
            if let Some(p) = res.iter().position(|&ok| ok) {
                accepted = Some(chunk[p].clone());
                break;
            }
            if expired(deadline) {
                break;
            }
        }
        match accepted {
            Some(next) => {
                info!("a struct field removed ({} -> {} bytes)", cur.len(), next.len());
                cur = next;
            }
            None => break,
        }
    }
    cur
}

/// Enum variants (together with the match arms that name them) and match arms, one at a time.
pub fn remove_variants_and_arms(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let b = src.as_bytes();
    let items = item_ranges(src);
    let mut all_arms: Vec<(usize, usize)> = Vec::new();
    // match arms: from `match` to the `{ .. }` after it, split at depth-0 commas (an arm with a block body may have none)
    for at in whole_words(src, "match") {
        let mut depth = 0i32;
        let mut k = at + 5;
        let mut open = None;
        while k < b.len() {
            match b[k] {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b'{' if depth == 0 => {
                    open = Some(k);
                    break;
                }
                b';' => break,
                _ => {}
            }
            k += 1;
        }
        let Some(open) = open else { continue };
        let Some(close) = matching(b, open) else { continue };
        let mut arms: Vec<(usize, usize)> = Vec::new();
        let mut start = open + 1;
        let mut i = open + 1;
        while i < close {
            match b[i] {
                b'(' | b'[' | b'{' => {
                    if let Some(c) = matching(b, i) {
                        let was_block = b[i] == b'{';
                        i = c;
                        // `pat => { .. }` ends here unless a comma follows
                        let next = skip_ws(b, c + 1);
                        if was_block && b.get(next) != Some(&b',') && src[start..i].contains("=>") && !matches!(b.get(next), Some(b'.') | Some(b'?')) {
                            arms.push((start, c + 1));
                            start = c + 1;
                        }
                    }
                }
                b',' => {
                    arms.push((start, i + 1));
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        if start < close && !src[start..close].trim().is_empty() {
            arms.push((start, close));
        }
        if arms.len() > 1 {
            all_arms.extend(arms);
        }
    }
    let mut groups: Vec<Vec<(usize, usize)>> = all_arms.iter().map(|&a| vec![a]).collect();
    for item in items.iter().filter(|i| i.kw == "enum" && b.get(i.header_end) == Some(&b'{')) {
        let variants = top_args(src, item.header_end, item.end - 1);
        if variants.len() < 2 {
            continue;
        }
        for (vi, &(vs, ve)) in variants.iter().enumerate() {
            let vname: String = src[vs..ve].trim().chars().take_while(|c| is_ident(*c as u8)).collect();
            let mut group = vec![list_removal(src, &variants, vi)];
            // arms that name the variant (`E::V`, `V`) go with it
            for &(s, e) in &all_arms {
                let pat = src[s..e].split("=>").next().unwrap_or("");
                if !vname.is_empty() && !whole_words(pat, &vname).is_empty() {
                    group.push((s, e));
                }
            }
            group.sort();
            if group.windows(2).all(|w| w[0].1 <= w[1].0) {
                groups.push(group);
            }
        }
    }
    crate::items::delete_groups(it, src, groups, deadline)
}
