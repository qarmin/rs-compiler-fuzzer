//! Make compiler backtraces readable: `_ZN4llvm16SelectionDAGISel17CodeGenAndEmitDAGEv` becomes
//! `llvm::SelectionDAGISel::CodeGenAndEmitDAG()`, Rust (legacy and v0) symbols likewise.

fn demangle_symbol(sym: &str) -> Option<String> {
    if let Ok(d) = rustc_demangle::try_demangle(sym) {
        return Some(format!("{d:#}"));
    }
    if let Ok(s) = cpp_demangle::Symbol::new(sym) {
        if let Ok(d) = s.demangle(&cpp_demangle::DemangleOptions::default()) {
            return Some(d);
        }
    }
    None
}

/// Replace every mangled symbol in `text` (leave everything else, including `+0x..` offsets, alone).
pub fn demangle_text(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let is_sym = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'.';
    while i < b.len() {
        let starts = (b[i] == b'_') && i + 2 < b.len() && (b[i + 1] == b'Z' || b[i + 1] == b'R') && (i == 0 || !is_sym(b[i - 1]));
        if starts {
            let mut j = i;
            while j < b.len() && is_sym(b[j]) {
                j += 1;
            }
            let sym = &text[i..j];
            match demangle_symbol(sym) {
                Some(d) => out.push_str(&d),
                None => out.push_str(sym),
            }
            i = j;
        } else {
            // copy one char (text may contain multi-byte characters)
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demangles_llvm_and_rust_symbols() {
        let s = "libLLVM.so(_ZN4llvm16SelectionDAGISel17CodeGenAndEmitDAGEv+0x189) [0x7d]";
        let d = demangle_text(s);
        assert!(d.contains("llvm::SelectionDAGISel::CodeGenAndEmitDAG()"), "{d}");
        assert!(d.contains("+0x189"));
        let r = demangle_text("x(_RNvCs1_5krate3foo+0x1)");
        assert!(r.contains("krate::foo"), "{r}");
        assert_eq!(demangle_text("plain text ü _ZNotASymbol"), "plain text ü _ZNotASymbol");
    }
}
