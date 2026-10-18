#![allow(warnings)]
#![allow(unused, dead_code, unsafe_op_in_unsafe_fn)]
mod rt {
    use std::hint::black_box;
    pub struct H(pub u64);
    impl H {
        pub fn new() -> H { H(0xcbf2_9ce4_8422_2325) }
        #[inline(never)]
        pub fn feed(&mut self, v: u64) {
            self.0 = (self.0 ^ v).wrapping_mul(0x0000_0100_0000_01b3);
            self.0 ^= self.0 >> 29;
        }
        pub fn put<T: Feed + ?Sized>(&mut self, v: &T) { v.feed_into(self) }
        pub fn feed128(&mut self, v: u128) { self.feed(v as u64); self.feed((v >> 64) as u64); }
    }
    pub trait Feed { fn feed_into(&self, h: &mut H); }
    macro_rules! feed_int { ($($t:ty),*) => { $( impl Feed for $t { fn feed_into(&self, h: &mut H) { h.feed(*self as u64) } } )* } }
    feed_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, bool, char);
    impl Feed for u128 { fn feed_into(&self, h: &mut H) { h.feed128(*self) } }
    impl Feed for i128 { fn feed_into(&self, h: &mut H) { h.feed128(*self as u128) } }
    impl<T: Feed> Feed for Option<T> {
        fn feed_into(&self, h: &mut H) { match self { Some(v) => { h.feed(1); v.feed_into(h) } None => h.feed(0) } }
    }
    impl<A: Feed, B: Feed> Feed for (A, B) { fn feed_into(&self, h: &mut H) { self.0.feed_into(h); self.1.feed_into(h) } }
    impl<T: Feed> Feed for [T] { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for x in self { x.feed_into(h) } } }
    impl<T: Feed> Feed for Vec<T> { fn feed_into(&self, h: &mut H) { self[..].feed_into(h) } }
    impl<T: Feed, const N: usize> Feed for [T; N] { fn feed_into(&self, h: &mut H) { self[..].feed_into(h) } }
    impl<T: Feed + ?Sized> Feed for &T { fn feed_into(&self, h: &mut H) { (**self).feed_into(h) } }
    #[inline(always)] pub fn bb<T>(x: T) -> T { black_box(x) }
    pub fn report(id: usize, ok: bool, h: &H) {
        if ok { println!("{} {:016x}", id, h.0) } else { println!("{} PANIC", id) }
    }
}
use rt::*;
mod case_1 {
    use super::rt::*;
    
    fn f0(p0: i64, p1: u128) -> i64 {
        let l0: u64 = (31u64 / ((p0 as u64).rotate_left(8u8 as u32) | 1)).wrapping_shl((p0 as u8) as u32).saturating_add(((!0u64) % (((p1 as u64) & 15837035463095824711u64) | 1)).saturating_sub((1u64.checked_add(2251799813685247u64).unwrap_or((p0 as u64)) / ((0u64.leading_zeros() as u64) | 1)))).rotate_right(((p0 as u8) | 8u8).saturating_add((p1 as u8).wrapping_add((p1 as u8)).checked_mul((p1 as u8).wrapping_shl((p0 as u8) as u32)).unwrap_or((12u8 & (p0 as u8)))).saturating_add(((p1 as u8).wrapping_mul(253u8).checked_add(((p0 as u8) % (1u8 | 1))).unwrap_or((p1 as u8).wrapping_add(0u8)) ^ (7u8 % ((p1 as u8) | 1)).wrapping_pow(2))) as u32);
        let l1: u64 = l0;
        let l2: u64 = ((l1 / (l1 | 1)).rotate_right(0u8.wrapping_mul((l1 as u8)) as u32).checked_mul(l1.saturating_sub((l1 as u64)).min((l1 as u64).overflowing_mul((l1 as u64)).0)).unwrap_or(l1) / ((70368744177663u64.wrapping_shl((p1 as u8) as u32).count_ones() as u64).rotate_left((l0 as u8).wrapping_mul(134u8).saturating_add(0u8) as u32) | 1));
        let l3: u32 = (7652625836837110172isize.checked_mul((p0 as isize)).unwrap_or((l2 as isize)).wrapping_neg() as u32).wrapping_sub((0u32.count_ones() as u32).checked_add((1800089008u32.rotate_right((l0 as u8) as u32) | 1u32.wrapping_shl((l1 as u8) as u32))).unwrap_or((((p1 as u32) | 1).ilog2() as u32).wrapping_shr(209u8.overflowing_mul(2u8).0 as u32)));
        (1i64.overflowing_sub(p0).1 as i64).wrapping_mul(549755813888i64.rotate_left((p0 as u8) as u32)).rotate_right((l0 as u8).saturating_sub((l1 as u8)).saturating_add((255u8 & (l3 as u8))) as u32)
    }
    #[inline]
    fn f1(p0: i16, p1: i128, p2: u64, p3: u8) -> u128 {
        let l0: i8 = ((if (1i32.wrapping_add(1073741824i32).wrapping_mul((0i32 ^ (p1 as i32))) <= 0i32.checked_mul((if (4096u16 == 4u16) { 4194303i32 } else { (p2 as i32) })).unwrap_or((p1 as i32).saturating_sub((p1 as i32)))) { ((p3 as i8).wrapping_shl(p3 as u32) | (p3 as i8).rotate_left(p3 as u32).wrapping_shl(255u8.saturating_sub(p3) as u32)) } else { (p2 as i8).rotate_left(p3 as u32).wrapping_mul(0i8.wrapping_mul(0i8)).checked_add((p3 as i8).overflowing_mul(14i8).0.rotate_left(p3.wrapping_neg() as u32)).unwrap_or((p3 as i8).wrapping_shl(2u8 as u32).wrapping_div((p1 as i8) | 1)) }).trailing_zeros() as i8);
        let l1: u64 = ((((p1 as u64) | 1).ilog2() as u64).wrapping_mul(p2.rotate_right(p3 as u32)) ^ ((p2 & 18446744073709551615u64) | (p2 as u64).checked_mul(p2).unwrap_or((p0 as u64))));
        (if (l0 != (l0 as i8)) { (l0 as u128) } else { 78641062838714968657195621887254049057u128 }).overflowing_mul((l1 as u128).rotate_left(p3 as u32)).0.checked_mul(((p1 as u128) % ((l0 as u128) | 1)).overflowing_mul((1u128 & (l1 as u128))).0).unwrap_or((1u128 ^ 5u128).wrapping_shl(p3.checked_add(p3).unwrap_or(p3) as u32)).saturating_add(235133259142214299190939716127169969945u128.rotate_right((p3 | 1u8).checked_mul((p2 as u8)).unwrap_or(p3.wrapping_shr(p3 as u32)) as u32))
    }
    
    fn f2(p0: i8, p1: u8, p2: i128, p3: i16) -> i128 {
        let l0: u32 = ((p3 as u32) ^ (67108863u32 % (4096u32 | 1)));
        let l1: u8 = (p1 ^ p1).wrapping_shl((if (10081444169454656741u64 < 2924514928515046457u64) { p1 } else { p1 }) as u32).wrapping_add((p2 as u8).checked_add(p1).unwrap_or(p1).checked_mul((13u8.overflowing_sub(p1).1 as u8)).unwrap_or(134u8.checked_mul(p1).unwrap_or((l0 as u8))));
        let l2: i32 = (p2 as i32).rotate_left(p1 as u32).checked_add((l0 as i32).checked_mul((p1 as i32)).unwrap_or((p1 as i32))).unwrap_or(7i32.checked_mul((p3 as i32)).unwrap_or((-1i32)));
        p2.saturating_sub((l0 as i128)).saturating_add(p2.wrapping_pow(2))
    }
    #[inline]
    fn f3(p0: i64, p1: i128, p2: i64, p3: i32) -> i128 {
        let l0: u16 = ((p2 as u16) / (16u16 | 1)).wrapping_shl((1u8 & (p3 as u8)) as u32).wrapping_pow(1);
        let l1: i8 = (1i8 ^ (2u32.checked_mul(0u32).unwrap_or(0u32).rotate_right((l0 as u8).checked_add((p0 as u8)).unwrap_or((p0 as u8)) as u32) as i8)).saturating_sub(((if !(p1 == p1) { (268435456isize.count_ones() as isize) } else { (p1 as isize) }).wrapping_add(0isize) as i8));
        (if !(p3 > p3) { p1.saturating_sub(576460752303423488i128) } else { p1.saturating_add(p1) })
    }
    #[inline]
    fn f4(p0: u8) -> i128 {
        let l0: u128 = (!340282366920938463463374607431768211455u128).wrapping_add(((p0 as u128) | (p0 as u128)));
        let l1: u128 = l0.wrapping_add(l0).wrapping_add((if (256i32 != 0i32) { l0 } else { l0 }));
        (1i128 | 281474976710656i128).wrapping_add((p0 as i128).checked_add((p0 as i128)).unwrap_or(136343510479004399201671423594082351077i128)).wrapping_rem((-1i128).swap_bytes().wrapping_mul((p0 as i128).wrapping_div((l1 as i128) | 1)) | 1).checked_mul((((l1 as u32) as i128).wrapping_div((p0 as i128).checked_mul((l1 as i128)).unwrap_or(0i128) | 1) | ((l1 as i128).overflowing_sub(618970019642690137449562111i128).1 as i128).overflowing_mul(1i128.rotate_left(p0 as u32)).0)).unwrap_or(((-74880275560081419883782580558174382742i128).rotate_right(1u8 as u32) & (l0 as i128).wrapping_add((l0 as i128))).checked_mul(((31634427057743500706584199471031362894i128.leading_zeros() as i128) ^ (l0 as i128).rotate_right((l0 as u8) as u32))).unwrap_or((p0 as i128).checked_add((l0 as i128)).unwrap_or((1i64 as i128)))).wrapping_mul(((if (262144i128 == (p0 as i128)) { ((-25022775611438000314977211741566298488i128) ^ 0i128) } else { (26263i16 as i128) }).overflowing_sub((l0 as i128)).1 as i128).wrapping_add((293116337u32.rotate_left((p0 as u8) as u32) as i128).wrapping_add((l0 as i128).wrapping_add((p0 as i128)).overflowing_mul(163976414031116195177608646044265653703i128.overflowing_mul(0i128).0).0)))
    }
    pub fn run(h: &mut H) {
        let v0: i64 = bb(0i64);
        let v1: i32 = bb(32767i32);
        let v2: u128 = bb(0u128);
        let t0: u16 = (v1 as u16);
        h.put(&t0);
        let t1: i8 = (if (((-1i8) != 64i8) || (((0usize < (v2 as usize)) || (!(v1 > (v0 as i32)))))) { (v0 as i8) } else { (v0 as i8) }).rotate_right((v2 as u8).wrapping_sub(1u8) as u32).wrapping_rem(16i8 | 1).wrapping_add(((v2 as i8).overflowing_sub((t0 as i8)).1 as i8).wrapping_div((t0 as i8).checked_add((v0 as i8)).unwrap_or((v0 as i8)) | 1).checked_mul(109i8.checked_mul(1i8).unwrap_or(5i8).overflowing_mul((t0 as i8)).0).unwrap_or((v1 as i8).wrapping_mul((-1i8).checked_mul((v0 as i8)).unwrap_or((v2 as i8)))));
        h.put(&t1);
        let t2: i128 = (v2 as i128).wrapping_shr(16u8.overflowing_mul((t1 as u8)).0.overflowing_mul((220u8 / (0u8 | 1))).0.wrapping_mul((v1 as u8).wrapping_shr((v1 as u8) as u32).wrapping_pow(2)) as u32).wrapping_shl((153u8 ^ (t0 as u8)).rotate_left(0u8 as u32).wrapping_sub(((104881666913362824920909438550674652468i128 as u8).overflowing_sub(9u8).1 as u8)).wrapping_mul((if ((t1.checked_mul(t1).unwrap_or(t1) >= t1.wrapping_rem(t1 | 1)) || (((v0 as u64) == 16777215u64))) { 31u8.wrapping_shr(0u8 as u32).checked_mul(((v1 as u8) ^ (t0 as u8))).unwrap_or((v0 as u8).wrapping_add((v1 as u8))) } else { (v2 as u8) })) as u32);
        h.put(&t2);
        let t3: i8 = (t1.checked_add(t1).unwrap_or(4i8).count_ones() as i8).max(t1.wrapping_pow(2).wrapping_rem(t1.wrapping_rem(t1 | 1) | 1));
        h.put(&t3);
        let r = f0(bb((-1i64)), bb(242157899311304759393794909474574479623u128));
        h.put(&r);
        let r = f0(bb(2i64), bb(340282366920938463463374607431768211455u128));
        h.put(&r);
        let r = f0(bb(2305843009213693952i64), bb(340282366920938463463374607431768211455u128));
        h.put(&r);
        let r = f1(bb((-1i16)), bb((-22686851763169499401772691129403250795i128)), bb(7287172909268571250u64), bb(255u8));
        h.put(&r);
        let r = f1(bb((-25381i16)), bb(664613997892457936451903530140172288i128), bb(1u64), bb(0u8));
        h.put(&r);
        let r = f2(bb(127i8), bb(255u8), bb((-1i128)), bb(21471i16));
        h.put(&r);
        let r = f3(bb(4503599627370495i64), bb(2048i128), bb((-1i64)), bb(512i32));
        h.put(&r);
        let r = f3(bb(17592186044415i64), bb(1i128), bb(1i64), bb((-1361215617i32)));
        h.put(&r);
        let r = f3(bb(0i64), bb(6i128), bb((-633477288926405031i64)), bb(1i32));
        h.put(&r);
        let r = f4(bb(0u8));
        h.put(&r);
        let mut acc0: u8 = bb(41u8);
        for i in 0..bb(17u32) { acc0 = (0u8 | acc0).checked_add(1u8.saturating_add(acc0)).unwrap_or(acc0.checked_add(9u8).unwrap_or(acc0)); }
        h.put(&acc0);
        let mut acc1: u8 = bb(197u8);
        for i in 0..bb(21u32) { acc1 = (acc1.wrapping_mul((v1 as u8)).overflowing_sub(((i as u8) / (acc1 | 1))).1 as u8).rotate_left(acc1.saturating_add(acc1).rotate_right(acc1 as u32) as u32); }
        h.put(&acc1);
    }

}
macro_rules! go { ($id:literal, $m:ident) => {{
    let mut h = H::new();
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $m::run(&mut h))).is_ok();
    report($id, ok, &h);
}}; }
fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    go!(1, case_1);
}
