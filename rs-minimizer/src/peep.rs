//! Small replacements in the style of C-Vise's `peep`/`balanced` and Shrink Ray's `replace_identifiers_with_zero`: every
//! candidate is one edit of the text, kept only when the oracle still accepts the program. Together they turn dependencies into
//! constants, which lets the definitions go in a later pass.

use crate::exprs::greedy_edits;
use crate::oracle::Oracle;
use crate::scan::{is_ident, matching, top_args};
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

/// Occurrences of `pattern` replaced by `with` (`let mut ` -> `let `, `&mut ` -> `&`, `+=` -> `=`).
fn replacements(src: &str, pattern: &str, with: &str) -> Vec<Edit> {
    let b = src.as_bytes();
    src.match_indices(pattern).filter(|(i, _)| !is_ident(pattern.as_bytes()[0]) || *i == 0 || !is_ident(b[i - 1])).map(|(i, p)| (i, i + p.len(), with.to_string())).collect()
}

fn keyword_weakening(src: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    for (pat, with) in [("let mut ", "let "), ("&mut ", "&"), ("unsafe {", "{"), ("pub ", ""), ("pub(crate) ", ""), ("+=", "="), ("-=", "="), ("*=", "="), ("/=", "="), ("%=", "="), ("^=", "="), ("|=", "="), ("&=", "="), ("<<=", "="), (">>=", "="), ("inline(never)", "inline"), ("#[inline(always)]", "")] {
        out.extend(replacements(src, pat, with));
    }
    out
}

/// The whole set, one category after the other.
pub fn peep(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    let passes: [(&str, fn(&str) -> Vec<Edit>); 5] = [
        ("keywords and compound assignments weakened", keyword_weakening),
        ("binary operators: one operand kept", binary_operands),
        ("parenthesised groups and blocks replaced by 0", groups_to_zero),
        ("calls replaced by values", calls_to_values),
        ("identifiers replaced by 0", idents_to_zero),
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
