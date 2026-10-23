//! Renaming one declaration with the compiler as the name resolver: the declaration gets the new name, every use that no longer
//! resolves is reported by rustc with its exact position and gets the new name too. Text-level renaming cannot tell the several
//! `a` of a renamed program apart (module, trait, function, method); the compiler can.

use crate::diag::compile_diags;
use crate::scan::is_ident;
use std::path::Path;

fn word_at(src: &str, off: usize, word: &str) -> bool {
    let b = src.as_bytes();
    src[off..].starts_with(word) && (off == 0 || !is_ident(b[off - 1])) && b.get(off + word.len()).is_none_or(|&c| !is_ident(c))
}

/// `src` with the declaration whose name starts at `pos` renamed to `new` and all uses fixed up; `None` if the program does not
/// compile again within a few rounds.
pub fn rename_decl(rustc: &[String], work: &Path, src: &str, pos: usize, old: &str, new: &str) -> Option<String> {
    if !word_at(src, pos, old) {
        return None;
    }
    let mut cur = format!("{}{}{}", &src[..pos], new, &src[pos + old.len()..]);
    for _ in 0..10 {
        let diags = compile_diags(rustc, work, &cur, &["-A", "warnings"])?;
        let errors: Vec<_> = diags.iter().filter(|d| d.error && !d.msg.starts_with("aborting")).collect();
        if errors.is_empty() {
            return Some(cur);
        }
        let mut spots: Vec<usize> = Vec::new();
        for d in errors {
            let Some(off) = d.offset(&cur) else { return None };
            if word_at(&cur, off, old) {
                spots.push(off);
            } else if let Some(found) = cur[off..(off + 160).min(cur.len())].split('\n').next().and_then(|l| l.match_indices(old).map(|(i, _)| off + i).find(|&o| word_at(&cur, o, old))) {
                // an unresolved import is reported at the start of its path, not at the name
                spots.push(found);
            }
        }
        spots.sort_unstable();
        spots.dedup();
        if spots.is_empty() {
            return None;
        }
        for &o in spots.iter().rev() {
            cur.replace_range(o..o + old.len(), new);
        }
    }
    None
}

/// A name based on `base` that does not occur as a word in `src`.
pub fn fresh_name(src: &str, base: &str) -> String {
    let b = src.as_bytes();
    let taken = |name: &str| src.match_indices(name).any(|(i, _)| (i == 0 || !is_ident(b[i - 1])) && b.get(i + name.len()).is_none_or(|&c| !is_ident(c)));
    if !taken(base) {
        return base.to_string();
    }
    (1..).map(|n| format!("{base}{n}")).find(|c| !taken(c)).unwrap()
}
