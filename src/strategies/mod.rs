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
mod loops;
mod memory;
mod recursion;
mod select;
mod simd;
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
    Strategy { name: "select", lines: 30000, about: "structs built by if/else over clamped ints and floats (InstCombine select->min/max, #163728)", generate: select::generate },
    Strategy { name: "recursion", lines: 30000, about: "recursion combining results with max/min after the call, accumulator and mutual recursion (TailCallElim, #164469)", generate: recursion::generate },
    Strategy { name: "floats", lines: 20000, about: "IEEE arithmetic, saturating float<->int casts, fma, rounding, formatting/parsing", generate: floats::generate },
    Strategy { name: "unwind", lines: 20000, about: "panics, landing pads, drop guards, partially built arrays/vecs, catch_unwind", generate: unwind::generate },
    Strategy { name: "abi", lines: 20000, about: "extern C/sysv64/win64 calls: many arg/ret kinds, fn pointers, values live across cross-ABI calls (#144371, #163911)", generate: abi::generate },
    Strategy { name: "simd", lines: 15000, about: "x86-64 SSE2/SSE4/AVX2 intrinsic chains, BMI2 pdep/pext (runtime-detected)", generate: simd::generate },
    Strategy { name: "borrows", lines: 30000, about: "lifetimes, reborrow chains, split borrows, closures, cursors, interior mutability", generate: borrows::generate },
    Strategy { name: "memory", lines: 25000, about: "unions, repr(packed), MaybeUninit, raw pointers, niches, provenance round-trips", generate: memory::generate },
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

/// An in-range integer literal for `ty`, biased towards edge values optimisers care about.
pub fn lit(rng: &mut Rng, ty: &str) -> String {
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
