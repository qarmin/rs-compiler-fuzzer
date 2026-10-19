// Found by the `arith` strategy (rs-compiler-fuzzer), reduced automatically from 120 to 60 lines (not hand-reduced).
//
//   rustc --edition 2021 -C debug-assertions=off -C overflow-checks=off -C opt-level=0 file.rs && ./file   (also s, 1, 2, 3)
//   rustc --edition 2021 -C debug-assertions=off -C overflow-checks=off -C opt-level=z file.rs && ./file
//
// prints different hashes (the output is `<case id> <hash of t4>`; `t4` is an i32 computed by the chain in `run`).
// Reproduces on rustc 1.99.1 stable and 1.101.0-nightly (8006091eb 2026-10-17), x86_64-unknown-linux-gnu,
// only at -C opt-level=z. Adding eprintln!/changing unused functions hides it, so it looks like an
// inlining-threshold / code-layout dependent miscompile.
//
// Verified UB-free: `cargo +nightly miri run` accepts the program (overflow checks on) and prints
// `149 af63bc4c8601b62c`, i.e. the -O0 result; -C opt-level=z prints `149 af63bd4c8601b7df`.
mod rt {
    use std::hint::black_box;
    pub struct H(pub u64);
    impl H {
        pub fn new() -> H { H(0xcbf2_9ce4_8422_2325) }
        pub fn feed(&mut self, v: u64) {
            self.0 = (self.0 ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        }
        pub fn put<T: Feed + ?Sized>(&mut self, v: &T) { v.feed_into(self) }
    }
    pub trait Feed { fn feed_into(&self, h: &mut H); }
    macro_rules! feed_int { ($($t:ty),*) => { $( impl Feed for $t { fn feed_into(&self, h: &mut H) { h.feed(*self as u64) } } )* } }
    feed_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, bool, char);
    impl<T: Feed> Feed for Option<T> {
        fn feed_into(&self, h: &mut H) { match self { Some(v) => { h.feed(1); v.feed_into(h) } None => h.feed(0) } }
    }
    #[inline(always)] pub fn bb<T>(x: T) -> T { black_box(x) }
    pub fn report(id: usize, ok: bool, h: &H) {
        if ok { println!("{} {:016x}", id, h.0) } else { println!("{} PANIC", id) }
    }
}
use rt::*;
mod case_149 {
    use super::rt::*;
    fn f2(p0: i64, p1: i8, p2: u128, p3: u128) -> i16 {
        let l1: i64 = 0;
        let l2: i64 = 0;
        (p2 as i16).wrapping_sub(((p0 as i16).abs_diff((1i16.saturating_add((-8105i16)).overflowing_sub((l2 as i16).rotate_right((p1 as u8) as u32)).1 as i16)) as i16).wrapping_sub(((l1 as i16).checked_add((p3 as i16)).unwrap_or((l1 as i16)).abs_diff(15i16.wrapping_abs()) as i16).wrapping_div_euclid((16062i16.wrapping_rem(1i16 | 1).leading_ones() as i16) | 1)))
    }
    fn f3(p0: i128, p1: u64) -> i8 {
        let l0: u64 = 0;
        ((l0 as i8).wrapping_pow(2).wrapping_pow(1) & (1i8.abs_diff((p0 as i8).wrapping_sub(4i8).overflowing_mul(7i8.rotate_left((p0 as u8) as u32)).0.saturating_mul(16i8.wrapping_add((-38i8)).saturating_sub(((p0 as i8).unsigned_abs() as i8)))) as i8))
    }
    fn f4(p0: usize, p1: i64) -> isize {
        let l0: isize = 0;
        let l1: i8 = 58i8.checked_neg().unwrap_or((-1i8).checked_neg().unwrap_or((p1 as i8)).checked_shl(255u8.saturating_pow(1) as u32).unwrap_or(((p0 as i8).trailing_ones() as i8))).checked_shl((p1 as u8).max((12u8 % ((p0 as u8) | 1))).wrapping_neg() as u32).unwrap_or(0i8.wrapping_rem_euclid(1i8 | 1).rotate_left(0u8.wrapping_add((p1 as u8)) as u32).saturating_add((l0 as i8).wrapping_neg().overflowing_mul(1i8.wrapping_rem((p0 as i8) | 1)).0)).wrapping_shl((64u8 / ((l0 as u8) | 1)).saturating_add(127u8.wrapping_shl((p1 as u8) as u32)).checked_neg().unwrap_or((l0 as u8).rotate_right(14u8 as u32).saturating_mul(29u8.saturating_add(16u8))).wrapping_shr((16u8.trailing_zeros() as u8).wrapping_shl(27u8.checked_shl(0u8 as u32).unwrap_or(33u8).wrapping_add(0u8.wrapping_mul((l0 as u8))) as u32) as u32) as u32);
        (!(p0 as isize).wrapping_shr((l0 as u8) as u32).wrapping_rem_euclid((p0 as isize).wrapping_pow(1) | 1).saturating_pow(1).saturating_add(((l0.wrapping_sub(l0) & (l0 ^ l0)) & (0u128.wrapping_shl(205u8 as u32) as isize))))
    }
    pub fn run(h: &mut H) {
        let v0: isize = bb(1isize);
        let v1: i16 = bb(1i16);
        let v2: i128 = bb(8388608i128);
        let v3: u32 = 0;
        let v4: i16 = bb((-24957i16));
        let t0: i16 = v1;
        let t1: u128 = (t0 as u128).saturating_sub(144115188075855871u128.checked_neg().unwrap_or((v4 as u128)).rotate_left((v0 as u8).rotate_right(15u8 as u32) as u32).rotate_left((if (matches!(1i8, -14i8..=127i8)) { (v0 as u8) } else { (v4 as u8) }).saturating_pow(3) as u32).checked_add(((v3 as u128) & ((v1 as u64) as u128).overflowing_mul(0u128.reverse_bits()).0)).unwrap_or(((31u128.is_power_of_two() as u128) / (((v4 as usize).overflowing_neg().0 as u128) | 1))).saturating_pow(1));
        let t2: u16 = (if ((v0.wrapping_div(v0 | 1) > v0.checked_add(v0).unwrap_or((v0 as isize))) || (((v2 as i8) != (v2 as i8)))) { 61580u16.wrapping_shl(107u8 as u32).checked_mul((v3 as u16).saturating_add(52043u16)).unwrap_or(((v0 as u16) % (1u16 | 1))) } else { 4096u16.checked_neg().unwrap_or(64514u16).saturating_sub(((v4 as u16) ^ 1u16)) }).checked_neg().unwrap_or(1u16.wrapping_rem_euclid(((v1 as u16) % ((t1 as u16) | 1)).rotate_left(255u8.saturating_sub(128u8) as u32) | 1));
        let t3: i128 = v2.min(v2).wrapping_sub(v2.checked_add(v2).unwrap_or(v2)).wrapping_div(((-22627222925499015795147261041155489629i128).wrapping_pow(1).overflowing_sub(9444732965739290427392i128.min((-1i128))).1 as i128) | 1);
        let t4: i32 = ((v1 as i32).overflowing_mul(1i32).0 | (t0 as i32).checked_neg().unwrap_or(268435455i32)).checked_pow(1).unwrap_or((t2 as i32).wrapping_rem_euclid((-1485008291i32).wrapping_shr(1u8 as u32) | 1)).wrapping_mul(((t0 as i32) & ((v3 as i32).trailing_zeros() as i32).saturating_pow(0))).wrapping_div((t2 as i32).wrapping_add((((v0 as i32) | 7i32) & (t2 as i32).wrapping_shr((v4 as u8) as u32))).min((((v2 as i32).abs_diff((v1 as i32)) as i32).count_zeros() as i32).checked_pow(4).unwrap_or(((t3 as i32).trailing_zeros() as i32))) | 1).saturating_pow(2);
        h.put(&t4);
        let r = f4(bb(140737488355328usize), bb(1i64));
    }
}
macro_rules! go { ($id:literal, $m:ident) => {{
    let mut h = H::new();
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $m::run(&mut h))).is_ok();
    report($id, ok, &h);
}}; }
fn main() {
    go!(149, case_149);
}