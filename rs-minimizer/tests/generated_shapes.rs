//! Shapes that generated programs have and the passes used to skip: `{:?}` in `main`, `extern` functions, constant conditions.
mod common;
use common::*;
use rs_minimizer::inline::substitute_lets;
use rs_minimizer::peep::peep;
use rs_minimizer::structure::remove_params;

#[test]
fn a_question_mark_in_a_format_string_does_not_hide_main_from_the_let_passes() {
    let src = "fn main() {\n    let a = 5u32;\n    let b = a;\n    println!(\"{:?}\", b + 1);\n}\n";
    let o = prints("6\n");
    check_oracle(&o, src);
    let out = substitute_lets(&o, src, far());
    assert!(!out.contains("let "), "{out}");
    assert_prints(&out, "6\n");
}

#[test]
fn unused_parameters_of_extern_functions_go_with_their_arguments() {
    let src = "extern \"C\" fn f(a: u32, unused: i128, b: bool) -> u32 { if b { a } else { 0 } }\nfn main() { println!(\"{}\", f(3, 0, true)); }\n";
    let o = prints("3\n");
    check_oracle(&o, src);
    let out = remove_params(&o, src, far());
    assert!(!out.contains("unused") && !out.contains("i128"), "{out}");
    assert_prints(&out, "3\n");
}

#[test]
fn unused_parameters_go_from_functions_that_return_early_or_recurse() {
    let src = "fn f(a: u32, unused: u8) -> u32 { if a == 0 { return 1; } a }\nfn main() { println!(\"{}\", f(3, 9)); }\n";
    let o = prints("3\n");
    let out = remove_params(&o, src, far());
    assert!(!out.contains("unused"), "{out}");
}

#[test]
fn constant_conditions_and_calling_conventions_are_simplified() {
    let src = "extern \"win64\" fn f(c: bool) -> u32 { if c { 7 } else { 8 } }\nfn main() { println!(\"{}\", f(0 == 0)); }\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("win64") && !out.contains("0 == 0"), "{out}");
    assert_prints(&out, "7\n");
}

#[test]
fn a_comparison_becomes_true_or_false() {
    let src = "fn f(c: bool) -> u32 { if c { 7 } else { 8 } }\nfn main() { let d = std::hint::black_box(3u32); println!(\"{}\", f(d * 2 > 1)); }\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(out.contains("f(true)") || out.contains("f( true )"), "{out}");
}

#[test]
fn a_long_unsigned_literal_becomes_max() {
    let src = "fn main() { println!(\"{}\", 18446744073709551615u64 == u64::MAX); }\n";
    let o = prints("true\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("18446744073709551615"), "{out}");
}

#[test]
fn loops_over_empty_arrays_are_removed_whole() {
    let src = "fn main() {\n    for x in [0u32] { println!(\"{}\", x); }\n    for y in [] { let z: u32 = y; println!(\"{}\", z + 1u32); }\n    for y in [ ] { let w: u8 = y; println!(\"{}\", w); }\n}\n";
    let o = prints("0\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("in []") && !out.contains("in [ ]"), "{out}");
    assert_prints(&out, "0\n");
}

#[test]
fn a_module_with_a_same_shaped_twin_of_a_root_item_is_hoisted_with_renames() {
    let src = "struct G(u32);\nimpl Drop for G { fn drop(&mut self) { println!(\"root {}\", self.0) } }\nmod m {\n    use super::*;\n    struct G(u32);\n    impl Drop for G { fn drop(&mut self) { println!(\"mod {}\", self.0) } }\n    pub fn run() { let _g = G(2); }\n}\nfn main() { let _a = G(1); m::run(); }\n";
    let o = prints("mod 2\nroot 1\n");
    check_oracle(&o, src);
    let out = rs_minimizer::modules::hoist_modules(&o, src, &|_| false, &["rustc".to_string()]);
    assert!(!out.contains("mod m"), "{out}");
    assert_prints(&out, "mod 2\nroot 1\n");
}

#[test]
fn a_catch_unwind_statement_loses_its_wrapper() {
    let src = "fn main() {\n    let mut h = 1u64;\n    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {\n        h += 2;\n        h * 3\n    }));\n    println!(\"{}\", h)\n}\n";
    let o = prints("3\n");
    check_oracle(&o, src);
    let out = rs_minimizer::unroll::drop_catch_unwind(&o, src);
    assert!(!out.contains("catch_unwind"), "{out}");
    assert_prints(&out, "3\n");
}

#[test]
fn a_unit_assignment_and_a_mutable_reborrow_binding_go() {
    let src = "fn main() {\n    let mut h = 1u64;\n    () = {\n        let r = &mut h;\n        *r = *r + 2;\n        *r = *r * 3\n    };\n    println!(\"{}\", h)\n}\n";
    let o = prints("9\n");
    check_oracle(&o, src);
    let out = rs_minimizer::inline::substitute_lets(&o, src, far());
    assert!(!out.contains("let r"), "{out}");
    let out = peep(&o, &out, far());
    assert!(!out.contains("() ="), "{out}");
    assert_prints(&out, "9\n");
}
