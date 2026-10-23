//! The compiler as a name resolver and lint source: one `rustc --emit=metadata` run, its diagnostics in the short format.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug)]
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub error: bool,
    pub msg: String,
}

impl Diag {
    /// Byte offset of the start of the diagnostic's span.
    pub fn offset(&self, src: &str) -> Option<usize> {
        let start = src.split_inclusive('\n').take(self.line.checked_sub(1)?).map(str::len).sum::<usize>();
        let l = src[start..].split('\n').next()?;
        let off = l.char_indices().nth(self.col.checked_sub(1)?).map(|(i, _)| i)?;
        Some(start + off)
    }
}

/// Diagnostics of `src` compiled as a bin (`fn main` present) or a lib; `None` when the compiler cannot be run.
pub fn compile_diags(rustc: &[String], work: &Path, src: &str, extra: &[&str]) -> Option<Vec<Diag>> {
    let dir = work.join(format!("diag{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join("main.rs");
    std::fs::write(&file, src).ok()?;
    let crate_type = if src.contains("fn main") { "bin" } else { "lib" };
    let out = Command::new(rustc.first()?)
        .args(&rustc[1..])
        .args(["--edition", "2021", "--emit=metadata", "--crate-type", crate_type, "--error-format=short", "--out-dir"])
        .arg(&dir)
        .args(extra)
        .arg(&file)
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&out.ok()?.stderr).to_string();
    let mut diags = Vec::new();
    for l in text.lines() {
        let mut parts = l.splitn(4, ':');
        let (Some(_), Some(line), Some(col), Some(rest)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
        let (Ok(line), Ok(col)) = (line.parse::<usize>(), col.parse::<usize>()) else { continue };
        let rest = rest.trim_start();
        let (error, msg) = match (rest.strip_prefix("error"), rest.strip_prefix("warning")) {
            (Some(m), _) => (true, m),
            (_, Some(m)) => (false, m),
            _ => continue,
        };
        let msg = msg.trim_start_matches(|c: char| c != ':').trim_start_matches(':').trim().to_string();
        diags.push(Diag { line, col, error, msg });
    }
    Some(diags)
}
