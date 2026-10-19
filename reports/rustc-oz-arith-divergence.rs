// Found by the `arith` strategy (rs-compiler-fuzzer), reduced automatically (73 -> 43 lines: own passes + treereduce-rust). Miri-verified UB-free.
//
//   rustc --edition 2021 -C debug-assertions=off -C overflow-checks=off -C opt-level=0 file.rs && ./file   -> 149 cbf29ce484222325 (also s, 1, 2, 3; same as Miri)
//   rustc --edition 2021 -C debug-assertions=off -C overflow-checks=off -C opt-level=z file.rs && ./file   -> 149 340d631b7bdddcda
//
mod rt {
    use std::hint::black_box;
    pub struct H( u64);
    impl H {
        pub fn new() -> H { H(0xcbf2_9ce4_8422_2325) }
         fn feed(&mut self, v: u64) {
            self.0 = (self.0 ^ v);
        }
        pub fn put<T: Feed + >(&mut self, v: &T) { v.feed_into(self) }
    }
     trait Feed { fn feed_into(&self, h: &mut H); }
    macro_rules! feed_int { ($($t:ty),*) => { $( impl Feed for $t { fn feed_into(&self, h: &mut H) { h.feed(*self as u64) } } )* } }
    feed_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, bool, char);
     pub fn bb<T>(x: T) -> T { black_box(x) }
    pub fn report(id: usize, ok: bool, h: &H) {
        if ok { println!("{} {:016x}", id, h.0) }  
    }
}
use rt::*;
mod case_149 {
    use super::rt::*;
    pub fn run(h: &mut H) {
        let v0: isize = bb(1isize);
        let v1: i16 = bb(1i16);
        let v2: i128 = bb(8388608i128);
        let v3: u32 = 0;
        let v4: i16 = bb((-24957i16));
        let t0: i16 = v1;
        let t1: u128 = (t0 as u128).saturating_sub(144115188075855871u128.checked_neg().unwrap_or((v4 as u128)).checked_add(((v3 as u128) & ((v1 as u64) as u128).overflowing_mul(0u128).0)).unwrap_or(((31u128 as u128) / (((v4 as usize).overflowing_neg().0 as u128) | 1))));
        let t2: u16 = (if ((v0 > v0.checked_add(v0).unwrap_or((v0 as isize))) || (((v2 as i8) != (v2 as i8)))) { 61580u16.checked_mul((v3 as u16)).unwrap_or(((v0 as u16) % (1u16 | 1))) } else { 4096u16.checked_neg().unwrap_or(64514u16) }).checked_neg().unwrap_or(1u16.wrapping_rem_euclid(((v1 as u16) % ((t1 as u16) | 1)) | 1));
        let t3: i128 = v2.wrapping_sub(v2.checked_add(v2).unwrap_or(v2));
        let t4: i32 = ((v1 as i32).overflowing_mul(1i32).0 | (t0 as i32).checked_neg().unwrap_or(268435455i32)).checked_pow(1).unwrap_or((t2 as i32)).wrapping_div((t2 as i32).min((((v2 as i32).abs_diff((v1 as i32)) as i32).count_zeros() as i32).checked_pow(4).unwrap_or(((t3 as i32) as i32))) | 1);
        h.put(&t4);
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