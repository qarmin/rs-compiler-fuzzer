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
    let status = Command::new(env!("CARGO_BIN_EXE_rs-compiler-fuzzer"))
        .arg("reduce")
        .arg(&input)
        .args(["--test", &script.display().to_string(), "-o", &out.display().to_string(), "--budget", "60", "--jobs", "4", "--rustc", "", "--log", &log.display().to_string()])
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
    let out = Command::new(env!("CARGO_BIN_EXE_rs-compiler-fuzzer")).arg("reduce").arg(&input).args(["--test", &script.display().to_string()]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing to reduce"));
}

#[test]
fn help_and_usage_errors() {
    let help = Command::new(env!("CARGO_BIN_EXE_rs-compiler-fuzzer")).args(["reduce", "--help"]).output().unwrap();
    assert!(help.status.success() && String::from_utf8_lossy(&help.stdout).contains("USAGE"));
    let bad = Command::new(env!("CARGO_BIN_EXE_rs-compiler-fuzzer")).arg("reduce").output().unwrap();
    assert!(!bad.status.success());
}

fn text_of(o: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// `extra` may name the compiler (`--rustc`); the default is the plain `rustc`: only the test that needs `-Z` asks for nightly.
fn reduce(dir: &std::path::Path, source: &str, extra: &[&str]) -> (std::process::Output, String) {
    let input = dir.join("in.rs");
    std::fs::write(&input, source).unwrap();
    let out = dir.join("out.rs");
    let o = Command::new(env!("CARGO_BIN_EXE_rs-compiler-fuzzer"))
        .arg("reduce")
        .arg(&input)
        .args(["--rustc", if extra.contains(&"-Z treat-err-as-bug=1") { "rustc +nightly" } else { "rustc" }, "-o"])
        .arg(&out)
        .args(["--budget", "60", "--jobs", "4"])
        .args(extra)
        .output()
        .unwrap();
    let result = std::fs::read_to_string(&out).unwrap_or_default();
    (o, result)
}

const NOISY_ERROR: &str = "use std::collections::HashMap;\nfn helper(a: u32) -> u32 { a * 2 }\nstruct Unused { x: u8 }\nfn main() {\n    let mut total = 0;\n    for i in 0..10 { total += helper(i); }\n    let wrong: u32 = \"text\";\n    println!(\"{}\", total);\n}\n";

#[test]
fn a_compile_error_is_found_and_reduced_without_a_script() {
    let dir = scratch("error");
    let (o, result) = reduce(&dir, NOISY_ERROR, &[]);
    assert!(o.status.success(), "{}", text_of(&o));
    assert!(text_of(&o).contains("E0308"), "the detected error is announced:\n{}", text_of(&o));
    assert!(result.contains("u32") && result.lines().count() <= 3, "{result}");
    assert!(!result.contains("helper") && !result.contains("HashMap"), "{result}");
}

#[test]
fn a_compiler_crash_is_found_by_its_flags_and_kept() {
    let dir = scratch("ice");
    let (o, result) = reduce(&dir, NOISY_ERROR, &["--flags", "-Z treat-err-as-bug=1"]);
    assert!(o.status.success(), "{}", text_of(&o));
    assert!(text_of(&o).contains("the compiler crashes"), "{}", text_of(&o));
    assert!(result.contains("u32") && result.lines().count() <= 3, "{result}");
}

#[test]
fn two_builds_that_disagree_are_reduced_to_what_makes_them_disagree() {
    let dir = scratch("vs");
    let src = "fn unused() -> u32 { 5 }\nfn main() {\n    let v: Vec<u32> = (0..5).collect();\n    println!(\"{}\", v.len());\n    println!(\"{}\", cfg!(debug_assertions));\n}\n";
    let (o, result) = reduce(&dir, src, &["--flags", "-C debug-assertions=off", "--vs", "-C debug-assertions=on"]);
    assert!(o.status.success(), "{}", text_of(&o));
    assert!(result.contains("debug_assertions") && !result.contains("unused") && !result.contains("collect"), "{result}");
}

#[test]
fn a_file_with_nothing_wrong_says_so() {
    let dir = scratch("fine");
    let (o, _) = reduce(&dir, "fn main() { println!(\"{}\", 6 * 7); }\n", &[]);
    assert!(!o.status.success());
    assert!(text_of(&o).contains("nothing to reduce"), "{}", text_of(&o));
}
