mod common;
use common::*;
use rs_minimizer::unroll::{drop_catch_unwind, expand_macros, unroll};

#[test]
fn statement_macro_with_literal_and_ident_is_expanded() {
    let src = "macro_rules! go { ($id:literal, $m:ident) => {{ let v = $m::f(); println!(\"{} {}\", $id, v); }}; }\nmod a { pub fn f() -> u32 { 7 } }\nfn main() {\n    go!(3, a);\n}\n";
    let o = prints("3 7\n");
    check_oracle(&o, src);
    let out = expand_macros(&o, src);
    assert!(!out.contains("macro_rules"), "{out}");
    assert!(!out.contains("go!"), "{out}");
    assert_prints(&out, "3 7\n");
}

#[test]
fn repeated_item_macro_with_and_without_commas() {
    let src = "trait T { fn t(&self) -> u32; }\nmacro_rules! imp { ($($ty:ty),*) => { $(impl T for $ty { fn t(&self) -> u32 { *self as u32 } })* } }\nmacro_rules! imp2 { ($($ty:ty)*) => { $(impl T for$ty { fn t(&self) -> u32 { *self as u32 + 1 } })* } }\nimp!(u8, u16);\nimp2!(u32);\nfn main() { println!(\"{} {} {}\", 1u8.t(), 2u16.t(), 3u32.t()); }\n";
    let o = prints("1 2 4\n");
    check_oracle(&o, src);
    let out = expand_macros(&o, src);
    assert!(!out.contains("macro_rules"), "{out}");
    assert!(out.contains("impl T for u8") && out.contains("impl T for u32"), "{out}");
    assert_prints(&out, "1 2 4\n");
}

#[test]
fn macros_with_the_same_name_are_expanded_by_their_own_definition() {
    let src = "fn a(x: u32) -> u32 { x }\nmacro_rules!a { ($x:literal) => { $x + 100 } }\nfn first() -> u32 { a!(1) }\nmacro_rules!a { ($x:literal, $y:literal) => { $x * $y } }\nfn second() -> u32 { a!(6, 7) }\nfn main() { println!(\"{} {}\", first(), second()); }\n";
    let o = prints("101 42\n");
    check_oracle(&o, src);
    let out = expand_macros(&o, src);
    assert!(!out.contains("macro_rules"), "{out}");
    assert_prints(&out, "101 42\n");
}

#[test]
fn macro_that_the_bug_depends_on_is_kept() {
    let src = "macro_rules! m { () => { 5 } }\nfn main() { println!(\"{}\", m!()); }\n";
    let o = rs_minimizer::Cached::new(|s: &str| s.contains("macro_rules") && common::run_program(s).as_deref() == Some("5\n"));
    check_oracle(&o, src);
    let out = expand_macros(&o, src);
    assert_eq!(out, src);
}

#[test]
fn unsupported_macros_are_left_alone() {
    let src = "macro_rules! m { (a) => { 1 }; (b) => { 2 }; }\nfn main() { println!(\"{}\", m!(a)); }\n";
    let o = prints("1\n");
    check_oracle(&o, src);
    assert_eq!(expand_macros(&o, src), src);
}

const WRAPPED: &str = "fn work(v: &mut Vec<u32>) { v.push(4); }\nfn main() {\n    let mut v = vec![1];\n    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut v))).is_ok();\n    println!(\"{ok} {v:?}\");\n}\n";

#[test]
fn catch_unwind_without_a_panic_is_removed() {
    let o = prints("true [1, 4]\n");
    check_oracle(&o, WRAPPED);
    let out = drop_catch_unwind(&o, WRAPPED);
    assert!(!out.contains("catch_unwind"), "{out}");
    assert_prints(&out, "true [1, 4]\n");
}

#[test]
fn catch_unwind_around_a_panic_stays() {
    let src = "fn work() { if std::hint::black_box(true) { panic!(\"boom\"); } }\nfn main() {\n    std::panic::set_hook(Box::new(|_| {}));\n    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work())).is_ok();\n    println!(\"{ok}\");\n}\n";
    let o = prints("false\n");
    check_oracle(&o, src);
    let out = unroll(&o, src);
    assert!(out.contains("catch_unwind"), "{out}");
    assert!(out.contains("set_hook"), "{out}");
    assert_prints(&out, "false\n");
}

#[test]
fn only_the_wrappers_that_can_go_are_removed() {
    let src = "fn ok_fn() {}\nfn bad_fn() { if std::hint::black_box(true) { panic!(\"x\"); } }\nfn main() {\n    std::panic::set_hook(Box::new(|_| {}));\n    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ok_fn())).is_ok();\n    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bad_fn())).is_ok();\n    println!(\"{a} {b}\");\n}\n";
    let o = prints("true false\n");
    check_oracle(&o, src);
    let out = drop_catch_unwind(&o, src);
    assert_eq!(out.matches("catch_unwind").count(), 1, "{out}");
    assert_prints(&out, "true false\n");
}
