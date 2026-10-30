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

#[test]
fn a_block_statement_ending_in_an_expression_is_spliced_with_a_semicolon() {
    let src = "fn main() {\n    let h;\n    {\n        let a = 4u32;\n        h = a * 2\n    }\n    println!(\"{}\", h)\n}\n";
    let o = prints("8\n");
    check_oracle(&o, src);
    let out = rs_minimizer::exprs::flatten_blocks(&o, src, far());
    assert!(!out.contains("{\n        let a"), "{out}");
    assert_prints(&out, "8\n");
}

#[test]
fn a_deferred_initialiser_moves_into_the_let() {
    let src = "fn main() {\n    let h: u64;\n    let a = std::hint::black_box(3u64);\n    h = a * 2;\n    println!(\"{}\", h)\n}\n";
    let o = prints("6\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("let h;") && !out.contains("let h: u64;"), "{out}");
    assert_prints(&out, "6\n");
}

#[test]
fn a_cast_of_a_literal_is_a_literal() {
    let src = "fn main() {\n    let a = std::hint::black_box(3u32);\n    println!(\"{}\", a.rotate_left(32u8 as u32) + (7 as u32));\n}\n";
    let o = prints("10\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains(" as "), "{out}");
    assert_prints(&out, "10\n");
}

#[test]
fn literals_after_a_string_are_still_shrunk() {
    let src = "fn main() {\n    println!(\"{}\", 1000u32 / 10);\n    println!(\"{}\", 100u32 + 0);\n}\n";
    let o = rs_minimizer::Cached::new(|s: &str| s.contains("println!") && run_program(s).is_some());
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("1000") && !out.contains("100u32"), "{out}");
}

#[test]
fn a_dereferenced_borrow_of_a_name_is_the_name() {
    let src = "fn main() {\n    let mut h = 1u32;\n    *&mut h = 5;\n    println!(\"{}\", *&h);\n}\n";
    let o = prints("5\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("*&"), "{out}");
    assert_prints(&out, "5\n");
}

#[test]
fn an_unused_parameter_goes_from_a_function_called_through_a_pointer() {
    let src = "use std::hint::black_box;\nfn f(a: i128, unused: i128, n: u32) -> i128 { a + n as i128 }\nfn main() {\n    let e: fn(i128, i128, u32) -> i128 = black_box(f);\n    println!(\"{}\", e(5, 0, 2));\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = remove_params(&o, src, far());
    assert!(!out.contains("unused"), "{out}");
    assert!(out.contains("fn(i128, u32)"), "{out}");
    assert_prints(&out, "7\n");
}

#[test]
fn calls_of_a_trait_method_with_several_impls_are_inlined() {
    let src = "trait F { fn feed(&self, h: &mut u64); }\nimpl F for u32 { fn feed(&self, h: &mut u64) { *h ^= *self as u64 } }\nimpl F for u64 { fn feed(&self, h: &mut u64) { *h = h.wrapping_add(*self) } }\nfn main() {\n    let mut h = 1u64;\n    5u32.feed(&mut h);\n    7u64.feed(&mut h);\n    println!(\"{}\", h);\n}\n";
    let o = prints("11\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::inline::inline_calls(&o, src, &["rustc".to_string()], far());
    let main_body = &out[out.find("fn main").unwrap()..];
    assert!(!main_body.contains(".feed("), "{out}");
    assert_prints(&out, "11\n");
}

#[test]
fn a_trait_default_body_does_not_hide_the_only_impl_from_inlining() {
    let src = "trait F { fn feed(&self, h: &mut u64) {} }\nimpl F for u32 { fn feed(&self, h: &mut u64) { *h ^= *self as u64 } }\nfn main() {\n    let mut h = 1u64;\n    6u32.feed(&mut h);\n    println!(\"{}\", h);\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::inline::inline_calls(&o, src, &["rustc".to_string()], far());
    let main_body = &out[out.find("fn main").unwrap()..];
    assert!(!main_body.contains(".feed("), "{out}");
    assert_prints(&out, "7\n");
}

#[test]
fn a_deferred_mutable_initialiser_moves_into_the_let() {
    let src = "fn main() {\n    let mut h;\n    let a = std::hint::black_box(3u64);\n    h = a * 2;\n    h += 1;\n    println!(\"{}\", h)\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    assert!(!out.contains("let mut h;"), "{out}");
    assert_prints(&out, "7\n");
}

#[test]
fn a_method_call_on_a_multiline_chain_is_inlined() {
    let src = "trait F { fn feed(&self, h: &mut u64); }\nimpl F for i16 { fn feed(&self, h: &mut u64) { *h = *h ^ *self as u64 } }\nfn main() {\n    let mut h = 1u64;\n    (3i16)\n        .wrapping_add(1)\n        .checked_mul(2)\n        .unwrap_or(0)\n        .feed(&mut h);\n    println!(\"{}\", h);\n}\n";
    let o = prints("9\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(4);
    let out = rs_minimizer::inline::inline_calls(&o, src, &["rustc".to_string()], far());
    let main_body = &out[out.find("fn main").unwrap()..];
    assert!(!main_body.contains(".feed("), "{out}");
    assert_prints(&out, "9\n");
}

#[test]
fn a_loop_over_one_element_and_an_identity_match_go() {
    let src = "fn f(x: u32) -> u32 { x * 2 }\nfn main() {\n    for s in [4u32] {\n        println!(\"{}\", match f(s) { k => k });\n    }\n}\n";
    let o = prints("8\n");
    check_oracle(&o, src);
    let out = peep(&o, src, far());
    let out = rs_minimizer::exprs::flatten_blocks(&o, &out, far());
    assert!(!out.contains("for ") && !out.contains("match "), "{out}");
    assert_prints(&out, "8\n");
}
