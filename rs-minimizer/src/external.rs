//! External reducers driven as child processes: Shrink Ray (strongest on expression chains) and treereduce-rust (tree-sitter).
//! Both call back into the oracle through `Oracle::external_command`.

use crate::control::{interrupted, mark_deadline_hit, note_external_result, par, set_external_running};
use crate::oracle::Oracle;
use crate::pipeline::ReduceLog;
use log::{error, info, warn};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static N: AtomicUsize = AtomicUsize::new(0);

fn shell_quote(a: &str) -> String {
    format!("'{}'", a.replace('\'', "'\\''"))
}

/// Shrink Ray (https://github.com/DRMacIver/shrinkray, `pip install shrinkray`): a generic reducer with
/// tree-sitter based passes. By far the strongest of the tools tried here on expression chains, so it
/// gets most of the time budget. Returns the input unchanged if it is not installed.
pub fn shrinkray_available() -> bool {
    let exe = std::env::var("SHRINKRAY").unwrap_or_else(|_| "shrinkray".to_string());
    Command::new(exe).arg("--version").output().is_ok()
}

pub fn shrinkray(it: &dyn Oracle, src: &str, deadline: Instant, log: &mut ReduceLog) -> String {
    let exe = std::env::var("SHRINKRAY").unwrap_or_else(|_| "shrinkray".to_string());
    if Command::new(&exe).arg("--version").output().is_err() {
        return src.to_string();
    }
    let dir = it.work_dir().join(format!("sr{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&dir);
    let Some(cmd) = it.external_command(&dir) else {
        warn!("this oracle cannot be called from another process - shrinkray skipped");
        return src.to_string();
    };
    let (case, test) = (dir.join("case.rs"), dir.join("test.sh"));
    std::fs::write(&case, src).unwrap();
    std::fs::write(&test, format!("#!/bin/bash\nexec {} \"$1\"\n", cmd.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "))).unwrap();
    let _ = Command::new("chmod").arg("+x").arg(&test).status();
    let sr_start = Instant::now();
    // Shrink Ray keeps going after the last improvement (random restarts) and never exits on its own: stop it when the file
    // has not changed for a while. A stall counts as "finished"; hitting the deadline does not.
    let stall = Duration::from_secs((deadline.saturating_duration_since(sr_start).as_secs() / 5).clamp(45, 150) + (src.len() as u64 / 2000).min(240));
    let mut child = match Command::new(&exe)
        .args(["--ui", "basic", "--input-type", "arg", "--parallelism", &par().to_string(), "--volume", "quiet", "--timeout", "300", "--memory-limit", "0", "--exit-on-completion"])
        .arg(&test)
        .arg(&case)
        .current_dir(&dir)
        .stdout(std::fs::File::create(dir.join("sr.out")).map(Stdio::from).unwrap_or(Stdio::null()))
        .stderr(std::fs::File::create(dir.join("sr.err")).map(Stdio::from).unwrap_or(Stdio::null()))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            error!("cannot start {exe}: {e}");
            return src.to_string();
        }
    };
    info!("shrinkray started (pid {}, {} workers, gives up after {}s without a change)", child.id(), par(), stall.as_secs());
    set_external_running(true);
    let mut last_text = std::fs::read_to_string(&case).unwrap_or_default();
    let mut last_change = Instant::now();
    let how;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                how = format!("exited by itself ({:?})", st.code());
                break;
            }
            Ok(None) => {}
            Err(_) => {
                how = "lost".to_string();
                break;
            }
        }
        if Instant::now() >= deadline || interrupted() {
            mark_deadline_hit();
            how = "stopped at the deadline".to_string();
            let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
            break;
        }
        let now_text = std::fs::read_to_string(&case).unwrap_or_default();
        if now_text != last_text {
            if !now_text.trim().is_empty() {
                note_external_result(&now_text);
            }
            last_text = now_text;
            last_change = Instant::now();
        } else if last_change.elapsed() > stall {
            how = format!("no improvement for {}s", stall.as_secs());
            let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }
    // give it a moment to write the final file, then make sure it is gone
    for _ in 0..10 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    let _ = child.kill();
    let _ = child.wait();
    set_external_running(false);
    log.push(format!("shrinkray {how} after {}s", sr_start.elapsed().as_secs()));
    {
        let said = format!("{}{}", std::fs::read_to_string(dir.join("sr.out")).unwrap_or_default(), std::fs::read_to_string(dir.join("sr.err")).unwrap_or_default());
        if said.contains("should return 0") {
            warn!("shrinkray's check of the start program failed: the oracle called from another process says `not interesting`. In-process re-check: {}. {}", it.recheck(src), it.explain(src));
        }
    }
    if how.starts_with("exited by itself") {
        // an early exit usually means the tool gave up: keep its last words
        let text = format!("{}{}", std::fs::read_to_string(dir.join("sr.out")).unwrap_or_default(), std::fs::read_to_string(dir.join("sr.err")).unwrap_or_default());
        let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect();
        if !tail.is_empty() {
            log.push(format!("shrinkray said: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ").chars().take(400).collect::<String>()));
        }
    }
    let res = std::fs::read_to_string(&case).unwrap_or_else(|_| src.to_string());
    if res == src {
        // nothing happened: keep the tool's last words, otherwise a broken setup is invisible
        let text = format!("{}{}", std::fs::read_to_string(dir.join("sr.out")).unwrap_or_default(), std::fs::read_to_string(dir.join("sr.err")).unwrap_or_default());
        let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(6).collect();
        log.push(format!("shrinkray made no progress: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    let _ = std::fs::remove_dir_all(&dir);
    if res.trim().is_empty() || !it.check(&res) { src.to_string() } else { res }
}

/// treereduce-rust (syntax-tree delta debugging). Returns the input unchanged if the tool is missing, fails or finds nothing
/// that is still interesting; stops at the deadline or on Ctrl+C.
pub fn treereduce(it: &dyn Oracle, src: &str, deadline: Instant, jobs: usize, passes: usize) -> String {
    if interrupted() {
        return src.to_string();
    }
    if Command::new("treereduce-rust").arg("--version").output().is_err() {
        warn!("treereduce-rust is not installed (cargo install treereduce-rust) - skipped");
        return src.to_string();
    }
    let dir = it.work_dir().join(format!("tr{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(&dir);
    let Some(cmd) = it.external_command(&dir) else {
        warn!("this oracle cannot be called from another process - treereduce-rust skipped");
        return src.to_string();
    };
    let started = Instant::now();
    info!("treereduce-rust started ({}s at most)", deadline.saturating_duration_since(Instant::now()).as_secs());
    let (inp, out) = (dir.join("in.rs"), dir.join("out.rs"));
    std::fs::write(&inp, src).unwrap();
    let child = Command::new("treereduce-rust")
        .args(["-s", &inp.display().to_string(), "-o", &out.display().to_string(), "-j", &jobs.to_string(), "--passes", &passes.to_string(), "--min-reduction", "1", "-q"])
        .args(["--timeout", "400", "--"])
        .args(&cmd)
        .arg("@@")
        .stdout(std::fs::File::create(dir.join("tr.out")).map(Stdio::from).unwrap_or(Stdio::null()))
        .stderr(std::fs::File::create(dir.join("tr.err")).map(Stdio::from).unwrap_or(Stdio::null()))
        .spawn();
    let mut code = None;
    match child {
        Ok(mut c) => loop {
            match c.try_wait() {
                Ok(Some(st)) => {
                    code = st.code();
                    break;
                }
                Ok(None) => {}
                Err(_) => break,
            }
            if Instant::now() >= deadline || interrupted() {
                mark_deadline_hit();
                let _ = c.kill();
                let _ = c.wait();
                break;
            }
            thread::sleep(Duration::from_millis(500));
        },
        Err(e) => error!("cannot start treereduce-rust: {e}"),
    }
    let res = std::fs::read_to_string(&out).unwrap_or_else(|_| src.to_string());
    let said = format!("{}{}", std::fs::read_to_string(dir.join("tr.out")).unwrap_or_default(), std::fs::read_to_string(dir.join("tr.err")).unwrap_or_default());
    let _ = std::fs::remove_dir_all(&dir);
    // a reducer result that is not interesting any more (timeouts, flakiness) is discarded
    if res != src && it.check(&res) {
        info!("treereduce-rust finished: {} -> {} bytes (status {code:?})", src.len(), res.len());
        res
    } else {
        let tail: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect();
        if code == Some(1) && started.elapsed().as_secs() < 5 {
            warn!("treereduce-rust gave up at once: its check of the start program probably failed. In-process re-check: {}. {}", it.recheck(src), it.explain(src));
        }
        info!("treereduce-rust finished after {}s (status {code:?}) without an improvement; it said: {}", started.elapsed().as_secs(), tail.into_iter().rev().collect::<Vec<_>>().join(" | "));
        src.to_string()
    }
}
