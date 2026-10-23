//! Modules hoisted into the crate root: `mod x { .. }` wrappers are a nuisance for every later pass (paths, `use super::*`).

use crate::items::{children, item_ranges};
use crate::oracle::Oracle;
use crate::rename::{fresh_name, rename_decl};
use crate::scan::{is_ident, matching, skip_ws};
use log::info;

/// `(name, header start, body start, close)` of the top-level `mod name { ... }` items.
pub(crate) fn top_level_modules(src: &str) -> Vec<(String, usize, usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
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
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'm' if depth == 0 && src[i..].starts_with("mod ") && (i == 0 || !is_ident(b[i - 1])) => {
                let ns = skip_ws(b, i + 4);
                let mut ne = ns;
                while ne < b.len() && is_ident(b[ne]) {
                    ne += 1;
                }
                let open = skip_ws(b, ne);
                if ne > ns && open < b.len() && b[open] == b'{' {
                    if let Some(close) = matching(b, open) {
                        out.push((src[ns..ne].to_string(), i, open + 1, close));
                        i = close;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Text of every `use ...;` statement whose path starts with one of `prefixes` removed.
fn drop_uses(src: &str, prefixes: &[String]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(p) = rest.find("use ") {
        let at_boundary = p == 0 || !is_ident(rest.as_bytes()[p - 1]);
        let end = rest[p..].find(';').map(|e| p + e + 1);
        match end {
            Some(e) if at_boundary && prefixes.iter().any(|x| rest[p + 4..e].trim_start().starts_with(x.as_str())) => {
                out.push_str(&rest[..p]);
                rest = &rest[e..];
            }
            _ => {
                out.push_str(&rest[..p + 4]);
                rest = &rest[p + 4..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `name::` removed from paths (`a::a(x)` -> `a(x)`, `crate::a::f` -> `crate::f`).
fn drop_path_prefix(src: &str, name: &str) -> String {
    let key = format!("{name}::");
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut from = 0;
    while let Some(p) = src[from..].find(&key) {
        let at = from + p;
        let before_ok = at == 0 || !(is_ident(b[at - 1]) || (b[at - 1] == b':' && !src[..at].ends_with("crate::")));
        out.push_str(&src[from..at]);
        if before_ok {
            from = at + key.len();
        } else {
            out.push_str(&key);
            from = at + key.len();
        }
    }
    out.push_str(&src[from..]);
    out
}

pub(crate) fn hoist(src: &str, only: &[String]) -> String {
    let mods: Vec<_> = top_level_modules(src).into_iter().filter(|m| only.contains(&m.0)).collect();
    let mut out = src.to_string();
    for (_, head, body, close) in mods.iter().rev() {
        // the body now lives at the crate root: `use super::...` has nothing to point at
        let inner = drop_uses(&out[*body..*close], &["super::".to_string(), "self::".to_string()]);
        out.replace_range(*head..close + 1, &inner);
    }
    for (n, ..) in &mods {
        out = out.replace(&format!("super::{n}::"), "super::").replace(&format!("crate::{n}::"), "crate::");
        out = drop_uses(&out, &[format!("{n}::")]);
        out = drop_path_prefix(&out, n);
    }
    out
}

/// `name` hoisted after renaming the items inside it whose names are already taken at the crate root; the compiler finds the uses.
pub fn hoist_with_renames(src: &str, name: &str, rustc: &[String], work: &std::path::Path) -> Option<String> {
    let mut cur = src.to_string();
    let mut renamed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 0..64 {
        let items = item_ranges(&cur);
        let module = children(&items, None).into_iter().find(|i| i.kw == "mod" && i.name_in(&cur) == Some(name))?.clone();
        // any word used outside the module may mean something else there once the items share its scope (shadowing rules, glob imports)
        let outside = format!("{}{}", &cur[..module.start], &cur[module.end..]);
        let taken: std::collections::HashSet<&str> = outside.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        let clash = children(&items, Some(&module)).into_iter().find_map(|k| {
            let (s, e) = k.name?;
            (taken.contains(&cur[s..e]) && !renamed.contains(&cur[s..e])).then(|| (s, cur[s..e].to_string()))
        });
        let Some((pos, old)) = clash else {
            return Some(hoist(&cur, &[name.to_string()]));
        };
        let new = fresh_name(&cur, &format!("{old}_{name}"));
        cur = rename_decl(rustc, work, &cur, pos, &old, &new)?;
        renamed.insert(new);
    }
    None
}

/// Top-level modules (those for which `keep` is false) hoisted into the crate root: all at once, else one at a time; a module
/// whose names collide with the root's is hoisted after renaming the colliding items (needs `rustc`, empty skips that).
pub fn hoist_modules(it: &dyn Oracle, src: &str, keep: &dyn Fn(&str) -> bool, rustc: &[String]) -> String {
    let names: Vec<String> = top_level_modules(src).into_iter().map(|m| m.0).filter(|n| !keep(n)).collect();
    if names.is_empty() {
        return src.to_string();
    }
    if names.len() > 12 {
        // dozens of modules are a batch of independent cases: hoisting them one by one costs an oracle call each and flattens nothing useful
        info!("{} top-level modules: hoisting skipped (it is meant for a handful)", names.len());
        return src.to_string();
    }
    let all = hoist(src, &names);
    if it.check(&all) {
        info!("hoisted the modules {} into the crate root ({} -> {} lines)", names.join(", "), src.lines().count(), all.lines().count());
        crate::control::rebase_checkpoint(&all);
        return all;
    }
    info!("hoisting the modules {} together changes the behaviour or does not compile (names collide?) - one by one", names.join(", "));
    let mut cur = src.to_string();
    for n in &names {
        let plain = hoist(&cur, std::slice::from_ref(n));
        let renamed = || if rustc.is_empty() || cur.len() > 60_000 { None } else { hoist_with_renames(&cur, n, rustc, &it.work_dir()) };
        let accepted = if plain != cur && it.check(&plain) {
            Some((plain, false))
        } else {
            renamed().filter(|c| *c != cur && it.check(c)).map(|c| (c, true))
        };
        if let Some((cand, renamed)) = accepted {
            info!("hoisted the module {n} into the crate root{}", if renamed { " (colliding names renamed)" } else { "" });
            crate::control::rebase_checkpoint(&cand);
            cur = cand;
        }
    }
    cur
}
