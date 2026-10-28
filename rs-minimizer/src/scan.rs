//! Text scanning helpers shared by the passes (the passes work on source text, not on a parsed tree).

pub fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Matching close bracket for the open bracket at `open`, skipping string literals.
pub fn matching(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut k = open;
    while k < b.len() {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            b'"' => {
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
    None
}

/// Start of the postfix expression (`a.b(c)[d].e`) that ends right before `end`.
pub fn postfix_start(b: &[u8], mut end: usize) -> usize {
    loop {
        if end == 0 {
            return 0;
        }
        let c = b[end - 1];
        if c == b'}' {
            // `{ .. }`, `unsafe { .. }`, `if c { .. } else { .. }`: the whole expression is the receiver
            let Some(start) = block_expr_start(b, end - 1) else { return end };
            end = start;
            if end > 0 && b[end - 1] == b'.' {
                end -= 1;
                continue;
            }
            return end;
        }
        if c == b')' || c == b']' {
            // jump to the matching open bracket
            let (open, close) = if c == b')' { (b'(', b')') } else { (b'[', b']') };
            let mut depth = 0i32;
            let mut k = end - 1;
            loop {
                if b[k] == close {
                    depth += 1;
                } else if b[k] == open {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if k == 0 {
                    return 0;
                }
                k -= 1;
            }
            end = k;
            // the callee / macro name in front of the bracket
            while end > 0 && (b[end - 1].is_ascii_alphanumeric() || b[end - 1] == b'_' || b[end - 1] == b':' || b[end - 1] == b'!') {
                end -= 1;
            }
            // `.name(...)` continues the chain
            if end > 0 && b[end - 1] == b'.' {
                end -= 1;
                continue;
            }
            if end > 0 && (b[end - 1] == b')' || b[end - 1] == b']') {
                continue;
            }
            return end;
        } else if c.is_ascii_alphanumeric() || c == b'_' || c == b':' {
            while end > 0 && (b[end - 1].is_ascii_alphanumeric() || b[end - 1] == b'_' || b[end - 1] == b':') {
                end -= 1;
            }
            if end > 0 && b[end - 1] == b'.' {
                end -= 1;
                continue;
            }
            return end;
        } else {
            return end;
        }
    }
}

/// Byte ranges of the top-level, comma separated arguments inside the brackets at `open..=close`.
pub fn top_args(src: &str, open: usize, close: usize) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut start = open + 1;
    let mut depth = 0i32;
    let mut k = open + 1;
    while k < close {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'"' => {
                k += 1;
                while k < close && b[k] != b'"' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            b',' if depth == 0 => {
                out.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    if src[start..close].trim().len() > 0 {
        out.push((start, close));
    }
    out
}

/// Open bracket that matches the close bracket at `close` (searching backwards).
fn open_of(b: &[u8], close: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut k = close;
    loop {
        match b[k] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
        if k == 0 {
            return None;
        }
        k -= 1;
    }
}

fn ends_with_word(b: &[u8], end: usize, word: &str) -> bool {
    let w = word.as_bytes();
    end >= w.len() && &b[end - w.len()..end] == w && (end == w.len() || !is_ident(b[end - w.len() - 1]))
}

fn trim_end(b: &[u8], mut end: usize) -> usize {
    while end > 0 && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

/// Start of the block-like expression that ends with the `}` at `close`: a plain or `unsafe` block, or an `if` / `else if` / `else`
/// chain. `None` for blocks that belong to something else (a `match`, a loop, a struct literal).
fn block_expr_start(b: &[u8], close: usize) -> Option<usize> {
    let mut open = open_of(b, close)?;
    loop {
        let before = trim_end(b, open);
        if ends_with_word(b, before, "else") {
            let then_close = trim_end(b, before - 4);
            if then_close == 0 || b[then_close - 1] != b'}' {
                return None;
            }
            open = open_of(b, then_close - 1)?;
            continue;
        }
        if ends_with_word(b, before, "unsafe") {
            return Some(before - 6);
        }
        if before > 0 && matches!(b[before - 1], b';' | b'{' | b'}' | b'(' | b',' | b'=' | b'>' | b'<' | b'+' | b'-' | b'*' | b'&' | b'|' | b'!') || before == 0 {
            return Some(open);
        }
        // a condition sits between `if` and the block: walk back over bracket groups to the `if` keyword
        let mut k = before;
        while k > 0 {
            if matches!(b[k - 1], b')' | b']' | b'}') {
                k = open_of(b, k - 1)?;
                continue;
            }
            if ends_with_word(b, k, "if") {
                let start = k - 2;
                let pre = trim_end(b, start);
                if ends_with_word(b, pre, "else") {
                    let then_close = trim_end(b, pre - 4);
                    if then_close == 0 || b[then_close - 1] != b'}' {
                        return None;
                    }
                    open = open_of(b, then_close - 1)?;
                    break;
                }
                return Some(start);
            }
            if ends_with_word(b, k, "match") || ends_with_word(b, k, "while") || ends_with_word(b, k, "for") || ends_with_word(b, k, "loop") {
                return None;
            }
            k -= 1;
        }
        if k == 0 {
            return None;
        }
    }
}
