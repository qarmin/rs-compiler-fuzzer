// Found by the `floats` strategy (rs-compiler-fuzzer), minimised by hand.
//
//   rustc -C opt-level=0 file.rs && ./file   ->  0x7ff8000000000000   (also -C opt-level=1, s)
//   rustc -C opt-level=2 file.rs && ./file   ->  0xfff8000000000000   (also -C opt-level=3)
//
// rustc 1.97.0 stable and 1.101.0-nightly (8006091eb 2026-10-17, LLVM 23.1.4), x86_64-unknown-linux-gnu.
//
// `c64` maps every NaN to one fixed bit pattern. After inlining, LLVM turns
//   select(is_nan(sqrt(x)), +qnan, sqrt(x))   into   sqrt(x)
// (all NaNs are treated as interchangeable at the floating-point level), and the hardware NaN
// (sign bit set for sqrtsd of a negative number) then leaks through `to_bits`.
// Arguably the payload/sign of a NaN is unspecified, but here the program explicitly returns a
// constant for NaN inputs, so the optimised result differs from the written semantics.
// Needs the call to be inlined: with #[inline(never)] on `c64` all levels agree.
fn c64(x: f64) -> u64 {
    if x.is_nan() { 0x7ff8_0000_0000_0000 } else { x.to_bits() }
}

fn main() {
    let v = std::hint::black_box(-1.0f64);
    let n = v.sqrt();
    println!("{:#x}", c64(n));
}
