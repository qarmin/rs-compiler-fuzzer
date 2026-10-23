use rs_minimizer::rustc_oracle::{Mode, Spec, crash_signature, detect};
use std::time::Duration;

fn spec(flags: &[&str], mode: Mode) -> Spec {
    // only `-Z` options need nightly
    let nightly = flags.contains(&"-Z");
    Spec { rustc: if nightly { vec!["rustc".into(), "+nightly".into()] } else { vec!["rustc".into()] }, flags: flags.iter().map(|s| s.to_string()).collect(), mode, compile_timeout: Duration::from_secs(120), run_timeout: Duration::from_secs(20) }
}

const WRONG_TYPE: &str = "fn main() {\n    let x: u32 = \"a\";\n}\n";
const FINE: &str = "fn main() {\n    println!(\"{}\", cfg!(debug_assertions));\n}\n";

#[test]
fn a_compile_error_is_detected_with_its_code_and_kept_only_while_it_shows() {
    let s = spec(&[], Mode::Hang { secs: 1 });
    let (mode, why) = detect(&s, WRONG_TYPE).expect("an error is something to reduce");
    assert_eq!(mode, Mode::Error { needle: "E0308".into() }, "{why}");
    let s = spec(&[], mode);
    assert!(s.check(WRONG_TYPE));
    assert!(s.check("fn main() { let y: i8 = 1; let x: u32 = \"b\"; }"));
    assert!(!s.check("fn main() { let x: u32 = 1; }"), "compiles: the error is gone");
    assert!(!s.check("fn main() { let x: u32 = foo(); }"), "another error");
}

#[test]
fn a_crash_is_detected_by_its_signature() {
    // the compiler is told to treat the first error as a bug: a reproducible "internal compiler error" without a compiler bug
    let s = spec(&["-Z", "treat-err-as-bug=1"], Mode::Hang { secs: 1 });
    let (mode, why) = detect(&s, WRONG_TYPE).expect("a crash is something to reduce");
    assert!(matches!(&mode, Mode::Crash { signature } if signature.contains("internal compiler error")), "{why}");
    let s = spec(&["-Z", "treat-err-as-bug=1"], mode);
    assert!(s.check(WRONG_TYPE));
    assert!(!s.check("fn main() {}"), "no error, no crash");
}

#[test]
fn two_flag_sets_that_give_different_output_are_a_miscompilation_for_the_oracle() {
    let s = spec(&["-C", "debug-assertions=off"], Mode::Differ { other_flags: vec!["-C".into(), "debug-assertions=on".into()] });
    assert!(s.check(FINE));
    assert!(!s.check("fn main() { println!(\"same\"); }"));
    assert!(!s.check("fn main() { let x: u32 = \"a\"; }"), "a program that does not compile is no miscompilation");
}

#[test]
fn output_and_stderr_needles() {
    let out = spec(&[], Mode::Output { stdout: true, needle: "needle".into() });
    assert!(out.check("fn main() { println!(\"a needle b\"); }"));
    assert!(!out.check("fn main() { println!(\"a b\"); }"));
    let err = spec(&[], Mode::Output { stdout: false, needle: "boom".into() });
    assert!(err.check("fn main() { panic!(\"boom\"); }"));
}

#[test]
fn a_hang_is_a_compilation_that_does_not_finish() {
    let s = spec(&[], Mode::Hang { secs: 0 });
    let mut s = s;
    s.compile_timeout = Duration::from_millis(1);
    assert!(s.check(FINE), "one millisecond is not enough for rustc");
    s.compile_timeout = Duration::from_secs(120);
    assert!(!s.check(FINE));
}

#[test]
fn memory_is_measured_while_the_compiler_runs() {
    // rustc needs far more than 1 MB: the limit is crossed at once and the compiler is killed
    let tiny = spec(&[], Mode::Oom { mb: 1 });
    assert!(tiny.check(FINE));
    let huge = spec(&[], Mode::Oom { mb: 1_000_000 });
    assert!(!huge.check(FINE));
}

#[test]
fn a_program_that_behaves_the_same_everywhere_has_nothing_to_reduce() {
    let s = spec(&[], Mode::Hang { secs: 1 });
    assert!(detect(&s, "fn main() { println!(\"{}\", 6 * 7); }").is_none());
}

#[test]
fn crash_signatures_ignore_numbers_and_quoted_names() {
    let a = "error: internal compiler error: compiler/rustc_mir/src/foo.rs:10:20: assertion failed for `Foo<u32>`\nthread 'rustc' panicked at compiler/rustc_mir/src/foo.rs:10:20:\nboom";
    let b = "error: internal compiler error: compiler/rustc_mir/src/foo.rs:99:3: assertion failed for `Bar`\nthread 'rustc' panicked at compiler/rustc_mir/src/foo.rs:99:3:\nboom";
    assert_eq!(crash_signature(a, None), crash_signature(b, None));
    assert!(crash_signature("error[E0308]: mismatched types", None).is_none());
    assert!(crash_signature("", Some(11)).is_some(), "a segmentation fault is a crash");
}
