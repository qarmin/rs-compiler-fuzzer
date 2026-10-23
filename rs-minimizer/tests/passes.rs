mod common;
use common::*;
use rs_minimizer::deadcode::remove_dead_code;
use rs_minimizer::exprs::{drop_calls, simplify_exprs, simplify_lets, strip_parens};
use rs_minimizer::{Oracle, Options, ReduceLog, minimize};
use std::time::Duration;

#[test]
fn the_test_oracle_rejects_wrong_and_broken_programs() {
    let o = prints("1\n");
    assert!(o.check("fn main() { println!(\"1\"); }"));
    assert!(!o.check("fn main() { println!(\"2\"); }"));
    assert!(!o.check("fn main() { println!(\"1\") "));
}

#[test]
fn compiler_reported_dead_code_is_removed() {
    let src = "use std::collections::HashMap;\nuse std::fmt::Debug;\nfn used() -> u32 { 7 }\nfn never() -> u32 { 1 }\nstruct Never2;\nimpl Never2 { fn m(&self) {} }\nfn main() { println!(\"{}\", used()); }\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = remove_dead_code(&o, src, &["rustc".to_string()], far());
    for gone in ["HashMap", "Debug", "never", "Never2"] {
        assert!(!out.contains(gone), "{gone} should be gone:\n{out}");
    }
    assert_prints(&out, "7\n");
}

#[test]
fn dead_code_removal_keeps_what_the_oracle_needs() {
    let src = "fn marker() {}\nfn main() { println!(\"1\"); }\n";
    let o = rs_minimizer::Cached::new(|s: &str| s.contains("fn marker") && run_program(s).as_deref() == Some("1\n"));
    check_oracle(&o, src);
    let out = remove_dead_code(&o, src, &["rustc".to_string()], far());
    assert!(out.contains("fn marker"), "{out}");
}

#[test]
fn lets_and_expressions_shrink_to_literals() {
    let src = "use std::hint::black_box;\nfn main() {\n    let a: u32 = black_box(7u32).wrapping_mul(3).wrapping_add(11).min(1000);\n    let b: u32 = 5u32.max(2).pow(2);\n    println!(\"{}\", a.min(b) * 0 + 4);\n}\n";
    let o = prints("4\n");
    check_oracle(&o, src);
    let mut out = simplify_lets(&o, src, far());
    out = simplify_exprs(&o, &out, far());
    assert!(out.len() < src.len() * 3 / 4, "{out}");
    assert_prints(&out, "4\n");
}

#[test]
fn method_calls_are_dropped_from_chains() {
    let src = "fn main() {\n    let x: u64 = 10u64.wrapping_add(1).wrapping_mul(1).saturating_sub(0).max(0);\n    println!(\"{}\", x > 0);\n}\n";
    let o = prints("true\n");
    check_oracle(&o, src);
    let out = drop_calls(&o, src, far());
    assert!(out.matches('.').count() < src.matches('.').count(), "{out}");
    assert_prints(&out, "true\n");
}

#[test]
fn redundant_parentheses_go() {
    let src = "fn main() { let a = (1u32); let b = (a) + (2); println!(\"{}\", b); }\n";
    let o = prints("3\n");
    let out = strip_parens(&o, src);
    assert!(out.len() < src.len(), "{out}");
    assert_prints(&out, "3\n");
}

/// What a batch of generated cases looks like: runtime module, macro, panic plumbing, several cases of which one is needed.
const BATCH: &str = "mod rt {\n    pub struct H(pub u64);\n    impl H {\n        pub fn new() -> H { H(1) }\n        pub fn put(&mut self, v: u64) { self.0 = self.0.wrapping_mul(31).wrapping_add(v); }\n    }\n    pub fn report(id: usize, ok: bool, h: &H) { if ok { println!(\"{} {}\", id, h.0) } else { println!(\"{} PANIC\", id) } }\n}\nuse rt::*;\nmod case_1 {\n    use super::rt::*;\n    pub fn run(h: &mut H) { let a = std::hint::black_box(3u64); h.put(a); h.put(a * 2); }\n}\nmod case_2 {\n    use super::rt::*;\n    fn unused_helper() -> u32 { 4 }\n    pub fn run(h: &mut H) { h.put(100); }\n}\nmod case_3 {\n    use super::rt::*;\n    pub fn run(h: &mut H) { h.put(7); }\n}\nmacro_rules! go { ($id:literal, $m:ident) => {{\n    let mut h = H::new();\n    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $m::run(&mut h))).is_ok();\n    report($id, ok, &h);\n}}; }\nfn main() {\n    std::panic::set_hook(Box::new(|_| {}));\n    go!(1, case_1);\n    go!(2, case_2);\n    go!(3, case_3);\n}\n";

#[test]
fn a_batch_harness_is_reduced_to_plain_code_for_the_needed_case() {
    let expected = "1 1018\n2 131\n3 38\n";
    let full = prints(expected);
    // the "bug": the line of case 2 is printed
    let o = rs_minimizer::Cached::new(|s: &str| run_program(s).map(|out| out.contains("2 131\n")).unwrap_or(false));
    let _ = full;
    check_oracle(&o, BATCH);
    rs_minimizer::control::set_parallel(8);
    let opts = Options::new(Duration::from_secs(120));
    let mut log = ReduceLog::default();
    let out = minimize(&o, BATCH, &opts, &mut log, &mut |_, _, _| None);
    assert!(!out.contains("macro_rules") && !out.contains("catch_unwind") && !out.contains("set_hook"), "{out}");
    assert!(!out.contains("case_1") && !out.contains("case_3") && !out.contains("unused_helper"), "{out}");
    assert!(!out.contains("mod ") && !out.contains("fn run") && !out.contains("trait "), "everything is plain code in main:\n{out}");
    assert!(out.lines().count() <= 25, "{out}");
    assert!(run_program(&out).unwrap().contains("2 131\n"), "{out}");
}

#[test]
fn calls_are_pasted_into_their_callers_and_the_functions_go() {
    let src = "struct P(u32);\nimpl P {\n    fn new(x: u32) -> P { P(x) }\n    fn add(&mut self, d: u32) { self.0 += d; }\n    fn get(&self) -> u32 { self.0 }\n}\nfn twice(v: u32) -> u32 { v * 2 }\nfn main() {\n    let mut p = P::new(3);\n    p.add(twice(4));\n    println!(\"{}\", p.get());\n}\n";
    let o = prints("11\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::inline::inline_calls(&o, src, &["rustc".to_string()], far());
    let main_body = &out[out.find("fn main").unwrap()..];
    for gone in ["P::new(", ".add(", ".get(", "twice("] {
        assert!(!main_body.contains(gone), "{gone} should be inlined:\n{out}");
    }
    assert_prints(&out, "11\n");
}

#[test]
fn inlining_is_refused_when_the_behaviour_would_change() {
    let src = "fn early(v: u32) -> u32 { if v > 1 { return 10; } v }\nfn main() { println!(\"{}\", early(5) + 1); }\n";
    let o = prints("11\n");
    let out = rs_minimizer::inline::inline_calls(&o, src, &["rustc".to_string()], far());
    assert!(out.contains("early(5)"), "a body with `return` must not be pasted:\n{out}");
}

#[test]
fn the_renamed_program_with_modules_traits_and_generics_becomes_plain_code() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/renamed.rs")).unwrap();
    let o = prints("0\n");
    check_oracle(&o, &src);
    rs_minimizer::control::set_parallel(8);
    let opts = Options::new(Duration::from_secs(180));
    let mut log = ReduceLog::default();
    let out = minimize(&o, &src, &opts, &mut log, &mut |_, _, _| None);
    for gone in ["mod ", "trait ", "impl "] {
        assert!(!out.contains(gone), "`{gone}` should be gone:\n{out}");
    }
    assert!(out.lines().count() <= 12, "{out}");
    assert_prints(&out, "0\n");
}

const CAPTURE: &str = "struct d(i8);\ntrait a {\n    fn a(&self, c: &mut d);\n}\nimpl a for i8 {\n    fn a(&self, c: &mut d) {\n        c.0 = b(*self);\n    }\n}\nfn b(x: i8) -> i8 {\n    x + 1\n}\nfn main() {\n    let mut c = d(0);\n    let b = 41i8;\n    b.a(&mut c);\n    println!(\"{}\", c.0);\n}\n";

#[test]
fn locals_named_like_items_are_renamed_so_that_pasted_bodies_are_not_captured() {
    let o = prints("42\n");
    check_oracle(&o, CAPTURE);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::inline::uniquify_locals(&o, CAPTURE, &["rustc".to_string()]);
    assert!(out.contains("b_l"), "the local `b` must be renamed:\n{out}");
    assert_prints(&out, "42\n");
}

#[test]
fn a_trait_method_call_whose_body_uses_a_shadowed_function_is_inlined_and_the_trait_goes() {
    let o = prints("42\n");
    rs_minimizer::control::set_parallel(8);
    let opts = Options::new(Duration::from_secs(120));
    let mut log = ReduceLog::default();
    let out = minimize(&o, CAPTURE, &opts, &mut log, &mut |_, _, _| None);
    assert!(!out.contains("trait ") && !out.contains("impl "), "{out}");
    assert_prints(&out, "42\n");
}

#[test]
fn blocks_and_tuple_lets_left_by_inlining_are_flattened() {
    let src = "fn main() {\n    let x = { 5 };\n    { let (a, b, c) = (1, 2, x); println!(\"{}\", c); };\n    { let y = 1; let _ = y; }\n}\n";
    let o = prints("5\n");
    check_oracle(&o, src);
    let out = rs_minimizer::exprs::simplify_let_tuples(&o, src, far());
    assert!(!out.contains("(a, b, c)"), "{out}");
    let out = rs_minimizer::exprs::flatten_blocks(&o, &out, far());
    assert!(out.matches('{').count() < src.matches('{').count(), "{out}");
    assert_prints(&out, "5\n");
}

#[test]
fn an_unused_binding_keeps_only_its_initialiser() {
    let src = "fn main() {\n    let mut n = 1;\n    let ok = { n += 41; true };\n    let keep = n;\n    println!(\"{}\", keep);\n}\n";
    let o = prints("42\n");
    check_oracle(&o, src);
    let out = rs_minimizer::exprs::drop_let_bindings(&o, src, far());
    assert!(!out.contains("let ok"), "{out}");
    assert!(out.contains("let keep") || out.contains("let mut n"), "used bindings stay:\n{out}");
    assert_prints(&out, "42\n");
}

#[test]
fn statements_nobody_reads_are_deleted() {
    let src = "fn main() {\n    let mut v = vec![1u32];\n    v.push(2);\n    let unused = v.len() * 3;\n    if v.len() > 5 { println!(\"big\"); }\n    println!(\"{}\", v.iter().sum::<u32>());\n}\n";
    let o = prints("3\n");
    check_oracle(&o, src);
    let out = rs_minimizer::items::shrink_statements(&o, src, far());
    assert!(!out.contains("unused") && !out.contains("big"), "{out}");
    assert_prints(&out, "3\n");
}

#[test]
fn single_use_lets_are_substituted() {
    let src = "fn main() {\n    let a = 40u32;\n    let b = a + 1;\n    let c = &b;\n    println!(\"{}\", *c + 1);\n}\n";
    let o = prints("42\n");
    check_oracle(&o, src);
    let out = rs_minimizer::inline::substitute_lets(&o, src, far());
    assert!(out.matches("let ").count() < src.matches("let ").count(), "{out}");
    assert_prints(&out, "42\n");
}

#[test]
fn dead_code_is_found_even_when_the_crate_allows_it() {
    let src = "#![allow(dead_code, unused)]\nfn used() -> u32 { 7 }\nfn never() -> u32 { 1 }\nuse std::collections::HashMap;\nfn main() { println!(\"{}\", used()); }\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = remove_dead_code(&o, src, &["rustc".to_string()], far());
    assert!(!out.contains("never") && !out.contains("HashMap"), "{out}");
    assert_prints(&out, "7\n");
}

#[test]
fn a_hook_that_loses_the_bug_is_discarded() {
    let src = "fn main() {\n    let v = 6u32 * 7;\n    println!(\"{}\", v);\n}\n";
    let o = prints("42\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let opts = Options::new(Duration::from_secs(60));
    let mut log = ReduceLog::default();
    // a hook that "rebuilds" the program and breaks it
    let out = minimize(&o, src, &opts, &mut log, &mut |stage, cur, _| (stage == rs_minimizer::Stage::Flatten).then(|| cur.replace("42", "43").replace("6u32 * 7", "6u32 * 8")));
    assert_prints(&out, "42\n");
    assert!(log.0.iter().any(|l| l.contains("not interesting - discarded")), "{:?}", log.0);
}

#[test]
fn the_effort_shuffle_is_deterministic_and_a_permutation() {
    let mut a: Vec<u32> = (0..20).collect();
    let mut b = a.clone();
    rs_minimizer::control::shuffled(&mut a, 3);
    rs_minimizer::control::shuffled(&mut b, 3);
    assert_eq!(a, b);
    let mut sorted = a.clone();
    sorted.sort();
    assert_eq!(sorted, (0..20).collect::<Vec<u32>>());
    assert_ne!(a, (0..20).collect::<Vec<u32>>());
}

#[test]
fn pairs_and_random_subsets_run_at_a_higher_effort() {
    let src = "fn main() {\n    let mut c = 0u32;\n    c += 1;\n    c += 2;\n    let d = c * 0;\n    let _ = d;\n    println!(\"ok\");\n}\n";
    let o = prints("ok\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    rs_minimizer::control::set_effort(2);
    let out = rs_minimizer::items::shrink_pairs(&o, src, far());
    let out = rs_minimizer::items::shrink_siblings(&o, &out, far());
    rs_minimizer::control::set_effort(0);
    assert!(!out.contains("c += 2"), "{out}");
    assert_prints(&out, "ok\n");
}

#[test]
fn constants_are_folded_and_integers_made_smaller() {
    let src = "fn main() {\n    let a = 1000u32 + 234u32;\n    let b = 64 / 8;\n    println!(\"{}\", (a > 5) as u32 + b - 8 + 3);\n}\n";
    let o = prints("4\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::peep::peep(&o, src, far());
    assert!(!out.contains("1000u32 + 234u32") && !out.contains("64 / 8"), "{out}");
    assert_prints(&out, "4\n");
}

#[test]
fn declared_names_get_single_letters() {
    let src = "fn compute_value(first_input: u32) -> u32 {\n    let doubled = first_input * 2;\n    doubled + 1\n}\nfn main() {\n    let result_total = compute_value(20);\n    println!(\"{}\", result_total);\n}\n";
    let o = prints("41\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::inline::shorten_names(&o, src, far());
    assert!(!out.contains("compute_value") && !out.contains("first_input") && !out.contains("result_total") && out.contains("fn main"), "{out}");
    assert_prints(&out, "41\n");
}

#[test]
fn a_run_that_finds_nothing_hands_the_input_back_unchanged() {
    let src = "fn main() {\n    println!(\"{}\", 6 * 7);\n}\n";
    let o = prints("42\n");
    rs_minimizer::control::set_parallel(8);
    let opts = Options::new(Duration::from_secs(40));
    let mut log = ReduceLog::default();
    let out = minimize(&o, src, &opts, &mut log, &mut |_, _, _| None);
    assert_prints(&out, "42\n");
    assert!(out.lines().count() <= src.lines().count(), "never bigger than what it was given:\n{out}");
}
