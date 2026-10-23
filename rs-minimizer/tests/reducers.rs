mod common;
use common::*;
use rs_minimizer::peep::peep;
use rs_minimizer::structure::*;
use rs_minimizer::tokens::{shrink_tokens, tokenize};
use rs_minimizer::{Options, ReduceLog, minimize};
use std::time::Duration;

#[test]
fn tokenizer_keeps_operators_lifetimes_and_strings_whole() {
    let src = "fn f<'a>(x: &'a str) -> u8 { let c = 'x'; if x == \"a b\" && c != 'y' { 1 } else { 2 } } ::std::mem::drop;";
    let toks: Vec<&str> = tokenize(src).into_iter().map(|(s, e)| &src[s..e]).collect();
    for want in ["'a", "->", "'x'", "\"a b\"", "==", "&&", "!=", "::"] {
        assert!(toks.contains(&want), "{want} missing in {toks:?}");
    }
}

#[test]
fn tokens_that_do_nothing_are_deleted() {
    let src = "fn main() {\n    let x: u32 = 5 + 0 * 7;\n    let y = x + 0;\n    println!(\"{}\", y);\n}\n";
    let o = prints("5\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = shrink_tokens(&o, src, far());
    assert!(out.len() < src.len() - 8, "{out}");
    assert_prints(&out, "5\n");
}

#[test]
fn peephole_turns_dependencies_into_constants_and_drops_operands() {
    let src = "fn helper(a: u32) -> u32 { a * 2 }\nfn main() {\n    let v = helper(21) + helper(4);\n    let n = 5u32;\n    println!(\"{}\", v * 0 + 5 * 1 + n * 0);\n}\n";
    let o = prints("5\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = peep(&o, src, far());
    assert!(!out.contains("helper(21)"), "{out}");
    assert_prints(&out, "5\n");
}

#[test]
fn a_newtype_wrapper_is_replaced_by_its_field() {
    let src = "struct W(i8);\nfn bump(w: &mut W) { w.0 += 1; }\nfn main() {\n    let mut w = W(1);\n    w.0 += 1;\n    println!(\"{}\", w.0);\n}\n";
    let o = prints("2\n");
    check_oracle(&o, src);
    let out = unwrap_newtypes(&o, src, far());
    assert!(!out.contains("struct W"), "{out}");
    assert_prints(&out, "2\n");
}

#[test]
fn a_newtype_with_a_method_stays_because_the_impl_needs_it() {
    let src = "struct W(i8);\nimpl W { fn get(&self) -> i8 { self.0 } }\nfn main() { println!(\"{}\", W(4).get()); }\n";
    let o = prints("4\n");
    let out = unwrap_newtypes(&o, src, far());
    assert_eq!(out, src);
}

#[test]
fn bodies_nobody_runs_are_stubbed() {
    let src = "use std::hint::black_box;\nfn heavy(x: u32) -> u32 {\n    let mut s = 0;\n    for i in 0..x { s += i * 3; }\n    s\n}\nfn main() {\n    if black_box(false) { println!(\"{}\", heavy(10)); }\n    println!(\"ok\");\n}\n";
    let o = rs_minimizer::Cached::new(|s: &str| s.contains("fn heavy") && run_program(s).as_deref() == Some("ok\n"));
    check_oracle(&o, src);
    let out = stub_bodies(&o, src, far());
    assert!(out.contains("unimplemented!()") && !out.contains("s += i * 3"), "{out}");
}

#[test]
fn unused_parameters_go_with_their_arguments() {
    let src = "fn f(a: u32, unused: u32, c: u32) -> u32 { a + c }\nstruct S;\nimpl S { fn m(&self, keep: u32, extra: u8) -> u32 { keep } }\nfn main() {\n    println!(\"{}\", f(1, 2 + 3, 4) + S.m(10, 9));\n}\n";
    let o = prints("15\n");
    check_oracle(&o, src);
    let out = remove_params(&o, src, far());
    assert!(!out.contains("unused") && !out.contains("extra"), "{out}");
    assert!(out.contains("f(1, 4)") || out.contains("f(1,4)"), "{out}");
    assert_prints(&out, "15\n");
}

#[test]
fn unused_fields_go_with_their_initialisers() {
    let src = "struct P { x: u32, y: u32, z: u8 }\nfn main() {\n    let p = P { x: 7, y: 2, z: 1 };\n    println!(\"{}\", p.x);\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    let out = remove_fields(&o, src, far());
    assert!(!out.contains("y:") && !out.contains("z:"), "{out}");
    assert_prints(&out, "7\n");
}

#[test]
fn an_enum_variant_goes_together_with_the_arms_that_name_it() {
    let src = "enum E { A, B, C }\nfn f(e: E) -> u32 {\n    match e {\n        E::A => 1,\n        E::B => 2,\n        E::C => { 3 }\n    }\n}\nfn main() { println!(\"{}\", f(E::A)); }\n";
    let o = prints("1\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = remove_variants_and_arms(&o, src, far());
    assert!(!out.contains("E::B") && !out.contains("E::C"), "{out}");
    assert_prints(&out, "1\n");
}

/// A trait with one impl, a wrapper struct, a function that does nothing special and locals named like items: all of it is only
/// packaging around one value.
#[test]
fn the_trait_and_wrapper_example_becomes_a_few_lines_of_plain_code() {
    let src = "trait a {\nfn a(&self, a:&mut c);\n}\nstruct c(i8);\nimpl a for i8 {\nfn a(&self, a_l:&mut c) {\na_l.0 = b()\n}\n}\nfn b() -> i8 { 42 }\nfn main() {\nlet(a, b) = (&mut c(0), 0);\n0.a(a);\nprint!(\"{}\", a.0)\n}\n";
    let o = prints("42");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let opts = Options::new(Duration::from_secs(120));
    let mut log = ReduceLog::default();
    let out = minimize(&o, src, &opts, &mut log, &mut |_, _, _| None);
    for gone in ["trait ", "impl ", "struct "] {
        assert!(!out.contains(gone), "`{gone}` should be gone:\n{out}");
    }
    assert!(out.lines().count() <= 5, "{out}");
    assert_prints(&out, "42");
}

fn prints_ending_in(end: &'static str) -> rs_minimizer::oracle::Cached<impl Fn(&str) -> bool + Sync> {
    let mut o = rs_minimizer::oracle::Cached::new(move |src: &str| run_program(src).is_some_and(|out| out.ends_with(end)));
    o.work = scratch();
    o
}

#[test]
fn peephole_drops_format_arguments_and_replaces_call_arguments_by_literals() {
    let src = "fn f(a: u32) -> u32 { a }\nfn main() {\n    let h = 7u32;\n    println!(\"{} {}\", f(3 + h * 0 + 1), h);\n}\n";
    let o = prints_ending_in("7\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = peep(&o, src, far());
    assert!(out.contains("println!(\"{}\", h)") || out.contains("println!(\"{}\", 7"), "{out}");
    assert!(!out.contains("3 +"), "{out}");
}

#[test]
fn a_dereferenced_borrow_is_cancelled() {
    let src = "fn main() {\n    let b = 7u32;\n    let c = *(&b);\n    println!(\"{}\", c);\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = peep(&o, src, far());
    assert!(!out.contains("*(&"), "{out}");
}

#[test]
fn a_reference_let_is_substituted_even_when_the_name_is_bound_again_later() {
    let src = "fn main() {\n    let mut h = 0;\n    let b = 7u32;\n    let v = &b;\n    let c = v;\n    let (c, v) = (&mut h, *c);\n    *c = v;\n    println!(\"{}\", h);\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::inline::substitute_lets(&o, src, far());
    assert!(!out.contains("let v = &b") && !out.contains("let c = "), "{out}");
}

#[test]
fn a_tuple_let_is_split_and_its_parts_substituted() {
    let src = "fn main() {\n    let mut h = 0;\n    let b = 7u32;\n    let (c, v) = (&mut h, b);\n    *c = v;\n    println!(\"{}\", h);\n}\n";
    let o = prints("7\n");
    check_oracle(&o, src);
    rs_minimizer::control::set_parallel(8);
    let out = rs_minimizer::exprs::simplify_let_tuples(&o, src, far());
    assert!(out.contains("let c = &mut h;"), "{out}");
}
