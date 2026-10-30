//! Small replacements in the style of C-Vise's `peep`/`balanced` and Shrink Ray's `replace_identifiers_with_zero`: every
//! candidate is one edit of the text, kept only when the oracle still accepts the program. Together they turn dependencies into
//! constants, which lets the definitions go in a later pass.

use crate::exprs::greedy_edits;
use crate::oracle::Oracle;
use crate::scan::{is_ident, matching, postfix_start, top_args};
use crate::tokens::tokenize;
use log::info;
use std::time::Instant;

type Edit = (usize, usize, String);

const KEYWORDS: [&str; 40] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "macro_rules", "union",
];

/// Identifiers used as values (not after `.` or `::`, not macro names, not followed by `:` as a field or `::` as a path) -> `0`.
fn idents_to_zero(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let toks = tokenize(src);
    let mut out = Vec::new();
    for (k, &(s, e)) in toks.iter().enumerate() {
        let word = &src[s..e];
        if !word.as_bytes()[0].is_ascii_alphabetic() && word.as_bytes()[0] != b'_' || KEYWORDS.contains(&word) {
            continue;
        }
        let prev = k.checked_sub(1).map(|p| &src[toks[p].0..toks[p].1]);
        let next = toks.get(k + 1).map(|&(a, z)| &src[a..z]);
        if matches!(prev, Some("." | "::" | "fn" | "struct" | "enum" | "trait" | "mod" | "type" | "let" | "mut" | "impl" | "for" | "'")) {
            continue;
        }
        if matches!(next, Some("::" | "!" | ":" | "<")) || word.starts_with(|c: char| c.is_uppercase()) {
            continue;
        }
        if b.get(e) == Some(&b'!') {
            continue;
        }
        out.push((s, e, "0".to_string()));
    }
    out
}

/// `(..)` groups that are plain parenthesised expressions (not a call or a parameter list) and `{..}` expression blocks -> `0`.
fn groups_to_zero(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
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
            b'(' | b'{' => {
                let before = src[..i].trim_end().bytes().last();
                let expr_position = matches!(before, Some(b'=') | Some(b'(') | Some(b',') | Some(b'+') | Some(b'-') | Some(b'*') | Some(b'/') | Some(b'%') | Some(b'&') | Some(b'|') | Some(b'^') | Some(b'<') | Some(b'>') | Some(b'?'));
                if expr_position && !src[..i].trim_end().ends_with("->") {
                    if let Some(close) = matching(b, i) {
                        out.push((i, close + 1, "0".to_string()));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

const BINOPS: [&str; 16] = ["||", "&&", "==", "!=", "<=", ">=", "<<", ">>", "+", "-", "*", "/", "%", "&", "|", "^"];

/// Operands of the binary operators of every expression span (parenthesised groups, call arguments, right-hand sides of `let`):
/// the span is replaced by one of its operands.
fn binary_operands(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, &c) in b.iter().enumerate() {
        if matches!(c, b'(' | b'[') {
            if let Some(close) = matching(b, i) {
                spans.extend(top_args(src, i, close));
            }
        }
    }
    let mut from = 0;
    while let Some(p) = src[from..].find("let ") {
        let at = from + p;
        from = at + 4;
        let mut depth = 0i32;
        let mut eq = None;
        let mut k = at + 4;
        while k < b.len() {
            match b[k] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'=' if depth == 0 && eq.is_none() && b.get(k + 1) != Some(&b'=') && !matches!(b[k - 1], b'<' | b'>' | b'!') => eq = Some(k),
                b';' if depth == 0 => {
                    if let Some(e) = eq {
                        spans.push((e + 1, k));
                    }
                    break;
                }
                _ => {}
            }
            if depth < 0 {
                break;
            }
            k += 1;
        }
    }
    let mut out = Vec::new();
    for (s, e) in spans {
        let text = &src[s..e];
        let tb = text.as_bytes();
        let mut depth = 0i32;
        let mut cuts: Vec<(usize, usize)> = Vec::new(); // operator ranges, relative
        let mut i = 0;
        while i < tb.len() {
            match tb[i] {
                b'"' => {
                    i += 1;
                    while i < tb.len() && tb[i] != b'"' {
                        if tb[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                _ if depth == 0 => {
                    if let Some(op) = BINOPS.iter().find(|o| text[i..].starts_with(**o)) {
                        let before = text[..i].trim_end().bytes().last();
                        let unary = before.is_none() || matches!(before, Some(b'(' | b',' | b'=' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'<' | b'>' | b'!' | b':'));
                        let arrow = (*op == "-" && text[i..].starts_with("->")) || (*op == "|" && before.is_none());
                        let assign = text[i + op.len()..].starts_with('=') && !matches!(*op, "==" | "!=" | "<=" | ">=");
                        if !unary && !arrow && !assign {
                            cuts.push((i, i + op.len()));
                        }
                        i += op.len() - 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if cuts.is_empty() {
            continue;
        }
        // `<` and `>` are not in BINOPS (generics), a space on both sides makes them comparisons
        let compares = cuts.iter().any(|&(a, z)| matches!(&text[a..z], "==" | "!=" | "<=" | ">=" | "&&" | "||")) || text.contains(" < ") || text.contains(" > ");
        if compares {
            out.push((s, e, " true ".to_string()));
            out.push((s, e, " false ".to_string()));
        }
        let mut bounds = vec![0usize];
        for &(a, z) in &cuts {
            bounds.push(a);
            bounds.push(z);
        }
        bounds.push(text.len());
        for pair in bounds.chunks(2) {
            let operand = text[pair[0]..pair[1]].trim();
            if !operand.is_empty() {
                out.push((s, e, format!(" ({operand}) ")));
            }
        }
    }
    out
}

/// Function-call expressions `name(args)` and method calls replaced by a plain value.
fn calls_to_values(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let toks = tokenize(src);
    let mut out = Vec::new();
    for (k, &(s, e)) in toks.iter().enumerate() {
        let word = &src[s..e];
        if !(word.as_bytes()[0].is_ascii_alphabetic() || word.as_bytes()[0] == b'_') || KEYWORDS.contains(&word) || b.get(e) != Some(&b'(') {
            continue;
        }
        let prev = k.checked_sub(1).map(|p| &src[toks[p].0..toks[p].1]);
        if matches!(prev, Some("fn" | "." | "struct" | "enum" | "impl" | "trait")) {
            continue;
        }
        if let Some(close) = matching(b, e) {
            let values: &[&str] = if crate::control::effort() > 0 { &["0", "Default::default()", "1", "false", "()"] } else { &["0", "Default::default()"] };
            for value in values {
                out.push((s, close + 1, (*value).to_string()));
            }
        }
    }
    out
}

/// Every argument of every call that is more than a literal or a name, replaced by a small literal (`f(a.g(1).0 as i8)` -> `f(1)`):
/// a call is rarely needed with exactly the argument it was generated with.
fn args_to_literals(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let toks = tokenize(src);
    let mut out = Vec::new();
    for (k, &(s, e)) in toks.iter().enumerate() {
        let word = &src[s..e];
        if !(word.as_bytes()[0].is_ascii_alphabetic() || word.as_bytes()[0] == b'_') || KEYWORDS.contains(&word) || b.get(e) != Some(&b'(') {
            continue;
        }
        let prev = k.checked_sub(1).map(|p| &src[toks[p].0..toks[p].1]);
        if matches!(prev, Some("fn" | "struct" | "enum" | "impl" | "trait")) {
            continue;
        }
        let Some(close) = matching(b, e) else { continue };
        for (a, z) in top_args(src, e, close) {
            let arg = src[a..z].trim();
            if arg.len() < 3 || arg.bytes().all(|c| c.is_ascii_digit() || c == b'_') {
                continue;
            }
            let lead = src[a..z].len() - src[a..z].trim_start().len();
            for value in ["0", "1"] {
                out.push((a + lead, a + lead + arg.len(), value.to_string()));
            }
        }
    }
    out
}

/// `println!("{} {}", a, b)` -> `println!("{}", b)`: one placeholder with its argument at a time (plain `{}`-style placeholders only).
fn format_arguments_dropped(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for mac in ["println!(", "print!(", "eprintln!(", "eprint!(", "format!("] {
        for (at, _) in src.match_indices(mac) {
            if at > 0 && is_ident(b[at - 1]) {
                continue;
            }
            let open = at + mac.len() - 1;
            let Some(close) = matching(b, open) else { continue };
            let args = top_args(src, open, close);
            let Some(&(ls, le)) = args.first() else { continue };
            let lit = src[ls..le].trim();
            if !lit.starts_with('"') || !lit.ends_with('"') || args.len() < 2 {
                continue;
            }
            let lit_start = ls + (src[ls..le].len() - src[ls..le].trim_start().len());
            let body = &lit[1..lit.len() - 1];
            let mut spans = Vec::new();
            let mut k = 0;
            let bytes = body.as_bytes();
            while k < bytes.len() {
                match bytes[k] {
                    b'{' if bytes.get(k + 1) == Some(&b'{') => k += 2,
                    b'}' if bytes.get(k + 1) == Some(&b'}') => k += 2,
                    b'{' => match body[k..].find('}') {
                        Some(e) => {
                            let inner = &body[k + 1..k + e];
                            if inner.is_empty() || inner.starts_with(':') {
                                spans.push((k, k + e + 1));
                            } else {
                                spans.clear();
                                break;
                            }
                            k += e + 1;
                        }
                        None => break,
                    },
                    _ => k += 1,
                }
            }
            if spans.len() != args.len() - 1 {
                continue;
            }
            for (i, &(ps, pe)) in spans.iter().enumerate() {
                let (head, rest) = (&body[..ps], &body[pe..]);
                let text = match head.strip_suffix(' ') {
                    Some(h) => format!("\"{h}{rest}\""),
                    None => format!("\"{head}{}\"", rest.strip_prefix(' ').unwrap_or(rest)),
                };
                let mut kept: Vec<&str> = Vec::new();
                for (j, &(s2, e2)) in args.iter().skip(1).enumerate() {
                    if j != i {
                        kept.push(src[s2..e2].trim());
                    }
                }
                let mut all = text;
                for a in kept {
                    all.push_str(", ");
                    all.push_str(a);
                }
                out.push((lit_start, close, all));
            }
        }
    }
    out
}

/// `x.checked_add(y).unwrap_or(z)`: an Option or Result producing call together with the unwrap that follows it is dropped (`x`), or
/// the whole expression is replaced by one of the arguments (`y`, `z`). Dropping only one of the two never type checks.
fn option_pairs(src: &str) -> Vec<Edit> {
    const UNWRAPS: [&str; 5] = [".unwrap_or(", ".unwrap()", ".expect(", ".unwrap_or_default()", ".unwrap_or_else("];
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (dot, _) in src.match_indices('.') {
        if !b.get(dot + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') {
            continue;
        }
        let mut j = dot + 1;
        while j < b.len() && is_ident(b[j]) {
            j += 1;
        }
        if b.get(j) != Some(&b'(') {
            continue;
        }
        let Some(close) = matching(b, j) else { continue };
        let Some(un) = UNWRAPS.iter().find(|u| src[close + 1..].starts_with(**u)) else { continue };
        let un_open = close + 1 + un.len() - 1;
        let (un_open, un_end) = if *un == ".unwrap()" || *un == ".unwrap_or_default()" { (un_open, un_open + 1) } else { match matching(b, un_open) { Some(c) => (un_open, c + 1), None => continue } };
        let rs = postfix_start(b, dot);
        out.push((dot, un_end, String::new()));
        let mut args = top_args(src, j, close);
        if un_end > un_open + 1 && *un != ".unwrap()" && *un != ".unwrap_or_default()" {
            args.extend(top_args(src, un_open, un_end - 1));
        }
        for (s, e) in args {
            let arg = src[s..e].trim();
            if !arg.is_empty() {
                out.push((rs, un_end, format!("({arg})")));
            }
        }
    }
    out
}

/// `*(&x)` -> `(x)`: what is left after a let that holds a reference was substituted into its use.
fn deref_of_borrow(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (at, _) in src.match_indices("*(&") {
        if at > 0 && (is_ident(b[at - 1]) || matches!(b[at - 1], b')' | b']')) {
            continue;
        }
        let Some(close) = matching(b, at + 1) else { continue };
        let inner = src[at + 3..close].trim_start_matches("mut ");
        out.push((at, close + 1, format!("({inner})")));
    }
    // `*&mut name` / `*&name` without parentheses
    for (at, _) in src.match_indices("*&") {
        if at > 0 && (is_ident(b[at - 1]) || matches!(b[at - 1], b')' | b']')) {
            continue;
        }
        let rest = src[at + 2..].trim_start_matches("mut ");
        let name_start = src.len() - rest.len();
        let name_len = rest.bytes().take_while(|c| is_ident(*c)).count();
        if name_len > 0 {
            out.push((at, name_start + name_len, src[name_start..name_start + name_len].to_string()));
        }
    }
    out
}

/// Occurrences of `pattern` replaced by `with` (`let mut ` -> `let `, `&mut ` -> `&`, `+=` -> `=`).
fn replacements(src: &str, pattern: &str, with: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    src.match_indices(pattern).filter(|(i, _)| !is_ident(pattern.as_bytes()[0]) || *i == 0 || !is_ident(b[i - 1])).map(|(i, p)| (i, i + p.len(), with.to_string())).collect()
}

fn keyword_weakening(src: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    for (pat, with) in [("let mut ", "let "), ("() = ", ""), ("&mut ", "&"), ("unsafe {", "{"), ("pub ", ""), ("pub(crate) ", ""), ("+=", "="), ("-=", "="), ("*=", "="), ("/=", "="), ("%=", "="), ("^=", "="), ("|=", "="), ("&=", "="), ("<<=", "="), (">>=", "="), ("inline(never)", "inline"), ("#[inline(always)]", "")] {
        out.extend(replacements(src, pat, with));
    }
    out
}

/// `for x in [] { .. }` never runs: the loop goes whole (the compiler does not report it as dead code, and with all of them gone in
/// one oracle call a generated harness loses most of its cases at once).
fn empty_loops(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (at, _) in src.match_indices("for") {
        if (at > 0 && is_ident(b[at - 1])) || b.get(at + 3).is_none_or(|&c| is_ident(c)) {
            continue;
        }
        let Some(p) = src[at..].find(" in ") else { continue };
        let head = &src[at + 3..at + p];
        if head.contains(['{', '}', ';']) {
            continue;
        }
        let iter = crate::scan::skip_ws(b, at + p + 4);
        if b.get(iter) != Some(&b'[') {
            continue;
        }
        let Some(arr) = matching(b, iter) else { continue };
        if !src[iter + 1..arr].trim().is_empty() {
            continue;
        }
        let open = crate::scan::skip_ws(b, arr + 1);
        if b.get(open) != Some(&b'{') {
            continue;
        }
        if let Some(close) = matching(b, open) {
            out.push((at, close + 1, String::new()));
        }
    }
    out
}

/// `let h; .. h = x;` -> `.. let h = x;`: the one assignment at the declaration's own nesting level becomes the initialiser.
fn deferred_lets(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (at, _) in src.match_indices("let ") {
        if at > 0 && is_ident(b[at - 1]) {
            continue;
        }
        let mut name_start = crate::scan::skip_ws(b, at + 4);
        let mut mutable = false;
        if src[name_start..].starts_with("mut ") {
            mutable = true;
            name_start = crate::scan::skip_ws(b, name_start + 4);
        }
        let mut name_end = name_start;
        while name_end < b.len() && is_ident(b[name_end]) {
            name_end += 1;
        }
        let name = &src[name_start..name_end];
        if name.is_empty() {
            continue;
        }
        let Some(semi) = src[name_end..].find(';').map(|p| name_end + p) else { continue };
        let annotation = src[name_end..semi].trim();
        if !(annotation.is_empty() || (annotation.starts_with(':') && !annotation.contains('='))) {
            continue;
        }
        let mut depth = 0i32;
        let mut k = semi + 1;
        let mut found: Option<usize> = None;
        let mut statement_start = true;
        while k < b.len() && depth >= 0 {
            match b[k] {
                b'"' => {
                    k += 1;
                    while k < b.len() && b[k] != b'"' {
                        k += if b[k] == b'\\' { 2 } else { 1 };
                    }
                    statement_start = false;
                }
                b'{' | b'(' | b'[' => {
                    depth += 1;
                    statement_start = b[k] == b'{';
                }
                b'}' | b')' | b']' => {
                    depth -= 1;
                    statement_start = b[k] == b'}';
                }
                b';' => statement_start = true,
                c if c.is_ascii_whitespace() => {}
                _ => {
                    if statement_start && depth == 0 && src[k..].starts_with(name) && !b.get(k + name.len()).is_some_and(|&c| is_ident(c)) {
                        let after = crate::scan::skip_ws(b, k + name.len());
                        if b.get(after) == Some(&b'=') && b.get(after + 1) != Some(&b'=') {
                            if found.is_some() {
                                found = None;
                                break;
                            }
                            found = Some(k);
                        }
                    }
                    statement_start = false;
                }
            }
            k += 1;
        }
        let Some(assign) = found else { continue };
        let kept = format!("{}let {}{}{}", &src[semi + 1..assign], if mutable { "mut " } else { "" }, name, annotation);
        out.push((at, assign + name.len(), kept));
    }
    out
}

/// `extern "win64" fn` -> `extern "C" fn` -> `fn`: the calling convention a generator picked is rarely what the bug needs.
fn abi_weakening(src: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (at, _) in src.match_indices("extern") {
        if at > 0 && is_ident(b[at - 1]) {
            continue;
        }
        let lit = crate::scan::skip_ws(b, at + 6);
        if b.get(lit) != Some(&b'"') {
            continue;
        }
        let Some(close) = src[lit + 1..].find('"').map(|p| lit + 1 + p) else { continue };
        let after = crate::scan::skip_ws(b, close + 1);
        if !src[after..].starts_with("fn") || b.get(after + 2).is_some_and(|c| is_ident(*c)) {
            continue;
        }
        if &src[lit..=close] != "\"C\"" {
            out.push((at, close + 1, "extern \"C\"".to_string()));
        }
        out.push((at, after, String::new()));
    }
    out
}

fn int_literals(src: &str) -> Vec<(usize, usize, u128, String)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
        } else if b[i].is_ascii_digit() && (i == 0 || !(is_ident(b[i - 1]) || b[i - 1] == b'.')) {
            let s = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let digits: String = src[s..i].chars().filter(|c| *c != '_').collect();
            let hex = digits == "0" && matches!(b.get(i), Some(b'x' | b'b' | b'o'));
            let float = b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) || matches!(b.get(i), Some(b'e' | b'E'));
            let mut e = i;
            while e < b.len() && is_ident(b[e]) {
                e += 1;
            }
            let suffix = &src[i..e];
            if !hex && !float && (suffix.is_empty() || suffix.starts_with('u') || suffix.starts_with('i')) {
                if let Ok(v) = digits.parse::<u128>() {
                    out.push((s, e, v, suffix.to_string()));
                }
            }
            i = e.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Integer literals made smaller: halved, divided by ten, one less (Shrink Ray's integer reduction; 0 and 1 are tried elsewhere).
fn smaller_integers(src: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    for (s, e, v, suffix) in int_literals(src) {
        for cand in [v / 2, v / 10, v.saturating_sub(1)] {
            if cand > 1 && cand < v {
                out.push((s, e, format!("{cand}{suffix}")));
            }
        }
        if suffix.starts_with('u') && v > 1 {
            // the all-ones value is what a long literal usually stands for
            let max = format!("{suffix}::MAX");
            if max.len() < e - s {
                out.push((s, e, max));
            }
        }
    }
    out
}

/// `32u8 as u32` -> `32u32`, `0 as i32` -> `0i32`: a cast of a literal is a literal of the target type.
fn folded_casts(src: &str) -> Vec<Edit> {
    const INTS: [&str; 12] = ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize"];
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (s, e, v, _) in int_literals(src) {
        let at = crate::scan::skip_ws(b, e);
        if !src[at..].starts_with("as") || b.get(at + 2).is_none_or(|c| is_ident(*c)) {
            continue;
        }
        let ty_start = crate::scan::skip_ws(b, at + 2);
        let mut ty_end = ty_start;
        while ty_end < b.len() && is_ident(b[ty_end]) {
            ty_end += 1;
        }
        let ty = &src[ty_start..ty_end];
        if INTS.contains(&ty) {
            out.push((s, ty_end, format!("{v}{ty}")));
        }
    }
    out
}

/// `2 + 3` -> `5`: arithmetic on two literals of the same type is evaluated (Shrink Ray's `combine_expressions`).
fn folded_constants(src: &str) -> Vec<Edit> {
    let lits = int_literals(src);
    let mut out = Vec::new();
    for w in lits.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if a.3 != b.3 && !(a.3.is_empty() || b.3.is_empty()) {
            continue;
        }
        let between = src[a.1..b.0].trim();
        let comparison = match between {
            "==" => Some(a.2 == b.2),
            "!=" => Some(a.2 != b.2),
            "<" => Some(a.2 < b.2),
            "<=" => Some(a.2 <= b.2),
            ">" => Some(a.2 > b.2),
            ">=" => Some(a.2 >= b.2),
            _ => None,
        };
        if let Some(v) = comparison {
            out.push((a.0, b.1, v.to_string()));
            continue;
        }
        let value = match between {
            "+" => a.2.checked_add(b.2),
            "-" => a.2.checked_sub(b.2),
            "*" => a.2.checked_mul(b.2),
            "/" if b.2 != 0 => Some(a.2 / b.2),
            "%" if b.2 != 0 => Some(a.2 % b.2),
            "&" => Some(a.2 & b.2),
            "|" => Some(a.2 | b.2),
            "^" => Some(a.2 ^ b.2),
            "<<" if b.2 < 128 => a.2.checked_shl(b.2 as u32),
            ">>" if b.2 < 128 => Some(a.2 >> b.2),
            _ => None,
        };
        if let Some(v) = value {
            let suffix = if a.3.is_empty() { &b.3 } else { &a.3 };
            out.push((a.0, b.1, format!("{v}{suffix}")));
        }
    }
    out
}

/// The whole set, one category after the other.
pub fn peep(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    let passes: [(&str, fn(&str) -> Vec<Edit>); 15] = [
        ("keywords and compound assignments weakened", keyword_weakening),
        ("casts of literals folded", folded_casts),
        ("deferred initialisers moved into the let", deferred_lets),
        ("loops over empty arrays removed", empty_loops),
        ("calling conventions weakened", abi_weakening),
        ("binary operators: one operand kept", binary_operands),
        ("parenthesised groups and blocks replaced by 0", groups_to_zero),
        ("calls replaced by values", calls_to_values),
        ("option and result pairs dropped", option_pairs),
        ("dereferenced borrows cancelled", deref_of_borrow),
        ("format arguments dropped", format_arguments_dropped),
        ("call arguments replaced by literals", args_to_literals),
        ("identifiers replaced by 0", idents_to_zero),
        ("constant arithmetic folded", folded_constants),
        ("integer literals made smaller", smaller_integers),
    ];
    for (what, make) in passes {
        let before = cur.len();
        let edits = make(&cur);
        if edits.is_empty() {
            continue;
        }
        cur = greedy_edits(it, &cur, edits, deadline);
        if cur.len() != before {
            info!("{what}: {before} -> {} bytes", cur.len());
        }
    }
    cur
}
