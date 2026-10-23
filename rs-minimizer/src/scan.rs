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
