//! Token-level reduction (C-Vise's `clex`): runs of 1..8 consecutive tokens deleted. Finds what lines and items cannot: a bound, a
//! `mut`, a generic argument, one operand of an expression.

use crate::control::{expired, par_checks};
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::scan::is_ident;
use log::info;
use std::time::Instant;

const MULTI: [&str; 22] = ["..=", "<<=", ">>=", "::", "->", "=>", "..", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "+=", "-=", "*=", "/=", "%=", "^=", "&="];

/// `(start, end)` byte ranges of the tokens of `src` (comments are not tokens; they are expected to be gone).
pub fn tokenize(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if is_ident(c) {
            let s = i;
            while i < b.len() && is_ident(b[i]) {
                i += 1;
            }
            out.push((s, i));
        } else if c == b'"' {
            let s = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            out.push((s, i));
        } else if c == b'\'' {
            // lifetime `'a` or char literal `'x'` / `'\n'`
            let s = i;
            if b.get(i + 1) == Some(&b'\\') {
                i += 2;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i = (i + 1).min(b.len());
            } else if b.get(i + 2) == Some(&b'\'') {
                i += 3;
            } else {
                i += 1;
                while i < b.len() && is_ident(b[i]) {
                    i += 1;
                }
            }
            out.push((s, i));
        } else {
            let s = i;
            let len = MULTI.iter().find(|m| src[i..].starts_with(**m)).map(|m| m.len()).unwrap_or_else(|| src[i..].chars().next().map(char::len_utf8).unwrap_or(1));
            i += len;
            out.push((s, i));
        }
    }
    out
}

/// Do the brackets nest properly? Candidates that cut a bracket without its partner never compile: the compiler need not be asked.
pub fn balanced(src: &str) -> bool {
    let mut stack: Vec<u8> = Vec::new();
    for (s, e) in tokenize(src) {
        if e - s != 1 {
            continue;
        }
        match src.as_bytes()[s] {
            c @ (b'(' | b'[' | b'{') => stack.push(c),
            b')' if stack.pop() != Some(b'(') => return false,
            b']' if stack.pop() != Some(b'[') => return false,
            b'}' if stack.pop() != Some(b'{') => return false,
            _ => {}
        }
    }
    stack.is_empty()
}

/// `src` without the bytes of tokens `first..=last`; a space is kept when two identifiers would otherwise merge.
fn without_tokens(src: &str, toks: &[(usize, usize)], first: usize, last: usize) -> String {
    let (s, e) = (toks[first].0, toks[last].1);
    let b = src.as_bytes();
    let glue = s > 0 && e < b.len() && is_ident(b[s - 1]) && is_ident(b[e]);
    format!("{}{}{}", &src[..s], if glue { " " } else { "" }, &src[e..])
}

pub fn shrink_tokens(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    let width = par_checks();
    let mut toks = tokenize(&cur);
    if toks.len() > 4000 {
        return cur;
    }
    let check_nesting = balanced(&cur);
    let windows: &[usize] = if crate::control::effort() > 0 { &[64, 32, 16, 8, 4, 2, 1] } else { &[8, 4, 2, 1] };
    for &n in windows {
        // every window position on small programs, every other one on bigger ones
        let step = if toks.len() <= 1500 { 1 } else { (n / 2).max(1) } as isize;
        let mut i = toks.len().saturating_sub(n) as isize;
        while i >= 0 {
            if expired(deadline) {
                return cur;
            }
            let starts: Vec<isize> = (0..width as isize).map(|k| i - k * step).take_while(|&s| s >= 0).collect();
            let cands: Vec<String> = starts.iter().map(|&s| without_tokens(&cur, &toks, s as usize, (s as usize + n - 1).min(toks.len() - 1))).collect();
            let res = par_map(&cands, width, |c| !c.trim().is_empty() && (!check_nesting || balanced(c)) && it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    cur = cands[p].clone();
                    toks = tokenize(&cur);
                    i = starts[p].min(toks.len().saturating_sub(n) as isize);
                }
                None => i = starts.last().copied().unwrap_or(0) - step,
            }
        }
    }
    if cur.len() < src.len() {
        info!("tokens: {} -> {} bytes", src.len(), cur.len());
    }
    cur
}
