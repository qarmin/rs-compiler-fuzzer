use std::process::Command;

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rsmin-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn reduces_with_a_test_script_and_keeps_what_the_script_needs() {
    let dir = scratch("script");
    let input = dir.join("in.rs");
    let mut src = String::from("macro_rules! m { () => { 1 } }\nmod junk {\n    pub fn a() -> u32 { 1 }\n    pub fn b() -> u32 { 2 }\n}\nfn main() {\n");
    for i in 0..60 {
        src.push_str(&format!("    let v{i} = {i} + junk::a();\n"));
    }
    src.push_str("    println!(\"needle {}\", m!());\n}\n");
    std::fs::write(&input, &src).unwrap();
    let script = dir.join("check.sh");
    std::fs::write(&script, "#!/bin/sh\ngrep -q needle \"$1\"\n").unwrap();
    Command::new("chmod").arg("+x").arg(&script).status().unwrap();
    let out = dir.join("out.rs");
    let log = dir.join("run.log");
    let status = Command::new(env!("CARGO_BIN_EXE_rs-minimizer"))
        .arg(&input)
        .args(["--test", &script.display().to_string(), "-o", &out.display().to_string(), "--budget", "60", "--jobs", "4", "--rustc", "", "--no-shrinkray", "--no-treereduce", "--log", &log.display().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let result = std::fs::read_to_string(&out).unwrap();
    assert!(result.contains("needle"), "{result}");
    assert!(result.lines().count() <= 3, "{result}");
    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("done:") && log.contains("smaller program"), "{log}");
}

#[test]
fn refuses_an_input_the_script_rejects() {
    let dir = scratch("reject");
    let input = dir.join("in.rs");
    std::fs::write(&input, "fn main() {}\n").unwrap();
    let script = dir.join("check.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    Command::new("chmod").arg("+x").arg(&script).status().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rs-minimizer")).arg(&input).args(["--test", &script.display().to_string()]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing to reduce"));
}

#[test]
fn help_and_usage_errors() {
    let help = Command::new(env!("CARGO_BIN_EXE_rs-minimizer")).arg("--help").output().unwrap();
    assert!(help.status.success() && String::from_utf8_lossy(&help.stdout).contains("USAGE"));
    let bad = Command::new(env!("CARGO_BIN_EXE_rs-minimizer")).output().unwrap();
    assert!(!bad.status.success());
}
