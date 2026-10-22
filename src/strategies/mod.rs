//! Program generation strategies. Each produces the body of one `mod case_N`:
//! self-contained items plus `pub fn run(h: &mut H)`, which feeds every observable
//! result into the hasher `h`.
//!
//! Hard rules for every strategy (a generator that produces UB finds only noise):
//! - no UB, no data races, no address/time/hash-order dependent output;
//! - arithmetic goes through `wrapping_*`/`checked_*`/`saturating_*`, shifts are masked,
//!   divisors are forced non-zero, so results do not depend on overflow-checks;
//! - every loop is bounded; inputs are passed through `bb()` (black_box) so the optimiser
//!   cannot fold the whole case away at compile time;
//! - `unsafe` is only used in patterns whose validity is argued next to the generator.

use crate::rng::Rng;

mod abi;
mod arith;
mod asyncs;
mod borrows;
mod collections;
mod control;
mod enums;
mod floats;
mod generics;
mod loops;
mod memory;
mod recursion;
mod select;
mod simd;
mod stdlib;
mod stdlib_coll;
mod stdlib_float;
mod stdlib_int;
mod stdlib_misc;
mod stdlib_more;
mod stdlib_seq;
mod stdlib_text;
mod traits;
mod unwind;

pub type GenFn = fn(&mut Rng) -> String;

pub struct Strategy {
    pub name: &'static str,
    pub about: &'static str,
    /// Default batch size in lines: tuned so that the slowest configuration (fat LTO) compiles the batch in roughly 60-150 s.
    pub lines: usize,
    pub generate: GenFn,
}

pub const ALL: &[Strategy] = &[
    Strategy { name: "arith", lines: 40000, about: "typed integer expression trees, inlining variants, loops folding them", generate: arith::generate },
    Strategy { name: "loops", lines: 10000, about: "iterator chains, slicing loops, nested counters (cf. zxt5's rustc reports)", generate: loops::generate },
    Strategy { name: "control", lines: 150000, about: "random structured control flow: labeled blocks, break/continue, bool state machines", generate: control::generate },
    Strategy { name: "enums", lines: 30000, about: "pattern matching: enum tuples, guards, ranges, bindings, slice patterns (match-tree MIR opts)", generate: enums::generate },
    Strategy { name: "asyncs", lines: 30000, about: "async fns/blocks on a mini executor: state across await, drop of suspended futures, join", generate: asyncs::generate },
    Strategy { name: "collections", lines: 5000, about: "VecDeque/BTreeMap/BinaryHeap/String operation sequences", generate: collections::generate },
    Strategy { name: "select", lines: 40000, about: "structs built by if/else over clamped ints and floats (InstCombine select->min/max, #163728)", generate: select::generate },
    Strategy { name: "recursion", lines: 18000, about: "recursion combining results with max/min after the call, accumulator and mutual recursion (TailCallElim, #164469)", generate: recursion::generate },
    Strategy { name: "floats", lines: 6000, about: "IEEE arithmetic, saturating float<->int casts, fma, rounding, formatting/parsing", generate: floats::generate },
    Strategy { name: "unwind", lines: 14000, about: "panics, landing pads, drop guards, partially built arrays/vecs, catch_unwind", generate: unwind::generate },
    Strategy { name: "abi", lines: 50000, about: "extern C/sysv64/win64 calls: many arg/ret kinds, fn pointers, values live across cross-ABI calls (#144371, #163911)", generate: abi::generate },
    Strategy { name: "simd", lines: 40000, about: "x86-64 SSE2/SSE4/AVX2 intrinsic chains, BMI2 pdep/pext (runtime-detected)", generate: simd::generate },
    Strategy { name: "generics", lines: 40000, about: "generic fns over a numeric trait monomorphised at all 12 integer types, via fn pointers/closures", generate: generics::generate },
    Strategy { name: "borrows", lines: 30000, about: "lifetimes, reborrow chains, split borrows, closures, cursors, interior mutability", generate: borrows::generate },
    Strategy { name: "memory", lines: 25000, about: "unions, repr(packed), MaybeUninit, raw pointers, niches, provenance round-trips", generate: memory::generate },
    Strategy { name: "stdlib", lines: 20000, about: "calls to (nearly) every safe std function: integer/float/char/str methods, slices, iterators, Option/Result, cmp, mem, cells, collections (cmp::max was enough for a miscompilation)", generate: stdlib::generate },
    Strategy { name: "traits", lines: 18000, about: "dyn/generic dispatch, default-vs-override methods, const eval vs runtime, drop order", generate: traits::generate },
];

/// Not part of `all`: deliberately diverges between `debug-assertions` on/off, so the
/// detector + minimiser can be exercised end to end (`--strategies selftest --configs O0,O1-checks`).
pub const SELFTEST: Strategy = Strategy { name: "selftest", lines: 2000, about: "(hidden) injected debug-assertions divergence", generate: selftest };

fn selftest(rng: &mut Rng) -> String {
    if rng.chance(3) {
        // fails only at monomorphisation time, i.e. after the cheap type-check pass
        return "    fn bad<const N: usize>() -> usize { const { assert!(N > 5, \"bad immediate\"); } N }\n    pub fn run(h: &mut H) { h.put(&bad::<3>()); }\n".to_string();
    }
    let mut s = String::from("    pub fn run(h: &mut H) {\n");
    s += &format!("        h.put(&bb({}u32));\n", rng.below(1000));
    if rng.chance(8) {
        s += "        h.put(&cfg!(debug_assertions));\n";
    }
    s += "    }\n";
    s
}

pub fn set_miri_mode(on: bool) {
    stdlib::set_miri_mode(on);
}

pub fn stdlib_table_names() -> Vec<&'static str> {
    stdlib::table_names()
}

pub fn find(name: &str) -> Option<&'static Strategy> {
    if name == "selftest" {
        return Some(&SELFTEST);
    }
    ALL.iter().find(|s| s.name == name)
}

/// `write!` into a String without the unwrap noise.
#[macro_export]
macro_rules! w {
    ($dst:expr, $($arg:tt)*) => {{ use std::fmt::Write as _; writeln!($dst, $($arg)*).unwrap(); }};
}

pub const UINTS: &[&str] = &["u8", "u16", "u32", "u64", "u128", "usize"];
pub const SINTS: &[&str] = &["i8", "i16", "i32", "i64", "i128", "isize"];

pub fn all_ints() -> Vec<&'static str> {
    UINTS.iter().chain(SINTS.iter()).copied().collect()
}

/// A small in-range value (0..=9) of integer type `ty`.
pub fn small_ints(rng: &mut Rng, ty: &str) -> String {
    format!("{}{ty}", rng.below(10))
}

pub fn bits(ty: &str) -> u32 {
    match ty {
        "u8" | "i8" => 8,
        "u16" | "i16" => 16,
        "u32" | "i32" => 32,
        "u128" | "i128" => 128,
        _ => 64, // u64/i64/usize/isize
    }
}

pub fn signed(ty: &str) -> bool {
    ty.starts_with('i')
}

/// An in-range integer literal for `ty`, biased towards edge values optimisers care about. Every so often the value is
/// sent through a std round trip (`Some(x).unwrap_or(..)`, `to_string().parse()`, `from_le_bytes(to_le_bytes())`, ...), so
/// that all the strategies exercise a wide range of std functions on their values (see [`std_wrap`]).
pub fn lit(rng: &mut Rng, ty: &str) -> String {
    let l = lit_plain(rng, ty);
    if rng.chance(14) { std_wrap(rng, &l, ty) } else { l }
}

/// Like [`lit`] but always a plain literal (patterns, constants).
pub fn lit_plain(rng: &mut Rng, ty: &str) -> String {
    lit_base(rng, ty)
}

/// Value-preserving (or deterministically transforming) std round trips of an integer expression: Option/Result
/// combinators, cmp, mem, cells, smart pointers, byte conversions, iterators, sorting. Total for every input (no overflow,
/// no panic), so they are fine under every overflow-check setting.
pub fn std_wrap(rng: &mut Rng, x: &str, ty: &str) -> String {
    let other = lit_base(rng, ty);
    let k = rng.below(bits(ty) as usize);
    let t = ty;
    let w = match rng.below(64) {
        0 => format!("Some({x}).unwrap_or({other})"),
        1 => format!("Some({x}).map(|v| v).unwrap()"),
        2 => format!("Ok::<{t}, ()>({x}).unwrap_or_default()"),
        3 => format!("[{x}][0]"),
        4 => format!("std::cmp::max({x}, {other})"),
        5 => format!("std::cmp::min({x}, {other})"),
        6 => format!("({x}).clamp({t}::MIN, {t}::MAX)"),
        7 => format!("{t}::from_le_bytes(({x}).to_le_bytes())"),
        8 => format!("{t}::from_be_bytes(({x}).to_be_bytes())"),
        9 => format!("{t}::from_ne_bytes(({x}).to_ne_bytes())"),
        10 => format!("({x}).swap_bytes().swap_bytes()"),
        11 => format!("({x}).reverse_bits().reverse_bits()"),
        12 => format!("({x}).rotate_left({k}).rotate_right({k})"),
        13 => format!("{t}::try_from({x}).unwrap()"),
        14 => format!("({x}).to_string().parse::<{t}>().unwrap()"),
        15 => format!("std::iter::once({x}).sum::<{t}>()"),
        16 => format!("std::iter::repeat({x}).take(2).last().unwrap()"),
        17 => format!("vec![{x}; 2].into_iter().max().unwrap()"),
        18 => format!("[{x}, {other}].iter().copied().rev().last().unwrap()"),
        19 => format!("Some({x}).filter(|_| true).unwrap()"),
        20 => format!("Some({x}).and_then(|v| v.checked_add(0)).unwrap()"),
        21 => format!("Some({x}).xor(None).unwrap()"),
        22 => format!("Some({x}).zip(Some(1u8)).map(|(v, _)| v).unwrap()"),
        23 => format!("Some({x}).ok_or(()).unwrap()"),
        24 => format!("({x}).checked_add(0).unwrap()"),
        25 => format!("({x}).checked_sub(0).unwrap()"),
        26 => format!("({x}).checked_mul(1).unwrap()"),
        27 => format!("({x}).saturating_mul(1)"),
        28 => format!("({x}).wrapping_pow(1)"),
        29 => format!("({x}).pow(1)"),
        30 => format!("({x}).div_euclid(1)"),
        31 => format!("({x}).midpoint({x})"),
        32 => format!("({x}).min({x}).max({x})"),
        33 => format!("{{ let mut t = {x}; let u = std::mem::replace(&mut t, 0); u.wrapping_add(t) }}"),
        34 => format!("{{ let mut t = [{x}, {other}]; t.swap(0, 1); t.reverse(); t[0] }}"),
        35 => format!("std::hint::black_box({x})"),
        36 => format!("(std::num::Wrapping({x}) + std::num::Wrapping(0)).0"),
        37 => format!("std::num::Saturating({x}).0"),
        38 => format!("{{ let c = std::cell::Cell::new({x}); c.set(c.get()); c.replace(c.get()) }}"),
        39 => format!("*std::rc::Rc::new({x})"),
        40 => format!("*Box::new({x})"),
        41 => format!("*std::borrow::Cow::Borrowed(&{x})"),
        42 => format!("std::convert::identity({x})"),
        43 => format!("{{ let o: Option<{t}> = Some({x}); match o {{ Some(v) => v, None => {other} }} }}"),
        44 => format!("{{ let mut a = [{x}, {other}]; a.sort_unstable(); a[1] }}"),
        45 => format!("{{ let mut a = vec![{other}, {x}]; a.sort(); a.dedup(); a.pop().unwrap() }}"),
        46 => format!("({x}).wrapping_add({other}).wrapping_sub({other})"),
        47 => format!("(({x}) as i128 as {t})"),
        48 => format!("{t}::from_str_radix(&format!(\"{{:x}}\", ({x}) as u128 & {t}::MAX as u128), 16).unwrap_or({x})"),
        49 => format!("std::array::from_fn::<{t}, 2, _>(|i| if i == 0 {{ {x} }} else {{ {other} }})[0]"),
        50 => format!("[{x}; 3].map(|v| v.wrapping_add(0))[2]"),
        51 => format!("({x}..={x}).next().unwrap()"),
        52 => format!("({x}..={x}).last().unwrap()"),
        53 => format!("[{x}, {other}].into_iter().min_by_key(|v| *v != {x}).unwrap()"),
        54 => format!("[{x}].iter().position(|v| *v == {x}).map_or({other}, |_| {x})"),
        55 => format!("({x}).max({x})"),
        56 => format!("std::cmp::max_by({x}, {x}, |p, q| p.cmp(q))"),
        57 => format!("std::cmp::min_by_key({x}, {x}, |p| p.wrapping_neg())"),
        58 => format!("[{x}, {other}].iter().fold({x}, |a, b| if *b == {other} {{ a }} else {{ *b }})"),
        59 => format!("Some({x}).into_iter().chain(None).next().unwrap()"),
        60 => format!("std::mem::take(&mut {{ {x} }})"),
        61 => format!("{{ let v = vec![{x}, {other}]; v.first().copied().unwrap_or_default() }}"),
        62 => format!("*[{x}, {other}].first().unwrap()"),
        _ => format!("{{ let (a, b) = ({x}, {other}); [a, b].into_iter().find(|v| *v == a).unwrap() }}"),
    };
    // callers append method calls and casts: keep it one operand
    format!("({w})")
}

fn lit_base(rng: &mut Rng, ty: &str) -> String {
    let b = bits(ty);
    let raw: u128 = match rng.below(8) {
        0 => 0,
        1 => 1,
        2 => u128::MAX,
        3 => 1u128 << rng.below(b as usize),
        4 => (1u128 << rng.below(b as usize)).wrapping_sub(1),
        5 => rng.below(16) as u128,
        _ => (rng.next_u64() as u128) << 64 | rng.next_u64() as u128,
    };
    let masked = if b == 128 { raw } else { raw & ((1u128 << b) - 1) };
    if signed(ty) {
        // reinterpret the b-bit pattern as two's complement
        let v = if b == 128 { masked as i128 } else { ((masked << (128 - b)) as i128) >> (128 - b) };
        if v == i128::MIN {
            format!("{ty}::MIN")
        } else if v < 0 {
            format!("({v}{ty})")
        } else {
            format!("{v}{ty}")
        }
    } else {
        format!("{masked}{ty}")
    }
}

/// Numeric bounds of an integer type as i128 (u128/i128 are clamped to the i128 range, which is
/// plenty for patterns).
fn bounds(ty: &str) -> (i128, i128) {
    let b = bits(ty);
    if signed(ty) {
        if b >= 128 { (-i128::MAX, i128::MAX) } else { (-(1i128 << (b - 1)), (1i128 << (b - 1)) - 1) }
    } else if b >= 128 {
        (0, i128::MAX)
    } else {
        (0, (1i128 << b) - 1)
    }
}

fn pat_num(ty: &str, v: i128) -> String {
    if v < 0 { format!("-{}{ty}", -v) } else { format!("{v}{ty}") }
}

/// A valid pattern literal (no parentheses around negatives, unlike `lit`).
pub fn plit(rng: &mut Rng, ty: &str) -> String {
    let (lo, hi) = bounds(ty);
    let v = match rng.below(5) {
        0 => lo,
        1 => hi,
        2 => 0.clamp(lo, hi),
        3 => (rng.below(40) as i128 - 20).clamp(lo, hi),
        _ => lo + ((rng.next_u64() as i128).unsigned_abs() as i128) % (hi - lo).max(1),
    };
    pat_num(ty, v)
}

/// A valid inclusive/half-open range pattern with `lo <= hi`.
pub fn range_pat(rng: &mut Rng, ty: &str) -> String {
    let (min, max) = bounds(ty);
    let lo = {
        let spread = (max - min).max(1);
        match rng.below(4) {
            0 => min,
            1 => (rng.below(40) as i128 - 20).clamp(min, max),
            _ => min + ((rng.next_u64() as i128).unsigned_abs() as i128) % spread,
        }
    };
    let width = match rng.below(3) {
        0 => rng.below(8) as i128,
        1 => rng.below(1 << 16) as i128,
        _ => ((rng.next_u64() >> 1) as i128) << rng.below(30),
    };
    let hi = lo.saturating_add(width).min(max);
    match rng.below(5) {
        0 => format!("{}..", pat_num(ty, lo)),
        1 => format!("{}..={}", pat_num(ty, min), pat_num(ty, hi)), // `..=hi` does not parse as a macro `pat`
        _ => format!("{}..={}", pat_num(ty, lo), pat_num(ty, hi)),
    }
}
