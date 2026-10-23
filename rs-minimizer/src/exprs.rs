//! Expression-level passes (what a human does when shrinking a generated chain by hand): literals to `0`/`1`, calls replaced by one
//! of their arguments, `.method(..)` segments dropped, `if` replaced by a branch, redundant parentheses removed.

use crate::control::{expired, par_checks};
use crate::oracle::Oracle;
use crate::par::par_map;
use crate::scan::{matching, postfix_start, top_args};
use std::time::Instant;

/// Pass 2: `let name: ty = EXPR;`  ->  `let name: ty = 0;` (or `false`)
pub fn simplify_lets(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    // the line that would replace line `i` (the right-hand side by a literal), if it is such a `let`
    let replacement = |lines: &[String], i: usize| -> Option<String> {
        let l = lines[i].trim_start();
        let rest = l.strip_prefix("let ")?;
        let (head, expr) = rest.split_once(" = ")?;
        let (_, ty) = head.split_once(": ")?;
        if !expr.trim_end().ends_with(';') || expr.trim() == "0;" {
            return None;
        }
        let lit = match ty.trim() {
            "bool" => "false",
            t if t.starts_with('f') => "0.0",
            t if t.starts_with(|c: char| c == 'i' || c == 'u') => "0",
            _ => return None,
        };
        let indent = &lines[i][..lines[i].len() - lines[i].trim_start().len()];
        Some(format!("{indent}let {head} = {lit};"))
    };
    let width = par_checks();
    let mut todo: Vec<usize> = (0..lines.len()).collect();
    // several lines are tried at once (against the same text); the first that works is kept, the rest is tried again
    while !todo.is_empty() {
        if expired(deadline) {
            break;
        }
        let batch: Vec<usize> = todo.iter().copied().filter(|&i| replacement(&lines, i).is_some()).take(width).collect();
        if batch.is_empty() {
            break;
        }
        let cands: Vec<Vec<String>> = batch
            .iter()
            .map(|&i| {
                let mut c = lines.clone();
                c[i] = replacement(&lines, i).unwrap();
                c
            })
            .collect();
        let res = par_map(&cands, width, |c| it.check(&c.join("\n")));
        let last = *batch.last().unwrap();
        match res.iter().position(|&ok| ok) {
            Some(p) => {
                lines = cands[p].clone();
                // lines up to the accepted one are done (those before it failed); the ones behind it come again
                todo.retain(|&i| i > batch[p]);
            }
            None => todo.retain(|&i| i > last),
        }
    }
    lines.join("\n")
}

/// Byte ranges of `.ident(<balanced args>)` segments (turbofish allowed).
fn call_segments(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' && i + 1 < b.len() && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_') {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                let mut depth = 0i32;
                let mut k = j;
                while k < b.len() {
                    match b[k] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        b'"' => {
                            // skip string literals
                            k += 1;
                            while k < b.len() && b[k] != b'"' {
                                if b[k] == b'\\' {
                                    k += 1;
                                }
                                k += 1;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if depth == 0 && k < b.len() {
                    out.push((i, k + 1));
                }
            }
        }
        i += 1;
    }
    out
}

/// Pass 3: delete one `.method(..)` segment at a time, from the end of the file backwards. Several
/// candidates are evaluated concurrently.
pub fn drop_calls(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    loop {
        let mut segs = call_segments(&cur);
        let mut progress = false;
        while !segs.is_empty() {
            if expired(deadline) {
                return cur;
            }
            let lo = segs.len().saturating_sub(par_checks());
            let batch: Vec<(usize, usize)> = segs[lo..].iter().rev().copied().collect();
            let cands: Vec<String> = batch.iter().map(|&(s, e)| format!("{}{}", &cur[..s], &cur[e..])).collect();
            let res = par_map(&cands, par_checks(), |c| it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let (s, _) = batch[p];
                    cur = cands[p].clone();
                    progress = true;
                    // only segments lying entirely before the removed one keep valid offsets
                    segs.retain(|&(ss, ee)| ee <= s && ss < s);
                }
                None => segs.truncate(lo),
            }
        }
        if !progress {
            return cur;
        }
    }
}

/// All single-edit simplifications of `src` as `(position, new text)`, in textual order.
/// Edits `(start, end, replacement)` of `src` (applied lazily: materialising every candidate text of a big
/// file at once needs gigabytes).
fn expr_candidates(src: &str) -> Vec<(usize, usize, String)> {
    let b = src.as_bytes();
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    let keywords = ["if", "while", "match", "for", "fn", "return", "in", "else", "let", "mut", "loop", "unsafe", "as"];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // string literals are opaque
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        // `.method(args)`: replace the whole call (receiver included) by one of its arguments
        if c == b'.' && i + 1 < b.len() && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_') {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                if let Some(close) = matching(b, j) {
                    let rs = postfix_start(b, i);
                    for (s, e) in top_args(src, j, close) {
                        let arg = src[s..e].trim();
                        out.push((rs, close + 1, format!("({arg})")));
                    }
                }
            }
            i += 1;
            continue;
        }
        // plain call `name(args)` / `Some(x)` / `bb(x)`: replace by one argument
        if (c.is_ascii_alphabetic() || c == b'_') && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.' || b[i - 1] == b':')) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
                j += 1;
            }
            let word = &src[i..j];
            if j < b.len() && b[j] == b'(' && !keywords.contains(&word) && !word.ends_with(':') {
                if let Some(close) = matching(b, j) {
                    for (s, e) in top_args(src, j, close) {
                        let arg = src[s..e].trim();
                        out.push((i, close + 1, format!("({arg})")));
                    }
                }
            }
            // `if COND { A } else { B }`  ->  `{ A }` / `{ B }`
            if word == "if" && (i < 5 || &src[i - 5..i] != "else ") {
                // the block opens at the first `{` outside brackets
                let mut depth = 0i32;
                let mut k = j;
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
                if let Some(o1) = open {
                    if let Some(c1) = matching(b, o1) {
                        let rest = &src[c1 + 1..];
                        let tail = rest.trim_start();
                        if tail.starts_with("else") {
                            let o2 = c1 + 1 + (rest.len() - tail.len()) + 4;
                            let o2 = o2 + src[o2..].find('{').unwrap_or(0);
                            if src[c1 + 1..o2].trim() == "else" {
                                if let Some(c2) = matching(b, o2) {
                                    out.push((i, c2 + 1, src[o1..=c1].to_string()));
                                    out.push((i, c2 + 1, src[o2..=c2].to_string()));
                                }
                            }
                        }
                    }
                }
            }
            // ` as T` casts
            if word == "as" && i > 0 && b[i - 1] == b' ' {
                let mut k = j;
                while k < b.len() && b[k] == b' ' {
                    k += 1;
                }
                let mut e = k;
                while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                    e += 1;
                }
                if e > k {
                    out.push((i - 1, e, String::new()));
                }
            }
            i = j.max(i + 1);
            continue;
        }
        // unary minus in front of a literal
        if c == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_digit() && (i == 0 || matches!(b[i - 1], b'(' | b' ' | b',' | b'=')) {
            out.push((i, i + 1, String::new()));
        }
        // integer literals: try 0 and 1 (suffix kept)
        if c.is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.')) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'_') {
                j += 1;
            }
            let digits = &src[i..j];
            let is_float = j < b.len() && b[j] == b'.' && j + 1 < b.len() && b[j + 1].is_ascii_digit();
            let hex = digits == "0" && j < b.len() && (b[j] == b'x' || b[j] == b'b' || b[j] == b'o');
            if !is_float && !hex {
                let mut k = j;
                while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_') {
                    k += 1;
                }
                let suffix = &src[j..k];
                if suffix.is_empty() || suffix.starts_with('u') || suffix.starts_with('i') {
                    for rep in ["0", "1"] {
                        if digits != rep {
                            out.push((i, k, format!("{rep}{suffix}")));
                        }
                    }
                }
                i = k.max(i + 1);
                continue;
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

/// Apply `expr_candidates` greedily, last position first, evaluating several candidates at once.
pub fn simplify_exprs(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    if cur.len() > 60_000 {
        return cur; // far too many candidates; the case-level and line-level passes come first
    }
    let apply = |s: &str, e: &(usize, usize, String)| format!("{}{}{}", &s[..e.0], e.2, &s[e.1..]);
    loop {
        let mut cands = expr_candidates(&cur);
        let mut progress = false;
        while !cands.is_empty() {
            if expired(deadline) {
                return cur;
            }
            let lo = cands.len().saturating_sub(par_checks());
            let batch: Vec<(usize, usize, String)> = cands[lo..].iter().rev().cloned().collect();
            let texts: Vec<String> = batch.iter().map(|e| apply(&cur, e)).collect();
            let res = par_map(&texts, par_checks(), |c| (c.len() < cur.len() || (c.len() == cur.len() && c.as_str() < cur.as_str())) && it.check(c));
            match res.iter().position(|&ok| ok) {
                Some(p) => {
                    let pos = batch[p].0;
                    cur = texts[p].clone();
                    progress = true;
                    // earlier candidates keep their meaning (they only touch text before `pos`)
                    cands = expr_candidates(&cur).into_iter().filter(|c| c.0 < pos).collect();
                }
                None => cands.truncate(lo),
            }
        }
        if !progress {
            return cur;
        }
    }
}

pub fn strip_parens(it: &dyn Oracle, src: &str) -> String {
    let b = src.as_bytes();
    let mut drop: Vec<usize> = Vec::new();
    for i in 0..b.len() {
        if b[i] != b'(' {
            continue;
        }
        let Some(close) = matching(b, i) else { continue };
        let inner = &src[i + 1..close];
        let simple = !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'!' || b[i - 1] == b'>');
        let next_ok = close + 1 >= b.len() || !matches!(b[close + 1], b'.' | b'[' | b'(');
        if simple && prev_ok && (next_ok || !inner.starts_with(|c: char| c.is_ascii_digit())) && !(close + 1 < b.len() && b[close + 1] == b'(') {
            drop.push(i);
            drop.push(close);
        }
    }
    drop.sort();
    let mut out = String::with_capacity(src.len());
    for (i, ch) in src.char_indices() {
        if drop.binary_search(&i).is_err() {
            out.push(ch);
        }
    }
    if out != src && it.check(&out) { out } else { src.to_string() }
}

/// Edits `(start, end, replacement)` applied greedily from the end of the text, `par_checks()` candidates per round; what lies
/// before an applied edit keeps its position.
pub(crate) fn greedy_edits(it: &dyn Oracle, src: &str, mut edits: Vec<(usize, usize, String)>, deadline: Instant) -> String {
    let mut cur = src.to_string();
    edits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let width = par_checks();
    while !edits.is_empty() {
        if expired(deadline) {
            break;
        }
        let batch: Vec<(usize, usize, String)> = edits.iter().take(width).cloned().collect();
        let cands: Vec<String> = batch.iter().map(|(s, e, t)| format!("{}{}{}", &cur[..*s], t, &cur[*e..])).collect();
        let res = par_map(&cands, width, |c| !c.trim().is_empty() && it.check(c));
        let winners: Vec<usize> = res.iter().enumerate().filter(|(_, ok)| **ok).map(|(i, _)| i).collect();
        if winners.is_empty() {
            edits.drain(..batch.len());
            continue;
        }
        // several accepted on their own: all of them together in one more oracle call (when they do not overlap)
        let build = |subset: &[usize]| -> Option<String> {
            let mut chosen: Vec<&(usize, usize, String)> = subset.iter().map(|&i| &batch[i]).collect();
            chosen.sort_by_key(|e| e.0);
            if chosen.windows(2).any(|w| w[0].1 > w[1].0) {
                return None;
            }
            let mut t = cur.clone();
            for (s, e, r) in chosen.iter().rev() {
                t.replace_range(*s..*e, r);
            }
            Some(t)
        };
        let (applied, text) = match crate::combine::merge_winners(it, &winners, build) {
            Some(m) => m,
            None => (vec![winners[0]], cands[winners[0]].clone()),
        };
        let lowest = applied.iter().map(|&i| batch[i].0).min().unwrap_or(0);
        cur = text;
        // edits behind the changed text are recomputed by the caller's next round; the others of this batch that were fine on their own
        // and lie before the change are tried again
        let again: Vec<(usize, usize, String)> = winners.iter().filter(|w| !applied.contains(w)).map(|&i| batch[i].clone()).filter(|e| e.1 <= lowest).collect();
        edits.drain(..batch.len());
        edits.retain(|e| e.1 <= lowest);
        edits.extend(again);
        edits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    cur
}

/// Blocks in statement position spliced into their parent (`{ a; b }` -> `a; b`), blocks in expression position without
/// statements replaced by parentheses. Generated code is full of them after inlining.
pub fn flatten_blocks(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..4 {
        let b = cur.as_bytes();
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
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
            } else if b[i] == b'{' {
                let prev = cur[..i].trim_end().bytes().last();
                if let Some(close) = matching(b, i) {
                    let inner = &cur[i + 1..close];
                    match prev {
                        Some(b';') | Some(b'{') | Some(b'}') => edits.push((i, close + 1, inner.to_string())),
                        Some(b'=') | Some(b'(') | Some(b',') if !inner.contains(';') && !inner.trim().is_empty() && !inner.contains("let ") => edits.push((i, close + 1, format!("({})", inner.trim()))),
                        _ => {}
                    }
                }
            }
            i += 1;
        }
        if edits.is_empty() {
            break;
        }
        let next = greedy_edits(it, &cur, edits, deadline);
        if next == cur || expired(deadline) {
            return next;
        }
        cur = next;
    }
    cur
}

/// `let (a, b, c) = (x, y, z);` loses the pairs whose name is not needed (what a pasted function call leaves behind).
pub fn simplify_let_tuples(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let mut cur = src.to_string();
    for _ in 0..3 {
        let b = cur.as_bytes();
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        let mut from = 0;
        while let Some(p) = cur[from..].find("let ") {
            let at = from + p;
            from = at + 4;
            if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_') {
                continue;
            }
            let open = crate::scan::skip_ws(b, at + 4);
            if b.get(open) != Some(&b'(') {
                continue;
            }
            let Some(close) = matching(b, open) else { continue };
            let mut eq = crate::scan::skip_ws(b, close + 1);
            // an annotation `: (T1, T2)` is cut together with the pair
            let mut types: Option<Vec<String>> = None;
            if b.get(eq) == Some(&b':') && b.get(eq + 1) != Some(&b':') {
                let topen = crate::scan::skip_ws(b, eq + 1);
                if b.get(topen) == Some(&b'(') {
                    if let Some(tclose) = matching(b, topen) {
                        types = Some(top_args(&cur, topen, tclose).into_iter().map(|(s, e)| cur[s..e].trim().to_string()).collect());
                        eq = crate::scan::skip_ws(b, tclose + 1);
                    }
                }
            }
            if b.get(eq) != Some(&b'=') || b.get(eq + 1) == Some(&b'=') {
                continue;
            }
            let vopen = crate::scan::skip_ws(b, eq + 1);
            if b.get(vopen) != Some(&b'(') {
                continue;
            }
            let Some(vclose) = matching(b, vopen) else { continue };
            let pats: Vec<String> = top_args(&cur, open, close).into_iter().map(|(s, e)| cur[s..e].trim().to_string()).collect();
            let vals: Vec<String> = top_args(&cur, vopen, vclose).into_iter().map(|(s, e)| cur[s..e].trim().to_string()).collect();
            if pats.len() != vals.len() || pats.len() < 2 || types.as_ref().is_some_and(|t| t.len() != pats.len()) {
                continue;
            }
            for i in 0..pats.len() {
                let keep = |k: &usize| *k != i;
                let rest_p: Vec<&str> = pats.iter().enumerate().filter(|(k, _)| keep(k)).map(|(_, p)| p.as_str()).collect();
                let rest_v: Vec<&str> = vals.iter().enumerate().filter(|(k, _)| keep(k)).map(|(_, v)| v.as_str()).collect();
                let rest_t: Option<Vec<&str>> = types.as_ref().map(|t| t.iter().enumerate().filter(|(k, _)| keep(k)).map(|(_, v)| v.as_str()).collect());
                let text = match (&rest_t, rest_p.len()) {
                    (None, 1) => format!("let {} = {}", rest_p[0], rest_v[0]),
                    (None, _) => format!("let ({}) = ({})", rest_p.join(", "), rest_v.join(", ")),
                    (Some(t), 1) => format!("let {}: {} = {}", rest_p[0], t[0], rest_v[0]),
                    (Some(t), _) => format!("let ({}): ({}) = ({})", rest_p.join(", "), t.join(", "), rest_v.join(", ")),
                };
                edits.push((at, vclose + 1, text));
            }
            // the pair as separate statements: later passes can substitute one of them without the other
            if types.is_none() {
                let line_start = cur[..at].rfind('\n').map_or(0, |n| n + 1);
                let indent = &cur[line_start..at];
                if indent.trim().is_empty() {
                    let lets: Vec<String> = pats.iter().zip(&vals).map(|(p, v)| format!("let {p} = {v}")).collect();
                    edits.push((at, vclose + 1, lets.join(&format!(";\n{indent}"))));
                }
            }
        }
        if edits.is_empty() {
            break;
        }
        let next = greedy_edits(it, &cur, edits, deadline);
        if next == cur || expired(deadline) {
            return next;
        }
        cur = next;
    }
    cur
}

/// `let name = EXPR;` becomes `EXPR;` where the binding is not needed (the compiler rejects the candidate when it is).
pub fn drop_let_bindings(it: &dyn Oracle, src: &str, deadline: Instant) -> String {
    let b = src.as_bytes();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut from = 0;
    while let Some(p) = src[from..].find("let ") {
        let at = from + p;
        from = at + 4;
        if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_') {
            continue;
        }
        let mut depth = 0i32;
        let mut eq = None;
        let mut end = None;
        let mut k = at + 4;
        while k < b.len() {
            match b[k] {
                b'"' => {
                    k += 1;
                    while k < b.len() && b[k] != b'"' {
                        if b[k] == b'\\' {
                            k += 1;
                        }
                        k += 1;
                    }
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'=' if depth == 0 && eq.is_none() && b.get(k + 1) != Some(&b'=') && b.get(k + 1) != Some(&b'>') && !matches!(b[k - 1], b'<' | b'>' | b'!') => eq = Some(k),
                b';' if depth == 0 => {
                    end = Some(k + 1);
                    break;
                }
                _ => {}
            }
            if depth < 0 {
                break;
            }
            k += 1;
        }
        if let (Some(eq), Some(end)) = (eq, end) {
            let expr = src[eq + 1..end - 1].trim();
            if !expr.is_empty() {
                edits.push((at, end, format!("{expr};")));
            }
        }
    }
    greedy_edits(it, src, edits, deadline)
}
