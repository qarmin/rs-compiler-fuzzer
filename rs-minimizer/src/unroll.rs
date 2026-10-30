//! Un-packing before reduction: user `macro_rules!` are expanded in place (the `go!` macro of a batch,
//! `feed_int!`, whatever a reducer left behind) and `catch_unwind` wrappers are removed. The result is longer but plain code
//! that every later pass can shrink; each step is kept only if the program stays interesting.

use crate::oracle::Oracle;
use crate::scan::{is_ident, matching, skip_ws, top_args};
use log::info;

struct MacroDef {
    name: String,
    /// byte range of the whole `macro_rules! name { ... }`
    range: (usize, usize),
    params: Vec<String>,
    repeated: bool,
    comma_sep: bool,
    body: String,
}

/// Single-rule macros with `$x:frag, ...` or `$($x:frag),*` patterns; everything else is left alone.
fn parse_macros(src: &str) -> Vec<MacroDef> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = src[from..].find("macro_rules!") {
        let start = from + p;
        from = start + 1;
        let mut i = skip_ws(b, start + "macro_rules!".len());
        let name_start = i;
        while i < b.len() && is_ident(b[i]) {
            i += 1;
        }
        let name = src[name_start..i].to_string();
        i = skip_ws(b, i);
        if name.is_empty() || i >= b.len() || b[i] != b'{' {
            continue;
        }
        let Some(def_close) = matching(b, i) else { continue };
        let inner_start = skip_ws(b, i + 1);
        if inner_start >= b.len() || !matches!(b[inner_start], b'(' | b'[' | b'{') {
            continue;
        }
        let Some(pat_close) = matching(b, inner_start) else { continue };
        let pat = &src[inner_start + 1..pat_close];
        let mut j = skip_ws(b, pat_close + 1);
        if !src[j..].starts_with("=>") {
            continue;
        }
        j = skip_ws(b, j + 2);
        if j >= b.len() || !matches!(b[j], b'(' | b'[' | b'{') {
            continue;
        }
        let Some(body_close) = matching(b, j) else { continue };
        // exactly one rule: only `;` / whitespace may follow it
        if !src[body_close + 1..def_close].trim().trim_start_matches(';').trim().is_empty() {
            continue;
        }
        let body = src[j + 1..body_close].to_string();
        let pat_t = pat.trim();
        let comma_sep = pat_t.ends_with("),*");
        let (params, repeated) = if let Some(rest) = pat_t.strip_prefix("$(").and_then(|r| r.strip_suffix(")*").or_else(|| r.strip_suffix("),*"))) {
            (vec![rest.split(':').next().unwrap_or("").trim().trim_start_matches('$').to_string()], true)
        } else {
            let names: Vec<String> = pat_t.split(',').map(|p| p.trim().trim_start_matches('$').split(':').next().unwrap_or("").to_string()).filter(|n| !n.is_empty()).collect();
            if names.is_empty() && !pat_t.is_empty() {
                continue;
            }
            (names, false)
        };
        if params.iter().any(|p| p.is_empty() || !p.bytes().all(is_ident)) {
            continue;
        }
        out.push(MacroDef { name, range: (start, def_close + 1), params, repeated, comma_sep, body });
    }
    out
}

fn substitute(text: &str, params: &[String], args: &[String]) -> String {
    let mut out = text.to_string();
    for (p, a) in params.iter().zip(args) {
        let key = format!("${p}");
        let mut res = String::new();
        let mut rest = out.as_str();
        while let Some(pos) = rest.find(&key) {
            let after = rest.as_bytes().get(pos + key.len()).copied();
            res.push_str(&rest[..pos]);
            if after.is_some_and(is_ident) {
                res.push_str(&key);
            } else {
                if res.as_bytes().last().is_some_and(|&c| is_ident(c)) {
                    res.push(' ');
                }
                res.push_str(a);
            }
            rest = &rest[pos + key.len()..];
        }
        res.push_str(rest);
        out = res;
    }
    out
}

fn expansion(def: &MacroDef, args: &[String]) -> Option<String> {
    let body = def.body.trim();
    let text = if def.repeated {
        let at = body.find("$(")?;
        let close = matching(body.as_bytes(), at + 1)?;
        let sep_len = usize::from(matches!(body.as_bytes().get(close + 1), Some(b',' | b';')));
        if body.as_bytes().get(close + 1 + sep_len) != Some(&b'*') {
            return None;
        }
        let inner = &body[at + 2..close];
        let each: Vec<String> = args.iter().map(|a| substitute(inner, &def.params, std::slice::from_ref(a)).trim().to_string()).collect();
        format!("{}{}{}", &body[..at], each.join("\n"), &body[close + 2 + sep_len..])
    } else {
        substitute(body, &def.params, args)
    };
    if text.contains('$') {
        return None;
    }
    let t = text.trim();
    let stmt = t.starts_with("let ") || t.starts_with('{');
    Some(if stmt && !t.contains("//") {
        let one = t.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        if t.starts_with('{') { one } else { format!("{{ {one} }}") }
    } else {
        t.to_string()
    })
}

/// Invocations of `def` between the end of its definition and `region_end` (the next macro of the same name shadows it) replaced
/// by their expansion; the definition is removed when nothing is left to expand.
fn expand_def(src: &str, def: &MacroDef, region_end: usize) -> String {
    let b = src.as_bytes();
    let pat = format!("{}!", def.name);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut from = def.range.1;
    let mut all_done = true;
    while from < region_end {
        let Some(p) = src[from..region_end].find(&pat) else { break };
        let at = from + p;
        from = at + pat.len();
        if at > 0 && is_ident(b[at - 1]) {
            continue;
        }
        let open = at + pat.len();
        if open >= b.len() || !matches!(b[open], b'(' | b'[' | b'{') {
            all_done = false;
            continue;
        }
        let Some(close) = matching(b, open) else {
            all_done = false;
            continue;
        };
        let args: Vec<String> = if def.repeated && !def.comma_sep {
            src[open + 1..close].split_whitespace().map(String::from).collect()
        } else {
            top_args(src, open, close).into_iter().map(|(s, e)| src[s..e].trim().to_string()).collect()
        };
        if !def.repeated && args.len() != def.params.len() {
            all_done = false;
            continue;
        }
        let Some(exp) = expansion(def, &args) else {
            all_done = false;
            continue;
        };
        let line_start = src[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let own_stmt = src[line_start..at].trim().is_empty();
        let mut end = close + 1;
        if own_stmt && src[end..].starts_with(';') {
            end += 1;
        }
        edits.push((at, end, exp));
    }
    let mut out = src.to_string();
    for (s, e, t) in edits.into_iter().rev() {
        out.replace_range(s..e, &t);
    }
    if all_done {
        let (s, e) = def.range;
        let e = if out[e..].starts_with(';') { e + 1 } else { e };
        out.replace_range(s..e, "");
    }
    out
}

/// Macro definitions expanded one after another (the same name may be defined several times: every definition only covers the
/// text up to the next one). `accept` decides whether a step is kept.
fn expand_all(src: &str, accept: &mut dyn FnMut(&str, &str) -> bool) -> String {
    let mut cur = src.to_string();
    let mut skip = 0;
    loop {
        let defs = parse_macros(&cur);
        let Some(d) = defs.get(skip) else { break };
        let region_end = defs.iter().skip(skip + 1).find(|o| o.name == d.name).map(|o| o.range.0).unwrap_or(cur.len());
        let cand = expand_def(&cur, d, region_end);
        if cand != cur && accept(&d.name, &cand) {
            cur = cand;
        } else {
            skip += 1;
        }
    }
    cur
}

/// All single-rule macros of the file expanded; if that changes the behaviour, one macro at a time.
pub fn expand_macros(it: &dyn Oracle, src: &str) -> String {
    let defs = parse_macros(src);
    if defs.is_empty() {
        return src.to_string();
    }
    let all = expand_all(src, &mut |_, _| true);
    if all != src && it.check(&all) {
        info!("expanded all macros ({} -> {} lines)", src.lines().count(), all.lines().count());
        crate::control::rebase_checkpoint(&all);
        return all;
    }
    let cur = expand_all(src, &mut |name, cand| {
        let ok = it.check(cand);
        info!("macro {name}: {}", if ok { "expanded" } else { "expanding it changes the behaviour - kept" });
        ok
    });
    if cur != src {
        crate::control::rebase_checkpoint(&cur);
    }
    let left = parse_macros(&cur).len();
    if left > 0 {
        info!("{left} macro definition(s) could not be expanded (several rules, unsupported patterns or the bug depends on them)");
    }
    cur
}

const CATCH: &str = "std::panic::catch_unwind(std::panic::AssertUnwindSafe(||";

/// Each `catch_unwind(AssertUnwindSafe(|| X)).is_ok()` as `{ X; true }`, and one used as a statement as `{ X }`.
fn without_catch_unwind(src: &str, only: Option<usize>) -> (String, usize) {
    let b = src.as_bytes();
    let mut sites: Vec<(usize, usize, String)> = Vec::new();
    let mut from = 0;
    while let Some(p) = src[from..].find(CATCH) {
        let at = from + p;
        from = at + CATCH.len();
        let assert_open = at + "std::panic::catch_unwind(std::panic::AssertUnwindSafe".len();
        let Some(inner_close) = matching(b, assert_open) else { continue };
        let Some(outer_close) = matching(b, at + "std::panic::catch_unwind".len()) else { continue };
        let tail = ".is_ok()";
        if inner_close + 1 != outer_close {
            continue;
        }
        let x = src[at + CATCH.len()..inner_close].trim();
        if src[outer_close + 1..].starts_with(tail) {
            sites.push((at, outer_close + 1 + tail.len(), format!("{{ {x}; true }}")));
        } else if src[outer_close + 1..].trim_start().starts_with(';') {
            sites.push((at, outer_close + 1, format!("{{ {x} }}")));
        }
    }
    let count = sites.len();
    let mut out = src.to_string();
    for (i, (s, e, t)) in sites.into_iter().enumerate().rev() {
        if only.is_none_or(|o| o == i) {
            out.replace_range(s..e, &t);
        }
    }
    (out, count)
}

pub fn drop_catch_unwind(it: &dyn Oracle, src: &str) -> String {
    let (all, count) = without_catch_unwind(src, None);
    if count == 0 {
        return src.to_string();
    }
    if it.check(&all) {
        info!("removed {count} catch_unwind wrapper(s)");
        crate::control::rebase_checkpoint(&all);
        return all;
    }
    // single sites, last first so that earlier site numbers stay valid
    let mut cur = src.to_string();
    let mut removed = 0;
    for i in (0..count).rev() {
        let (cand, _) = without_catch_unwind(&cur, Some(i));
        if cand != cur && it.check(&cand) {
            cur = cand;
            removed += 1;
        }
    }
    if removed > 0 {
        info!("removed {removed} of {count} catch_unwind wrapper(s)");
        crate::control::rebase_checkpoint(&cur);
    }
    cur
}

fn drop_hook(it: &dyn Oracle, src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let Some(i) = lines.iter().position(|l| l.trim_start().starts_with("std::panic::set_hook(")) else { return src.to_string() };
    let cand: Vec<&str> = lines.iter().enumerate().filter(|(n, _)| *n != i).map(|(_, l)| *l).collect();
    let cand = cand.join("\n") + "\n";
    if it.check(&cand) { cand } else { src.to_string() }
}

/// First reduction step: plain code instead of macros and panic plumbing.
pub fn unroll(it: &dyn Oracle, src: &str) -> String {
    let mut cur = expand_macros(it, &normalize_spacing(src));
    cur = drop_catch_unwind(it, &cur);
    if !cur.contains("catch_unwind") {
        cur = drop_hook(it, &cur);
    }
    cur
}


/// `let(a, b) = ..` is the same program as `let (a, b) = ..`; the passes look for `let ` (reducers that strip whitespace write the
/// first form).
pub fn normalize_spacing(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let e = {
                let mut k = i + 1;
                while k < b.len() && b[k] != b'"' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
                k.min(b.len() - 1)
            };
            out.push_str(&src[i..=e]);
            i = e + 1;
        } else if src[i..].starts_with("let(") && (i == 0 || !is_ident(b[i - 1])) {
            out.push_str("let (");
            i += 4;
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}
