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

mod arith;
mod borrows;
mod control;
mod loops;
mod memory;
mod traits;

pub type GenFn = fn(&mut Rng) -> String;

pub struct Strategy {
    pub name: &'static str,
    pub about: &'static str,
    pub generate: GenFn,
}

pub const ALL: &[Strategy] = &[
    Strategy { name: "arith", about: "typed integer expression trees, inlining variants, loops folding them", generate: arith::generate },
    Strategy { name: "loops", about: "iterator chains, slicing loops, nested counters (cf. zxt5's rustc reports)", generate: loops::generate },
    Strategy { name: "control", about: "random structured control flow: labeled blocks, break/continue, bool state machines", generate: control::generate },
    Strategy { name: "borrows", about: "lifetimes, reborrow chains, split borrows, closures, cursors, interior mutability", generate: borrows::generate },
    Strategy { name: "memory", about: "unions, repr(packed), MaybeUninit, raw pointers, niches, provenance round-trips", generate: memory::generate },
    Strategy { name: "traits", about: "dyn/generic dispatch, default-vs-override methods, const eval vs runtime, drop order", generate: traits::generate },
];

/// Not part of `all`: deliberately diverges between `debug-assertions` on/off, so the
/// detector + minimiser can be exercised end to end (`--strategies selftest --configs O0,O1-checks`).
pub const SELFTEST: Strategy = Strategy { name: "selftest", about: "(hidden) injected debug-assertions divergence", generate: selftest };

fn selftest(rng: &mut Rng) -> String {
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
