mod common;
use common::*;
use rs_minimizer::traits::{monomorphize, remove_bounds, remove_trait_methods, specialize_trait_paths};

const SRC: &str = include_str!("data/generic_trait.rs");

fn expected() -> String {
    run_program(SRC).expect("the sample runs")
}

#[test]
fn trait_paths_point_at_the_only_impl() {
    let want: &'static str = Box::leak(expected().into_boxed_str());
    let o = prints(want);
    let out = specialize_trait_paths(&o, SRC, far());
    assert!(out.contains("i128::from_u64(2961832378146261130u64)"), "{out}");
}

#[test]
fn generic_parameters_become_the_implemented_type() {
    let want: &'static str = Box::leak(expected().into_boxed_str());
    let o = prints(want);
    let out = monomorphize(&o, SRC, far());
    assert!(out.contains("fn e("), "{out}");
    assert!(!out.contains("<T"), "{out}");
}

#[test]
fn single_trait_methods_and_bounds_go() {
    let src = "trait A { fn used(&self) -> u32; fn spare(&self) -> u32; }\nstruct S;\nimpl A for S { fn used(&self) -> u32 { 3 } fn spare(&self) -> u32 { 4 } }\nfn f<T: A + Copy + Clone>(t: T) -> u32 { t.used() }\n#[derive(Clone, Copy)]\nstruct Q;\nimpl A for Q { fn used(&self) -> u32 { 3 } fn spare(&self) -> u32 { 5 } }\nfn main() { println!(\"{}\", f(Q)); }\n";
    let o = prints("3\n");
    let out = remove_trait_methods(&o, src, far());
    assert!(!out.contains("spare"), "{out}");
    assert!(out.contains("used"), "{out}");
    let out = remove_bounds(&o, &out, far());
    assert!(out.contains("fn f<T: A") && !out.contains("A +"), "{out}");
}

#[test]
fn whole_pipeline_dissolves_the_trait() {
    let want: &'static str = Box::leak(expected().into_boxed_str());
    let o = prints(want);
    let mut log = rs_minimizer::ReduceLog::default();
    let mut hook = |_: rs_minimizer::Stage, _: &str, _: std::time::Instant| None;
    let opts = rs_minimizer::Options::new(std::time::Duration::from_secs(120));
    let out = rs_minimizer::minimize(&o, SRC, &opts, &mut log, &mut hook);
    assert!(!out.contains("trait") && !out.contains("impl"), "{out}");
    assert_prints(&out, want);
}

#[test]
fn receivers_that_are_blocks_and_trivial_matches() {
    use rs_minimizer::exprs::flatten_blocks;
    let src = "fn main() { let x = match 0 { _ => 5u32 }; println!(\"{}\", x); }\n";
    let o = prints("5\n");
    let out = flatten_blocks(&o, src, far());
    assert!(!out.contains("match"), "{out}");
}
