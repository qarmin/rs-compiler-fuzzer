//! Function and method calls replaced by their bodies, so that the function (and the impl, trait or generic parameter that only
//! existed for it) can go. `f(a, b)` becomes `{ let (x, y) = (a, b); BODY }`, `r.m(a)` becomes
//! `{ let (__s, x) = (&r, a); BODY[self -> __s] }`. Every replacement is kept only if the oracle still accepts the program.

use crate::control::{expired, par_checks};
use crate::items::{Item, item_ranges};
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::rename::{fresh_name, rename_decl};
use crate::scan::{is_ident, matching, postfix_start, skip_ws, top_args};
use log::info;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Recv {
    Ref,
    RefMut,
    Value { mutable: bool },
}

#[derive(Clone, Debug)]
pub(crate) struct Param {
    pub(crate) name: String,
    pub(crate) pos: usize,
    pub(crate) mutable: bool,
    pub(crate) ty: String,
}

#[derive(Clone, Debug)]
pub(crate) struct FnDef {
    pub(crate) name: String,
    pub(crate) name_pos: usize,
    pub(crate) item: Item,
    pub(crate) recv: Option<Recv>,
    pub(crate) params: Vec<Param>,
    pub(crate) generics: Vec<String>,
    pub(crate) body: String,
    pub(crate) unsafe_fn: bool,
    /// type of the enclosing `impl` block (`None` for free functions and generic impls)
    pub(crate) self_ty: Option<String>,
    /// the method belongs to a trait or a trait impl: its name is shared with the trait
    pub(crate) in_trait: bool,
    pub(crate) in_impl: bool,
}

#[derive(Clone, Copy)]
struct Variant {
    annotate: bool,
    reborrow: bool,
}

/// Bytes a function may add to the program by being pasted at all but one of its call sites.
const MAX_GROWTH: usize = 50_000;

const VARIANTS: [Variant; 4] = [Variant { annotate: false, reborrow: false }, Variant { annotate: false, reborrow: true }, Variant { annotate: true, reborrow: false }, Variant { annotate: true, reborrow: true }];

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

/// `text` with the insides of string literals and comments blanked out (same length), for searching the code only.
pub(crate) fn code_only(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        let end = if b[i] == b'"' {
            skip_string(b, i).min(b.len() - 1)
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            text[i..].find('\n').map_or(b.len() - 1, |n| i + n - 1)
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            text[i + 2..].find("*/").map_or(b.len() - 1, |n| i + n + 3)
        } else {
            i += 1;
            continue;
        };
        out[i + 1..end].iter_mut().filter(|c| **c != b'\n').for_each(|c| *c = b' ');
        i = end + 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

pub(crate) fn has_word(text: &str, word: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices(word).any(|(i, _)| (i == 0 || !is_ident(b[i - 1])) && b.get(i + word.len()).is_none_or(|&c| !is_ident(c)))
}

pub(crate) fn replace_word(text: &str, word: &str, with: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let e = skip_string(b, i).min(b.len() - 1);
            out.push_str(&text[i..=e]);
            i = e + 1;
        } else if text[i..].starts_with(word) && (i == 0 || !is_ident(b[i - 1])) && b.get(i + word.len()).is_none_or(|&c| !is_ident(c)) {
            out.push_str(with);
            i += word.len();
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Generic parameter names of `<...>` at `open` (type and const parameters; lifetimes are skipped).
pub(crate) fn generic_names(src: &str, open: usize) -> (Vec<String>, usize) {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut k = open;
    while k < b.len() {
        match b[k] {
            b'<' => depth += 1,
            b'>' if b[k - 1] != b'-' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        k += 1;
    }
    let inner = &src[open + 1..k.min(src.len())];
    let mut names = Vec::new();
    let mut d = 0i32;
    let mut start = 0;
    for (i, c) in inner.char_indices().chain(std::iter::once((inner.len(), ','))) {
        match c {
            '<' | '(' | '[' => d += 1,
            '>' | ')' | ']' => d -= 1,
            ',' if d == 0 => {
                let part = inner[start..i].trim();
                let part = part.strip_prefix("const ").unwrap_or(part);
                let name: String = part.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() && !part.starts_with('\'') {
                    names.push(name);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    (names, k)
}

/// `strict` keeps what a pasted body cannot contain (`extern`/`async` functions, `return`, `?`, recursion); the parameter pass
/// does not paste anything and takes every function.
fn parse_fn(src: &str, items: &[Item], item: &Item, strict: bool) -> Option<FnDef> {
    let b = src.as_bytes();
    if item.kw != "fn" || b.get(item.header_end) != Some(&b'{') || b[item.end - 1] != b'}' {
        return None;
    }
    let (ns, ne) = item.name?;
    let name = src[ns..ne].to_string();
    let header_prefix = &src[item.start..ns];
    let unsafe_fn = has_word(header_prefix, "unsafe");
    if strict && (has_word(header_prefix, "async") || has_word(header_prefix, "extern")) {
        return None;
    }
    let mut i = skip_ws(b, ne);
    let mut generics = Vec::new();
    if b.get(i) == Some(&b'<') {
        let (g, close) = generic_names(src, i);
        generics = g;
        i = skip_ws(b, close + 1);
    }
    if b.get(i) != Some(&b'(') {
        return None;
    }
    let close = matching(b, i)?;
    let mut recv = None;
    let mut params = Vec::new();
    for (k, (s, e)) in top_args(src, i, close).into_iter().enumerate() {
        let text = src[s..e].trim();
        if k == 0 {
            let r = match text {
                "&self" => Some(Recv::Ref),
                "&mut self" => Some(Recv::RefMut),
                "self" => Some(Recv::Value { mutable: false }),
                "mut self" => Some(Recv::Value { mutable: true }),
                _ => None,
            };
            if r.is_some() {
                recv = r;
                continue;
            }
            if has_word(text, "self") {
                return None; // `self: Box<Self>` and friends
            }
        }
        let (pat, ty) = text.split_once(':')?;
        if ty.starts_with(':') {
            return None;
        }
        let pat = pat.trim();
        let (mutable, ident) = match pat.strip_prefix("mut ") {
            Some(r) => (true, r.trim()),
            None => (false, pat),
        };
        if ident.is_empty() || !ident.bytes().all(is_ident) {
            return None;
        }
        let lead = src[s..e].len() - src[s..e].trim_start().len();
        let pos = s + lead + text.find(ty).map(|_| pat.rfind(ident).unwrap_or(0)).unwrap_or(0);
        params.push(Param { name: ident.to_string(), pos, mutable, ty: ty.trim().to_string() });
    }
    let body = src[item.header_end + 1..item.end - 1].to_string();
    if strict {
        let code = code_only(&body);
        if ["return", "async", "await", "yield"].iter().any(|w| has_word(&code, w)) {
            return None;
        }
        let calls_itself = code.match_indices(&format!("{name}(")).any(|(i, _)| i == 0 || !is_ident(code.as_bytes()[i - 1]));
        if code.contains('?') || calls_itself {
            return None;
        }
    }
    let parent = items.iter().filter(|p| p.contains(item)).max_by_key(|p| p.start);
    let (mut self_ty, mut in_trait, mut in_impl) = (None, false, false);
    if let Some(p) = parent {
        let head = src[p.start..p.header_end].trim();
        if p.kw == "trait" {
            in_trait = true;
        } else if p.kw == "impl" {
            in_impl = true;
            let after = head.find("impl").map(|x| &head[x + 4..]).unwrap_or("").trim_start();
            let generic_impl = after.starts_with('<');
            let (ty, is_trait_impl) = match after.rsplit_once(" for ") {
                Some((_, ty)) => (ty, true),
                None => (after, false),
            };
            in_trait = is_trait_impl;
            let ty = ty.split(" where ").next().unwrap_or(ty).trim();
            if !generic_impl && !ty.is_empty() {
                self_ty = Some(ty.to_string());
            }
        } else {
            return None;
        }
    }
    Some(FnDef { name, name_pos: ns, item: item.clone(), recv, params, generics, body, unsafe_fn, self_ty, in_trait, in_impl })
}

pub(crate) fn defs_of(src: &str) -> Vec<FnDef> {
    let items = item_ranges(src);
    items.iter().filter_map(|i| parse_fn(src, &items, i, true)).collect()
}

/// Like `defs_of`, for passes that edit signatures and call sites but never paste a body.
pub(crate) fn all_defs_of(src: &str) -> Vec<FnDef> {
    let items = item_ranges(src);
    items.iter().filter_map(|i| parse_fn(src, &items, i, false)).collect()
}

#[derive(Clone)]
pub(crate) struct Site {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) recv: Option<String>,
    pub(crate) args: Vec<String>,
    /// absolute byte ranges of the arguments (as `top_args` reports them)
    pub(crate) arg_ranges: Vec<(usize, usize)>,
}

pub(crate) fn find_sites(src: &str, def: &FnDef) -> Vec<Site> {
    let b = src.as_bytes();
    let pat = def.name.as_str();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i = skip_string(b, i) + 1;
            continue;
        }
        if !(src[i..].starts_with(pat) && (i == 0 || !is_ident(b[i - 1])) && b.get(i + pat.len()).is_none_or(|&c| !is_ident(c))) {
            i += 1;
            continue;
        }
        let name_at = i;
        i += pat.len();
        let open = skip_ws(b, i);
        if b.get(open) != Some(&b'(') || (name_at >= def.item.start && name_at < def.item.end) {
            continue;
        }
        let Some(close) = matching(b, open) else { continue };
        let before = src[..name_at].trim_end();
        if before.ends_with("fn") && !before[..before.len() - 2].ends_with(|c: char| is_ident(c as u8)) {
            continue;
        }
        let arg_ranges = top_args(src, open, close);
        let args: Vec<String> = arg_ranges.iter().map(|&(s, e)| src[s..e].trim().to_string()).collect();
        if args.len() != def.params.len() {
            continue;
        }
        let site = if before.ends_with('.') {
            if def.recv.is_none() {
                continue;
            }
            let dot = before.len() - 1;
            let start = postfix_start(b, dot);
            if start >= dot {
                continue;
            }
            Site { start, end: close + 1, recv: Some(src[start..dot].trim().to_string()), args, arg_ranges }
        } else if before.ends_with("::") {
            let ty = def.self_ty.as_deref();
            let head = &before[..before.len() - 2];
            // `<Type>::f(..)` and `<Type as Trait>::f(..)` call the same method as `Type::f(..)`
            let qualified = head.ends_with('>').then(|| head.rfind('<')).flatten().map(|lt| (lt, head[lt + 1..head.len() - 1].split(" as ").next().unwrap_or("").trim()));
            let (prefix_start, prefix) = match qualified {
                Some((lt, ty)) => (lt, ty),
                None => {
                    let st = head.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map(|p| p + 1).unwrap_or(0);
                    (st, &before[st..before.len() - 2])
                }
            };
            if def.recv.is_some() || ty != Some(prefix) {
                continue;
            }
            Site { start: prefix_start, end: close + 1, recv: None, args, arg_ranges }
        } else {
            if def.recv.is_some() || def.in_impl {
                continue;
            }
            Site { start: name_at, end: close + 1, recv: None, args, arg_ranges }
        };
        out.push(site);
        i = close + 1;
    }
    // nested sites (an argument that calls the same function) wait for the next round
    out.sort_by_key(|s| s.start);
    let mut kept: Vec<Site> = Vec::new();
    for s in out {
        if kept.last().is_none_or(|k| s.start >= k.end) {
            kept.push(s);
        }
    }
    kept
}

fn annotatable(def: &FnDef) -> bool {
    def.params.iter().all(|p| !has_word(&p.ty, "impl") && !p.ty.contains("dyn ") && def.generics.iter().all(|g| !has_word(&p.ty, g)) && !has_word(&p.ty, "Self"))
}

fn replacement(def: &FnDef, site: &Site, v: Variant) -> Option<String> {
    let mut body = def.body.clone();
    if has_word(&body, "Self") {
        body = replace_word(&body, "Self", def.self_ty.as_deref()?);
    }
    let mut names: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    let mut tys: Vec<String> = Vec::new();
    if let Some(r) = def.recv {
        let text = site.recv.as_deref()?;
        body = replace_word(&body, "self", "__s");
        names.push(if matches!(r, Recv::Value { mutable: true }) { "mut __s".into() } else { "__s".into() });
        vals.push(match (r, v.reborrow) {
            (Recv::Ref, false) => format!("&({text})"),
            (Recv::Ref, true) => format!("&*({text})"),
            (Recv::RefMut, false) => format!("&mut ({text})"),
            (Recv::RefMut, true) => format!("&mut *({text})"),
            (Recv::Value { .. }, _) => format!("({text})"),
        });
        let ty = def.self_ty.as_deref();
        tys.push(match (r, ty) {
            (Recv::Ref, Some(t)) => format!("&{t}"),
            (Recv::RefMut, Some(t)) => format!("&mut {t}"),
            (Recv::Value { .. }, Some(t)) => t.to_string(),
            (_, None) => String::new(),
        });
    }
    for (p, a) in def.params.iter().zip(&site.args) {
        names.push(if p.mutable { format!("mut {}", p.name) } else { p.name.clone() });
        vals.push(a.clone());
        tys.push(p.ty.clone());
    }
    let annotate = v.annotate && annotatable(def) && tys.iter().all(|t| !t.is_empty());
    let lets = match names.len() {
        0 => String::new(),
        1 if annotate => format!("let {}: {} = {};", names[0], tys[0], vals[0]),
        1 => format!("let {} = {};", names[0], vals[0]),
        _ if annotate => format!("let ({}): ({}) = ({});", names.join(", "), tys.join(", "), vals.join(", ")),
        _ => format!("let ({}) = ({});", names.join(", "), vals.join(", ")),
    };
    let inner = if def.unsafe_fn { format!("unsafe {{ {body} }}") } else { body };
    Some(format!("{{ {lets} {inner} }}"))
}

/// The program with the calls of `def` (all sites, or only site number `only`) replaced.
fn inline_def(src: &str, def: &FnDef, v: Variant, only: Option<usize>) -> Option<String> {
    let sites = find_sites(src, def);
    if sites.is_empty() {
        return None;
    }
    let mut out = src.to_string();
    for (n, s) in sites.iter().enumerate().rev() {
        if only.is_none_or(|o| o == n) {
            out.replace_range(s.start..s.end, &replacement(def, s, v)?);
        }
    }
    Some(out)
}

/// Function names that exist twice (an inherent method and a trait method, `fn a` in two modules) get distinct names, so that a
/// call can be matched with its definition by name alone. Methods that belong to a trait keep theirs.
fn uniquify_names(it: &dyn Oracle, src: &str, rustc: &[String]) -> String {
    let mut cur = src.to_string();
    let mut renamed = std::collections::HashSet::new();
    for _ in 0..32 {
        let defs = defs_of(&cur);
        let clash = defs.iter().find(|d| !d.in_trait && !renamed.contains(&d.name_pos) && defs.iter().filter(|o| o.name == d.name).count() > 1);
        let Some(d) = clash else { break };
        let base = d.self_ty.as_deref().map(|t| format!("{}_{}", d.name, t.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect::<String>())).unwrap_or_else(|| format!("{}_fn", d.name));
        let new = fresh_name(&cur, &base);
        let pos = d.name_pos;
        renamed.insert(pos);
        if let Some(next) = rename_decl(rustc, &it.work_dir(), &cur, pos, &d.name.clone(), &new) {
            // positions behind the renamed name moved: the set only guards against retrying the same definition
            renamed = renamed.into_iter().map(|p| if p > pos { p + new.len() - d.name.len() } else { p }).collect();
            cur = next;
        }
    }
    if cur != src && it.check(&cur) {
        info!("gave the duplicate function names distinct names");
        cur
    } else {
        src.to_string()
    }
}

const KEYWORDS: [&str; 37] = ["mut", "ref", "box", "true", "false", "_", "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "pub", "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use", "where"];

/// Names bound by `let` patterns in `body` (absolute positions: `base` is where `body` starts in the program).
fn let_bindings(body: &str, base: usize) -> Vec<(usize, String)> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i = skip_string(b, i) + 1;
            continue;
        }
        if body[i..].starts_with("let") && (i == 0 || !is_ident(b[i - 1])) && b.get(i + 3).is_some_and(|c| c.is_ascii_whitespace()) {
            // pattern: up to the first `=` or `:` (not `::`) or `;` outside brackets
            let mut depth = 0i32;
            let mut k = i + 3;
            while k < b.len() {
                match b[k] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => depth -= 1,
                    b'=' | b';' if depth == 0 => break,
                    b':' if depth == 0 && b.get(k + 1) != Some(&b':') && b[k - 1] != b':' => break,
                    _ => {}
                }
                k += 1;
            }
            let mut j = i + 3;
            while j < k {
                if b[j].is_ascii_alphabetic() || b[j] == b'_' {
                    let st = j;
                    while j < k && is_ident(b[j]) {
                        j += 1;
                    }
                    let word = &body[st..j];
                    let next = body[j..k].trim_start().chars().next();
                    let path_like = matches!(next, Some('(') | Some('{') | Some(':')) || word.starts_with(|c: char| c.is_uppercase());
                    if !KEYWORDS.contains(&word) && !path_like && b[st - 1] != b'.' {
                        out.push((base + st, word.to_string()));
                    }
                } else {
                    j += 1;
                }
            }
            i = k;
            continue;
        }
        i += 1;
    }
    out
}

/// Innermost `{ .. }` that contains `pos`.
fn enclosing_block(src: &str, pos: usize) -> Option<(usize, usize)> {
    let b = src.as_bytes();
    let mut stack: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => i = skip_string(b, i),
            b'{' => stack.push(i),
            b'}' => {
                let open = stack.pop()?;
                if open < pos && pos < i {
                    return Some((open, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// End (just after the `;`) of the `let` statement whose pattern contains `pos`.
fn let_stmt_end(src: &str, pos: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut k = pos;
    while k < b.len() {
        match b[k] {
            b'"' => k = skip_string(b, k),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b';' if depth == 0 => return Some(k + 1),
            _ => {}
        }
        if depth < 0 {
            // `pos` sits inside a tuple or slice pattern: its closing bracket is not the end of the block
            if b[k] == b'}' {
                return None;
            }
            depth = 0;
        }
        k += 1;
    }
    None
}

/// `src` with every occurrence of the local `name` in `scope` renamed; format strings may mention it as `{name}`.
pub(crate) fn rename_in_scope(src: &str, scope: (usize, usize), skip: Option<usize>, name: &str, new: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    out.push_str(&src[..scope.0]);
    let mut i = scope.0;
    while i < scope.1 {
        if b[i] == b'"' {
            let e = skip_string(b, i).min(scope.1.saturating_sub(1));
            let lit = &src[i..=e];
            out.push_str(&lit.replace(&format!("{{{name}}}"), &format!("{{{new}}}")).replace(&format!("{{{name}:"), &format!("{{{new}:")));
            i = e + 1;
            continue;
        }
        let is_word = src[i..].starts_with(name) && (i == 0 || !is_ident(b[i - 1])) && b.get(i + name.len()).is_none_or(|&c| !is_ident(c));
        let renamed = is_word && Some(i) != skip && {
            let before = src[..i].trim_end().bytes().last();
            let rest = src[i + name.len()..].trim_start();
            before != Some(b'.') && !(before == Some(b':') && src[..i].trim_end().ends_with("::")) && !rest.starts_with("::") && !rest.starts_with('!')
        };
        if renamed {
            out.push_str(new);
            i += name.len();
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out.push_str(&src[scope.1..]);
    out
}

/// A local variable or parameter named like an item would be captured or shadowed the moment a body is pasted somewhere else:
/// rename those locals first. The scope of a `let` binding is the rest of its block up to the next binding of the same name; a
/// parameter's is the whole body. (The compiler cannot find the uses: they silently resolve to the item of the same name.)
pub fn uniquify_locals(it: &dyn Oracle, src: &str, rustc: &[String]) -> String {
    let items = item_ranges(src);
    let item_names: std::collections::HashSet<String> = items.iter().filter(|i| matches!(i.kw.as_str(), "fn" | "const" | "static" | "struct" | "enum" | "union" | "trait" | "mod" | "type")).filter_map(|i| i.name_in(src).map(String::from)).collect();
    // (name position, scope, name)
    let mut spots: Vec<(usize, (usize, usize), String)> = Vec::new();
    for def in defs_of(src) {
        let body = (def.item.header_end + 1, def.item.end - 1);
        spots.extend(def.params.iter().map(|p| (p.pos, body, p.name.clone())));
        for (pos, name) in let_bindings(&def.body, body.0) {
            let (Some(stmt_end), Some((_, block_end))) = (let_stmt_end(src, pos), enclosing_block(src, pos)) else { continue };
            // the next binding of the same name in the same block ends the scope (its right-hand side still belongs to this one)
            let next = let_bindings(&src[stmt_end..block_end], stmt_end)
                .into_iter()
                .find(|(p, n)| *n == name && enclosing_block(src, *p) == enclosing_block(src, pos));
            let end = next.as_ref().and_then(|(p, _)| let_stmt_end(src, *p)).unwrap_or(block_end);
            spots.push((pos, (stmt_end, end), name));
        }
    }
    spots.retain(|(_, _, n)| item_names.contains(n));
    spots.sort_by(|a, b| a.0.cmp(&b.0));
    spots.dedup_by(|a, b| a.0 == b.0);
    if spots.is_empty() || spots.len() > 80 {
        return src.to_string();
    }
    let mut cur = src.to_string();
    let mut renamed = 0;
    // from the back: a rename only changes text behind its own declaration
    for (pos, scope, name) in spots.into_iter().rev() {
        let new = fresh_name(&cur, &format!("{name}_l"));
        let skip = next_binding_skip(&cur, scope, &name);
        let renamed_scope = rename_in_scope(&cur, scope, skip, &name, &new);
        let cand = format!("{}{}{}", &renamed_scope[..pos], new, &renamed_scope[pos + name.len()..]);
        let compiles = crate::diag::compile_diags(rustc, &it.work_dir(), &cand, &["-A", "warnings"]).is_some_and(|d| d.iter().all(|d| !d.error));
        if compiles {
            cur = cand;
            renamed += 1;
        }
    }
    if renamed > 0 && it.check(&cur) {
        info!("renamed {renamed} local variable(s) and parameter(s) that have the name of an item");
        crate::control::rebase_checkpoint(&cur);
        cur
    } else {
        src.to_string()
    }
}

/// Position of the name token of the binding that ends `scope` (it must keep its name), if the scope ends at one.
fn next_binding_skip(src: &str, scope: (usize, usize), name: &str) -> Option<usize> {
    let region = &src[scope.0..scope.1];
    let last_let = region.rfind("let ")?;
    let found = let_bindings(&region[last_let..], scope.0 + last_let).into_iter().find(|(_, n)| n == name)?;
    // only when that `let` is the one that closed the scope (it sits at the end of the scope)
    let end = let_stmt_end(src, found.0)?;
    (end == scope.1).then_some(found.0)
}

/// Inline calls function by function (smallest bodies first), `par_checks()` functions tried at once.
pub fn inline_calls(it: &dyn Oracle, src: &str, rustc: &[String], deadline: Instant) -> String {
    let mut cur = if rustc.is_empty() { src.to_string() } else { uniquify_names(it, src, rustc) };
    if !rustc.is_empty() && cur.lines().count() <= 600 {
        cur = uniquify_locals(it, &cur, rustc);
    }
    let width = par_checks();
    for round in 1..=6 {
        if expired(deadline) {
            break;
        }
        let before = cur.clone();
        let mut defs: Vec<FnDef> = defs_of(&cur);
        defs.sort_by_key(|d| d.body.len());
        // a name defined twice cannot be told apart at a call site
        let names: Vec<String> = defs
            .iter()
            .filter(|d| d.name != "main" && defs.iter().filter(|o| o.name == d.name).count() == 1)
            // a body pasted at many call sites makes the program bigger, not simpler
            .filter(|d| {
                let sites = find_sites(&cur, d).len();
                sites > 0 && (sites - 1) * d.body.len() <= MAX_GROWTH
            })
            .map(|d| d.name.clone())
            .collect();
        let mut done = 0;
        let mut pending = names;
        for (vi, v) in VARIANTS.iter().enumerate() {
            let mut failed = Vec::new();
            let mut idx = 0;
            while idx < pending.len() {
                if expired(deadline) {
                    return cur;
                }
                let batch: Vec<&String> = pending[idx..(idx + width).min(pending.len())].iter().collect();
                let defs_now = defs_of(&cur);
                let cands: Vec<Option<String>> = batch.iter().map(|n| defs_now.iter().find(|d| &d.name == *n).and_then(|d| inline_def(&cur, d, *v, None))).collect();
                let res = par_map(&cands, width, |c| c.as_ref().is_some_and(|c| it.check(c)));
                match res.iter().position(|&ok| ok) {
                    Some(p) => {
                        info!("inlined the calls of `{}` (variant {vi})", batch[p]);
                        cur = cands[p].clone().unwrap();
                        crate::control::rebase_checkpoint(&cur);
                        done += 1;
                        failed.extend(batch[..p].iter().map(|n| (*n).clone()));
                        idx += p + 1;
                    }
                    None => {
                        failed.extend(batch.iter().map(|n| (*n).clone()));
                        idx += batch.len();
                    }
                }
            }
            pending = failed;
            if pending.is_empty() {
                break;
            }
        }
        // what is left: one call site at a time
        for n in pending {
            let defs_now = defs_of(&cur);
            let Some(d) = defs_now.iter().find(|d| d.name == n) else { continue };
            let count = find_sites(&cur, d).len();
            for site in (0..count).rev() {
                if expired(deadline) {
                    return cur;
                }
                let cands: Vec<Option<String>> = VARIANTS.iter().map(|v| inline_def(&cur, d, *v, Some(site))).collect();
                let res = par_map(&cands, width, |c| c.as_ref().is_some_and(|c| it.check(c)));
                if let Some(p) = res.iter().position(|&ok| ok) {
                    info!("inlined one call of `{n}`");
                    cur = cands[p].clone().unwrap();
                    crate::control::rebase_checkpoint(&cur);
                    done += 1;
                    // the other sites moved: the next round continues
                    break;
                }
            }
        }
        info!("inlining, round {round}: {done} function(s) inlined ({} -> {} lines)", before.lines().count(), cur.lines().count());
        if cur == before {
            break;
        }
    }
    cur
}

/// `let x = EXPR;` whose name is used once (or whose value is just a name, a reference or a literal) is replaced at its uses:
/// what a pasted function call leaves behind (`let v = &r; let __s = &*v;`). Only the oracle decides whether evaluation order or
/// borrows still work out.
pub fn substitute_lets(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let width = par_checks();
    let mut cur = src.to_string();
    for _ in 0..3 {
        let before = cur.clone();
        let mut spots: Vec<usize> = Vec::new();
        for def in all_defs_of(&cur) {
            spots.extend(simple_lets(&def.body, def.item.header_end + 1));
        }
        spots.sort();
        spots.dedup();
        let mut idx = spots.len();
        while idx > 0 {
            if expired(deadline) {
                return cur;
            }
            let batch: Vec<usize> = spots[idx.saturating_sub(width)..idx].iter().rev().copied().collect();
            let cands: Vec<Option<String>> = batch.iter().map(|&p| substitute_one(&cur, p)).collect();
            let res = par_map(&cands, width, |c| c.as_ref().is_some_and(|c| it.check(c)));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    cur = cands[p].clone().unwrap();
                    idx -= p + 1;
                }
                None => idx -= batch.len(),
            }
        }
        if cur == before {
            break;
        }
        info!("let substitution: {} -> {} lines", before.lines().count(), cur.lines().count());
    }
    cur
}

/// Positions of `let` keywords of `let NAME = EXPR;` / `let NAME: T = EXPR;` statements (no `mut`, no patterns).
fn simple_lets(body: &str, base: usize) -> Vec<usize> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i = skip_string(b, i) + 1;
            continue;
        }
        if body[i..].starts_with("let ") && (i == 0 || !is_ident(b[i - 1])) {
            let n = skip_ws(b, i + 4);
            let mut e = n;
            while e < b.len() && is_ident(b[e]) {
                e += 1;
            }
            let word = &body[n..e];
            if e > n && word != "mut" && word != "_" && !word.starts_with(|c: char| c.is_uppercase()) {
                out.push(base + i);
            }
        }
        i += 1;
    }
    out
}

fn substitute_one(src: &str, let_pos: usize) -> Option<String> {
    let b = src.as_bytes();
    let n = skip_ws(b, let_pos + 4);
    let mut e = n;
    while e < b.len() && is_ident(b[e]) {
        e += 1;
    }
    let name = &src[n..e];
    let end = let_stmt_end(src, n)?;
    let stmt = &src[let_pos..end];
    // `= EXPR;` after an optional type
    let eq = {
        let mut depth = 0i32;
        let mut found = None;
        for (k, c) in stmt.bytes().enumerate().skip(e - let_pos) {
            match c {
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth -= 1,
                b'=' if depth <= 0 && stmt.as_bytes().get(k + 1) != Some(&b'=') && stmt.as_bytes().get(k + 1) != Some(&b'>') => {
                    found = Some(k);
                    break;
                }
                _ => {}
            }
        }
        found?
    };
    let expr = stmt[eq + 1..stmt.len() - 1].trim();
    if expr.is_empty() {
        return None;
    }
    let block_end = enclosing_block(src, n).map(|(_, c)| c)?;
    let next = let_bindings(&src[end..block_end], end).into_iter().find(|(p, nm)| nm == name && enclosing_block(src, *p) == enclosing_block(src, n));
    let scope_end = next.as_ref().and_then(|(p, _)| let_stmt_end(src, *p)).unwrap_or(block_end);
    let scope = (end, scope_end);
    let skip = next_binding_skip(src, scope, name);
    // uses: whole words that rename_in_scope would touch
    let marker = "\u{1}USE\u{1}";
    let marked = rename_in_scope(src, scope, skip, name, marker);
    let uses = marked.matches(marker).count();
    let reference = expr.strip_prefix("&mut ").is_some_and(|r| !r.is_empty() && r.bytes().all(is_ident));
    let simple = reference || expr.bytes().all(|c| is_ident(c) || matches!(c, b'&' | b'*' | b'.' | b':')) || expr.parse::<f64>().is_ok();
    if uses == 0 || (uses > 1 && !simple) || src[scope.0..scope.1].contains(&format!("{{{name}")) {
        return None;
    }
    let text = if simple && !reference { expr.to_string() } else { format!("({expr})") };
    let replaced = marked.replace(marker, &text);
    // drop the let statement itself
    Some(format!("{}{}", &replaced[..let_pos], &replaced[end..]))
}

/// Names the program declares (`fn`, `let`, `struct`, `enum`, `trait`, `mod`, `type`, `const`, `static`, parameters).
fn declared_names(src: &str) -> Vec<String> {
    let toks = crate::tokens::tokenize(src);
    let word = |k: usize| &src[toks[k].0..toks[k].1];
    let mut names: Vec<String> = Vec::new();
    let mut k = 0;
    while k < toks.len() {
        match word(k) {
            "fn" | "struct" | "enum" | "trait" | "mod" | "type" | "const" | "static" | "union" | "let" if k + 1 < toks.len() => {
                let mut n = k + 1;
                if word(n) == "mut" && n + 1 < toks.len() {
                    n += 1;
                }
                let w = word(n);
                if w.bytes().next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') && !KEYWORDS.contains(&w) {
                    names.push(w.to_string());
                }
                // parameters: `name:` inside the parentheses of a fn header
                if word(k) == "fn" {
                    if let Some(open) = (n..toks.len().min(n + 12)).find(|&j| word(j) == "(") {
                        let mut depth = 0;
                        for j in open..toks.len() {
                            match word(j) {
                                "(" => depth += 1,
                                ")" => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                ":" if depth == 1 && j > 0 => {
                                    let p = word(j - 1);
                                    if p.bytes().next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') && p != "self" {
                                        names.push(p.to_string());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        k += 1;
    }
    names.sort();
    names.dedup();
    names
}

/// Declared names replaced by single letters, one name at a time (Shrink Ray's `normalize_identifiers`): shorter reproducers read
/// better in an issue. Only what the oracle still accepts is kept.
pub fn shorten_names(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let width = par_checks();
    let mut cur = src.to_string();
    let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 0..60 {
        if expired(deadline) {
            break;
        }
        let words: std::collections::HashSet<&str> = cur.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        let mut free = ('a'..='z').chain('A'..='Z').map(|c| c.to_string()).filter(|c| !words.contains(c.as_str()));
        let mut names: Vec<String> = declared_names(&cur).into_iter().filter(|n| n.len() > 1 && n != "main" && !tried.contains(n)).collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        let batch: Vec<(String, String)> = names.into_iter().take(width).filter_map(|n| free.next().map(|f| (n, f))).collect();
        if batch.is_empty() {
            break;
        }
        let cands: Vec<String> = batch.iter().map(|(n, f)| rename_in_scope(&cur, (0, cur.len()), None, n, f)).collect();
        let res = par_map(&cands, width, |c| it.check(c));
        for (n, _) in &batch {
            tried.insert(n.clone());
        }
        // letters of a rejected candidate are free again next round; of an accepted one the first success counts, the others retry
        if let Some(p) = res.iter().position(|&ok| ok) {
            cur = cands[p].clone();
            for (n, _) in batch.iter().skip(p + 1) {
                tried.remove(n);
            }
        }
    }
    cur
}
