// rs-compiler-fuzzer: `-C opt-level=z` differs from `-C opt-level=0/s/1/2/3`. Reduced automatically (73 -> 17 lines). Miri accepts it (UB-free).
//   rustc --edition 2021 -C debug-assertions=off -C overflow-checks=off -C opt-level=0 case.rs && ./case   -> -1
//   rustc --edition 2021 -C debug-assertions=off -C overflow-checks=off -C opt-level=z case.rs && ./case   -> 0
use std::hint::black_box as bb;

fn main() {
    let v1: i16 = bb(1i16);
    let v2: i128 = bb(0i128);

    let v4: i16 = (0i16);
    let t0: i16 = v1;
    let t1: u128 = (t0 as u128).saturating_sub((1u128 / ((v4.overflowing_neg().0 as u128) | 1)));
    let t2: u16 = (((v1 as u16) % ((t1 as u16) | 1)) | 1);
    let t3: i128 = v2.wrapping_sub(v2.checked_add(v2).unwrap_or(v2));
    let t4: i32 = ((v1 as i32).overflowing_mul(1i32).0 | (t0 as i32).checked_neg().unwrap_or(0i32))
        .checked_pow(1)
        .unwrap_or((t2 as i32))
        .wrapping_div((t2 as i32).min(((v1).count_zeros() as i32).checked_pow(4).unwrap_or((t3 as i32))) | 1);
    println!("{:?}", t4);
}
