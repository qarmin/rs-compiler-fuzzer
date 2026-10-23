mod common;
use common::*;
use rs_minimizer::items::{attribute_ranges, item_ranges, shrink_items};
use rs_minimizer::lines::{shrink_lines, strip_comments};
use rs_minimizer::modules::hoist_modules;

#[test]
fn modules_are_hoisted_with_their_paths_and_uses() {
    let src = "mod util {\n    pub fn double(x: u32) -> u32 { x * 2 }\n    pub mod inner { pub fn one() -> u32 { 1 } }\n}\nmod app {\n    use super::util::double;\n    pub fn run() -> u32 { double(21) }\n}\nuse app::*;\nfn main() { println!(\"{} {}\", run(), util::double(1)); }\n";
    let o = prints("42 2\n");
    check_oracle(&o, src);
    let out = hoist_modules(&o, src, &|_| false, &[]);
    assert!(!out.contains("mod util") && !out.contains("mod app"), "{out}");
    assert_prints(&out, "42 2\n");
}

#[test]
fn modules_whose_names_collide_stay_modules() {
    let src = "mod a { pub fn f() -> u32 { 1 } }\nmod b { pub fn f() -> u32 { 2 } }\nfn main() { println!(\"{} {}\", a::f(), b::f()); }\n";
    let o = prints("1 2\n");
    check_oracle(&o, src);
    let out = hoist_modules(&o, src, &|_| false, &[]);
    assert_prints(&out, "1 2\n");
    assert!(out.contains("mod a") || out.contains("mod b"), "at least one module must stay to keep the names apart:\n{out}");
}

#[test]
fn kept_modules_are_not_touched() {
    let src = "mod case_1 { pub fn f() -> u32 { 1 } }\nmod rt { pub fn g() -> u32 { 2 } }\nfn main() { println!(\"{} {}\", case_1::f(), rt::g()); }\n";
    let o = prints("1 2\n");
    let out = hoist_modules(&o, src, &|n| n.starts_with("case_"), &[]);
    assert!(out.contains("mod case_1"), "{out}");
    assert!(!out.contains("mod rt"), "{out}");
    assert_prints(&out, "1 2\n");
}

#[test]
fn item_ranges_find_nested_items_and_skip_expressions() {
    let src = "#[derive(Debug)]\npub struct S(u32);\nimpl S { pub fn get(&self) -> u32 { self.0 } const K: u32 = { 3 }; }\nstatic X: [u8; 2] = [1, 2];\nfn main() { let c = const { 5 }; let f = |x: u32| x; fn inner() {} println!(\"{}\", c + f(1)); }\n";
    let items = item_ranges(src);
    let texts: Vec<&str> = items.iter().map(|i| &src[i.start..i.end]).collect();
    assert!(texts.contains(&"pub struct S(u32);"), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("impl S {")), "{texts:?}");
    assert!(texts.contains(&"pub fn get(&self) -> u32 { self.0 }"), "{texts:?}");
    assert!(texts.contains(&"const K: u32 = { 3 };"), "{texts:?}");
    assert!(texts.contains(&"static X: [u8; 2] = [1, 2];"), "{texts:?}");
    assert!(texts.contains(&"fn inner() {}"), "{texts:?}");
    assert!(!texts.iter().any(|t| t.starts_with("const {")), "{texts:?}");
    assert_eq!(attribute_ranges(src).len(), 1);
}

#[test]
fn whole_unused_items_go_in_one_step_and_used_ones_stay() {
    let src = "#![allow(dead_code)]\n#[derive(Debug, Clone)]\nstruct Unused { a: u32 }\ntrait Gone { fn g(&self); }\nimpl Gone for u8 { fn g(&self) {} }\nfn helper() -> u32 { 5 }\nfn dead() -> u32 { 9 }\nuse std::collections::HashMap;\nfn main() { println!(\"{}\", helper()); }\n";
    let o = prints("5\n");
    check_oracle(&o, src);
    let out = shrink_items(&o, src, far());
    for gone in ["Unused", "Gone", "dead", "HashMap", "derive"] {
        assert!(!out.contains(gone), "{gone} should be gone:\n{out}");
    }
    assert!(out.contains("fn helper"), "{out}");
    assert_prints(&out, "5\n");
}

#[test]
fn an_item_the_oracle_needs_is_kept_with_its_attribute() {
    let src = "#[inline(never)]\nfn keep_me() -> u32 { 3 }\nfn junk() {}\nfn main() { println!(\"{}\", keep_me()); }\n";
    let o = rs_minimizer::Cached::new(|s: &str| s.contains("#[inline(never)]") && run_program(s).as_deref() == Some("3\n"));
    check_oracle(&o, src);
    let out = shrink_items(&o, src, far());
    assert!(out.contains("#[inline(never)]") && out.contains("keep_me") && !out.contains("junk"), "{out}");
}

#[test]
fn lines_are_shrunk_in_parallel_to_the_minimum() {
    rs_minimizer::control::set_parallel(8);
    let mut src = String::from("fn main() {\n    let mut s = 0u64;\n");
    for i in 0..120 {
        src.push_str(&format!("    let junk{i} = {i}u64 * 3;\n"));
    }
    src.push_str("    s += 41;\n    s += 1;\n");
    for i in 120..200 {
        src.push_str(&format!("    let junk{i} = {i}u64;\n"));
    }
    src.push_str("    println!(\"{s}\");\n}\n");
    let o = prints("42\n");
    check_oracle(&o, &src);
    let out = shrink_lines(&o, &src, far());
    assert_eq!(out.lines().count(), 6, "{out}");
    assert_prints(&out, "42\n");
}

#[test]
fn comments_and_blank_lines_are_stripped() {
    let src = "// header\nfn main() {\n\n    // say it\n    println!(\"1\");\n\n}\n";
    let o = prints("1\n");
    let out = strip_comments(&o, src);
    assert_eq!(out, "fn main() {\n    println!(\"1\");\n}\n");
}

#[test]
fn modules_with_colliding_names_are_hoisted_after_renaming() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/renamed.rs")).unwrap();
    let o = prints("0\n");
    check_oracle(&o, &src);
    let out = hoist_modules(&o, &src, &|_| false, &["rustc".to_string()]);
    assert!(!out.contains("mod d") && !out.contains("mod a"), "{out}");
    assert_prints(&out, "0\n");
}

#[test]
fn an_item_goes_together_with_the_statements_that_use_it() {
    use rs_minimizer::items::shrink_items_with_users;
    let mut src = String::new();
    for i in 0..30 {
        src.push_str(&format!("mod case_{i} {{ pub fn run() -> u32 {{ {i} }} }}\n"));
    }
    src.push_str("fn main() {\n    let mut t = 0u32;\n");
    for i in 0..30 {
        src.push_str(&format!("    t += case_{i}::run();\n"));
    }
    src.push_str("    println!(\"{}\", t);\n}\n");
    let o = rs_minimizer::Cached::new(|s: &str| s.contains("case_7::run") && run_program(s).is_some_and(|out| out.trim().parse::<u32>().is_ok_and(|n| n >= 7)));
    check_oracle(&o, &src);
    rs_minimizer::control::set_parallel(8);
    let out = shrink_items_with_users(&o, &src, far());
    assert!(out.contains("case_7") && !out.contains("case_8") && !out.contains("case_29"), "{out}");
    assert_eq!(out.matches("mod case_").count(), 1, "{out}");
}
