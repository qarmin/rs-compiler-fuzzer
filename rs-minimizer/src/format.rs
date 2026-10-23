use crate::oracle::Oracle;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

/// `rustfmt` the result if it stays interesting (the issue text is easier to read).
pub fn format_if_interesting(it: &dyn Oracle, src: &str) -> String {
    if src.lines().count() > 400 {
        return src.to_string(); // formatting only makes a big leftover bigger
    }
    let dir = it.work_dir().join(format!("fmt{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("x.rs");
    std::fs::write(&f, src).unwrap();
    let ok = Command::new("rustfmt").args(["--edition", "2021", "--config", "max_width=140,use_small_heuristics=Max"]).arg(&f).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    let res = if ok { std::fs::read_to_string(&f).unwrap_or_default() } else { String::new() };
    let _ = std::fs::remove_dir_all(&dir);
    if !res.is_empty() && it.check(&res) { res } else { src.to_string() }
}
