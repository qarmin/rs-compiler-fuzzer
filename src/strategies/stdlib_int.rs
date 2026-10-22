//! `stdlib` family: integer methods (all widths), `NonZero`, `Wrapping`/`Saturating`, `u8` ASCII methods.
//! Prelude: `T` (type), `U`/`I` (same-width unsigned/signed), `a b c: T`, `ua: U`, `ib: I`, `n < BITS`, `e <= 6`,
//! `d: &str`, `r: u32` (radix). Divisors are `| 1`; shifts/exponents are small.

use super::stdlib::{bbv, counterpart, emit, int_lit, is_unsigned, small, wrap};
use super::{all_ints, bits, lit};
use crate::rng::Rng;

const ANY: &[&str] = &[
    "h.put(&a.count_ones()); h.put(&a.count_zeros()); h.put(&a.leading_zeros()); h.put(&a.trailing_zeros()); h.put(&a.leading_ones()); h.put(&a.trailing_ones());",
    "h.put(&a.rotate_left(n)); h.put(&a.rotate_right(n)); h.put(&b.rotate_left(n + e)); h.put(&c.rotate_right(n ^ e));",
    "h.put(&a.swap_bytes()); h.put(&a.reverse_bits()); h.put(&b.swap_bytes().reverse_bits());",
    "h.put(&a.to_be()); h.put(&a.to_le()); h.put(&T::from_be(b)); h.put(&T::from_le(c));",
    "h.put(&a.to_be_bytes()); h.put(&b.to_le_bytes()); h.put(&c.to_ne_bytes());",
    "h.put(&T::from_be_bytes(a.to_le_bytes())); h.put(&T::from_le_bytes(b.to_be_bytes())); h.put(&T::from_ne_bytes(c.to_ne_bytes()));",
    "h.put(&a.wrapping_add(b)); h.put(&a.wrapping_sub(b)); h.put(&a.wrapping_mul(b)); h.put(&a.wrapping_neg());",
    "h.put(&a.wrapping_shl(n)); h.put(&a.wrapping_shr(n)); h.put(&b.wrapping_shl(n.wrapping_add(e * 31))); h.put(&c.wrapping_shr(n.wrapping_mul(7)));",
    "h.put(&a.wrapping_pow(e)); h.put(&b.wrapping_pow(e + 3)); h.put(&a.pow(e.min(1))); h.put(&T::wrapping_pow(c, e));",
    "h.put(&a.wrapping_div(b | 1)); h.put(&a.wrapping_rem(b | 1)); h.put(&a.wrapping_div_euclid(b | 1)); h.put(&a.wrapping_rem_euclid(b | 1));",
    "h.put(&a.checked_add(b)); h.put(&a.checked_sub(b)); h.put(&a.checked_mul(b)); h.put(&a.checked_neg());",
    "h.put(&a.checked_div(b)); h.put(&a.checked_rem(b)); h.put(&a.checked_div_euclid(b)); h.put(&a.checked_rem_euclid(b));",
    "h.put(&a.checked_shl(n + e * 20)); h.put(&a.checked_shr(n + e * 20)); h.put(&a.checked_pow(e + 2)); h.put(&b.checked_pow(e));",
    "h.put(&a.checked_ilog2()); h.put(&a.checked_ilog10()); h.put(&a.checked_ilog(b)); h.put(&b.checked_ilog(c));",
    "if a > 0 { h.put(&a.ilog2()); h.put(&a.ilog10()); h.put(&a.ilog(b.max(2))); }",
    "h.put(&a.overflowing_add(b)); h.put(&a.overflowing_sub(b)); h.put(&a.overflowing_mul(b)); h.put(&a.overflowing_neg());",
    "h.put(&a.overflowing_shl(n + e * 20)); h.put(&a.overflowing_shr(n + e * 20)); h.put(&a.overflowing_pow(e + 2));",
    "h.put(&a.overflowing_div(b | 1)); h.put(&a.overflowing_rem(b | 1)); h.put(&a.overflowing_div_euclid(b | 1)); h.put(&a.overflowing_rem_euclid(b | 1));",
    "h.put(&a.saturating_add(b)); h.put(&a.saturating_sub(b)); h.put(&a.saturating_mul(b)); h.put(&a.saturating_pow(e + 2)); h.put(&a.saturating_div(b | 1));",
    "h.put(&a.min(b)); h.put(&a.max(b)); h.put(&a.clamp(b.min(c), b.max(c)));",
    "h.put(&std::cmp::max(a, b)); h.put(&std::cmp::min(a, b)); h.put(&std::cmp::max(std::cmp::min(a, b), c));",
    "h.put(&std::cmp::max_by(a, b, |x, y| y.cmp(x))); h.put(&std::cmp::min_by(a, b, |x, y| x.cmp(y))); h.put(&std::cmp::max_by_key(a, b, |x| x.wrapping_neg())); h.put(&std::cmp::min_by_key(a, b, |x| x.rotate_left(1)));",
    "h.put(&Ord::max(a, b)); h.put(&Ord::min(a, b)); h.put(&Ord::clamp(a, b.min(c), b.max(c))); h.put(&PartialOrd::partial_cmp(&a, &b));",
    "h.put(&a.cmp(&b)); h.put(&a.partial_cmp(&c)); h.put(&b.cmp(&b)); h.put(&a.cmp(&b).reverse().then(b.cmp(&c)));",
    "h.put(&a.eq(&b)); h.put(&a.ne(&b)); h.put(&a.lt(&b)); h.put(&a.le(&b)); h.put(&a.gt(&c)); h.put(&a.ge(&c));",
    "h.put(&(a < b)); h.put(&(a == b)); h.put(&(a >= c)); h.put(&(a != c));",
    "h.put(&a.abs_diff(b)); h.put(&b.abs_diff(a)); h.put(&a.abs_diff(a));",
    "h.put(&a.midpoint(b)); h.put(&b.midpoint(c)); h.put(&a.midpoint(a));",
    "if a >= 0 { h.put(&a.isqrt()); }",
    "h.put(&(!a)); h.put(&(a & b)); h.put(&(a | b)); h.put(&(a ^ c)); h.put(&(a << n)); h.put(&(b >> n));",
    "h.put(&a.unbounded_shl(n + e * 40)); h.put(&b.unbounded_shr(n + e * 40));",
    "if !(a == T::MIN && (b | 1) == !0) { h.put(&(a / (b | 1))); h.put(&(a % (b | 1))); h.put(&a.div_euclid(b | 1)); h.put(&a.rem_euclid(b | 1)); }",
    "let mut x = a; x = x.wrapping_add(b); x ^= c; x <<= n / 2; x >>= n / 3; x |= b; x &= !c; h.put(&x);",
    "let mut x = a; x += 0; x = x.wrapping_mul(b); x -= 0; x ^= a; x = x.rotate_left(e); h.put(&x);",
    "{ use std::ops::*; h.put(&Add::add(a, 0)); h.put(&Sub::sub(a, 0)); h.put(&Mul::mul(a, 1)); h.put(&BitAnd::bitand(a, b)); h.put(&BitOr::bitor(a, b)); h.put(&BitXor::bitxor(a, b)); h.put(&Not::not(a)); h.put(&Shl::shl(a, n)); h.put(&Shr::shr(b, n)); h.put(&Div::div(a, 1)); h.put(&Rem::rem(a, 1)); }",
    "{ use std::ops::*; let mut x = a; x.bitxor_assign(b); x.bitor_assign(c); x.bitand_assign(!b); x.shl_assign(n); x.shr_assign(n / 2); x.add_assign(0); x.sub_assign(0); x.mul_assign(1); x.div_assign(1); h.put(&x); }",
    "h.put(&T::MIN); h.put(&T::MAX); h.put(&T::BITS); h.put(&T::default()); h.put(&a.clone()); h.put(&b.to_owned());",
    "h.put(&a.to_string()); h.put(&format!(\"{:x}\", a)); h.put(&format!(\"{:X}\", b)); h.put(&format!(\"{:o}\", c)); h.put(&format!(\"{:b}\", a));",
    "h.put(&format!(\"{:#x}\", a)); h.put(&format!(\"{:#010b}\", b)); h.put(&format!(\"{:+}\", c)); h.put(&format!(\"{:>12}|{:<12}|{:^12}\", a, b, c)); h.put(&format!(\"{:012}\", a));",
    "h.put(&format!(\"{:e}\", a)); h.put(&format!(\"{:E}\", b)); h.put(&format!(\"{:?}\", c)); h.put(&format!(\"{:#?}\", a)); h.put(&format!(\"{:*^9}\", b));",
    "h.put(&T::from_str_radix(d, r).ok()); h.put(&d.parse::<T>().ok()); h.put(&\"+7\".parse::<T>().ok()); h.put(&\"-0\".parse::<T>().ok());",
    "h.put(&T::from_str_radix(d, r).map_err(|e| e.to_string())); h.put(&\"\".parse::<T>().map_err(|e| e.to_string())); h.put(&\"99999999999999999999999999999999999999999\".parse::<T>().map_err(|e| e.to_string()));",
    "h.put(&u8::try_from(a).ok()); h.put(&i16::try_from(a).ok()); h.put(&u32::try_from(b).ok()); h.put(&i64::try_from(b).ok()); h.put(&usize::try_from(c).ok()); h.put(&u128::try_from(c).ok()); h.put(&i128::try_from(a).ok());",
    "let r1: Result<u16, _> = a.try_into(); let r2: Result<i8, _> = b.try_into(); let r3: Result<u64, _> = c.try_into(); h.put(&r1.ok()); h.put(&r2.ok()); h.put(&r3.ok());",
    "h.put(&T::try_from(300u16).ok()); h.put(&T::try_from(-1i64).ok()); h.put(&T::try_from(u128::MAX).ok()); h.put(&T::try_from(127i8).ok()); h.put(&T::try_from(usize::MAX).ok());",
    "h.put(&T::from(true)); h.put(&T::from(a > b)); h.put(&T::from(false));",
    "h.put(&(a as u8)); h.put(&(a as i16)); h.put(&(a as u32)); h.put(&(b as i64)); h.put(&(c as u128)); h.put(&(a as i128)); h.put(&(b as usize)); h.put(&(c as isize));",
    "h.put(&(a as f32)); h.put(&(b as f64)); h.put(&(a as f32 as T)); h.put(&(c as f64 as T)); h.put(&(a as u8 as char));",
    "{ use std::hash::{Hash, Hasher}; let mut s = std::collections::hash_map::DefaultHasher::new(); a.hash(&mut s); b.hash(&mut s); T::hash_slice(&[a, b, c], &mut s); h.put(&s.finish()); }",
    "{ use std::num::Wrapping; h.put(&(Wrapping(a) + Wrapping(b)).0); h.put(&(Wrapping(a) - Wrapping(b)).0); h.put(&(Wrapping(a) * Wrapping(b)).0); h.put(&(-Wrapping(a)).0); h.put(&(!Wrapping(c)).0); }",
    "{ use std::num::Wrapping; h.put(&(Wrapping(a) / Wrapping(b | 1)).0); h.put(&(Wrapping(a) % Wrapping(b | 1)).0); h.put(&(Wrapping(a) << (n as usize)).0); h.put(&(Wrapping(b) >> (n as usize)).0); h.put(&(Wrapping(a) ^ Wrapping(c)).0); }",
    "{ use std::num::Wrapping; let mut x = Wrapping(a); x += Wrapping(b); x -= Wrapping(c); x *= Wrapping(b); x ^= Wrapping(a); x <<= 3; x >>= 1; h.put(&x.0); }",
    "{ use std::num::Saturating; h.put(&(Saturating(a) + Saturating(b)).0); h.put(&(Saturating(a) - Saturating(b)).0); h.put(&(Saturating(a) * Saturating(b)).0); h.put(&(Saturating(a) / Saturating(b | 1)).0); h.put(&(Saturating(c) % Saturating(b | 1)).0); }",
    "{ use std::num::Saturating; let mut x = Saturating(a); x += Saturating(b); x -= Saturating(c); x *= Saturating(a); x /= Saturating(b | 1); h.put(&x.0); }",
    "{ use std::num::NonZero; let z = NonZero::<T>::new(a | 1).unwrap(); let z2 = NonZero::<T>::new(b | 1).unwrap(); h.put(&z.get()); h.put(&z.leading_zeros()); h.put(&z.trailing_zeros()); h.put(&z.count_ones().get()); h.put(&z.checked_mul(z2).map(|x| x.get())); h.put(&z.saturating_mul(z2).get()); h.put(&z.checked_pow(e).map(|x| x.get())); h.put(&z.saturating_pow(e).get()); }",
    "{ use std::num::NonZero; let z = NonZero::<T>::new(a | 1).unwrap(); let z2 = NonZero::<T>::new(c | 1).unwrap(); h.put(&z.cmp(&z2)); h.put(&(z == z2)); h.put(&z.min(z2).get()); h.put(&z.max(z2).get()); h.put(&T::from(z)); h.put(&z.to_string()); h.put(&NonZero::<T>::try_from(b).ok().map(|x| x.get())); h.put(&NonZero::<T>::MIN.get()); h.put(&NonZero::<T>::MAX.get()); h.put(&NonZero::<T>::BITS); }",
    "{ use std::num::NonZero; h.put(&NonZero::<T>::new(a).map(|x| x.get())); h.put(&NonZero::new(b).map_or(0 as T, |x: NonZero<T>| x.get())); h.put(&(std::mem::size_of::<Option<NonZero<T>>>() == std::mem::size_of::<T>())); h.put(&NonZero::<T>::new(c).is_some()); }",
    "let mut x = [a, b, c, a, b]; h.put(&x.iter().copied().fold(0 as T, |s, y| s.wrapping_add(y))); h.put(&x.iter().copied().max()); h.put(&x.iter().copied().min()); x.sort(); h.put(&x); h.put(&x.binary_search(&b).is_ok());",
    "h.put(&(a..=b).contains(&c)); h.put(&(a..b).contains(&c)); h.put(&(a..).contains(&b)); h.put(&(..=c).contains(&a)); h.put(&(a..b).is_empty()); h.put(&(a..=b).is_empty());",
    "h.put(&((a & 0xf) as u8..(b & 0xf) as u8).len()); h.put(&(0..(n as usize)).len()); h.put(&((c & 7) as u8..=(a & 7) as u8).count());",
    "h.put(&a.wrapping_add(b).wrapping_mul(c).rotate_left(n).swap_bytes().wrapping_sub(a).reverse_bits().wrapping_neg());",
    "if a.checked_add(b).is_some() { h.put(&a.strict_add(b)); } if a.checked_sub(b).is_some() { h.put(&a.strict_sub(b)); } if a.checked_mul(b).is_some() { h.put(&a.strict_mul(b)); } if a.checked_pow(e).is_some() { h.put(&a.strict_pow(e)); }",
    "if a.checked_neg().is_some() { h.put(&a.strict_neg()); } if a.checked_div(b).is_some() { h.put(&a.strict_div(b)); } if a.checked_rem(b).is_some() { h.put(&a.strict_rem(b)); } if a.checked_div_euclid(b).is_some() { h.put(&a.strict_div_euclid(b)); } if a.checked_rem_euclid(b).is_some() { h.put(&a.strict_rem_euclid(b)); }",
    "h.put(&a.strict_shl(n)); h.put(&b.strict_shr(n)); h.put(&a.highest_one()); h.put(&a.lowest_one()); h.put(&a.isolate_highest_one()); h.put(&a.isolate_lowest_one());",
];

const UNSIGNED: &[&str] = &[
    "h.put(&a.is_power_of_two()); h.put(&a.checked_next_power_of_two()); h.put(&(a & 0xff).next_power_of_two());",
    "h.put(&a.div_ceil(b | 1)); h.put(&a.checked_next_multiple_of(b)); h.put(&(a >> (T::BITS / 2)).next_multiple_of((b >> (T::BITS / 2)) | 1));",
    "h.put(&a.checked_add_signed(ib)); h.put(&a.wrapping_add_signed(ib)); h.put(&a.saturating_add_signed(ib)); h.put(&a.overflowing_add_signed(ib));",
    "h.put(&a.cast_signed()); h.put(&b.cast_signed().cast_unsigned()); h.put(&a.isqrt()); h.put(&b.isqrt());",
    "h.put(&a.checked_signed_diff(b)); h.put(&a.is_multiple_of(b)); h.put(&a.is_multiple_of(0));",
    "h.put(&a.checked_sub_signed(ib)); h.put(&a.wrapping_sub_signed(ib)); h.put(&a.saturating_sub_signed(ib)); h.put(&a.overflowing_sub_signed(ib));",
    "if a.checked_add_signed(ib).is_some() { h.put(&a.strict_add_signed(ib)); } if a.checked_sub_signed(ib).is_some() { h.put(&a.strict_sub_signed(ib)); } h.put(&a.bit_width());",
];

const SIGNED: &[&str] = &[
    "h.put(&a.wrapping_abs()); h.put(&a.checked_abs()); h.put(&a.saturating_abs()); h.put(&a.overflowing_abs()); h.put(&a.unsigned_abs());",
    "h.put(&a.signum()); h.put(&a.is_positive()); h.put(&a.is_negative()); h.put(&b.signum().wrapping_mul(c)); h.put(&a.saturating_neg());",
    "h.put(&a.abs().wrapping_add(0)); h.put(&(-a.wrapping_abs()).wrapping_abs());",
    "h.put(&a.checked_add_unsigned(ua)); h.put(&a.wrapping_add_unsigned(ua)); h.put(&a.saturating_add_unsigned(ua)); h.put(&a.overflowing_add_unsigned(ua));",
    "h.put(&a.checked_sub_unsigned(ua)); h.put(&a.wrapping_sub_unsigned(ua)); h.put(&a.saturating_sub_unsigned(ua)); h.put(&a.overflowing_sub_unsigned(ua));",
    "h.put(&a.cast_unsigned()); h.put(&b.cast_unsigned().cast_signed()); h.put(&a.unsigned_abs().is_power_of_two()); h.put(&b.unsigned_abs().leading_zeros());",
    "h.put(&a.checked_isqrt()); h.put(&a.wrapping_abs().max(0).isqrt());",
    "if !(a == T::MIN && (b | 1) == -1) { h.put(&a.div_euclid(b | 1)); h.put(&a.rem_euclid(b | 1)); } h.put(&a.checked_div_euclid(-1)); h.put(&a.wrapping_div_euclid(-1)); h.put(&a.overflowing_rem(-1)); h.put(&a.saturating_div(-1));",
    "h.put(&a.wrapping_neg().wrapping_abs()); h.put(&a.overflowing_neg()); h.put(&(a < 0).then_some(a.wrapping_abs()));",
    "h.put(&a.checked_neg()); h.put(&(-(a.max(T::MIN + 1))));",
    "if a.checked_add_unsigned(ua).is_some() { h.put(&a.strict_add_unsigned(ua)); } if a.checked_sub_unsigned(ua).is_some() { h.put(&a.strict_sub_unsigned(ua)); } if a.checked_abs().is_some() { h.put(&a.strict_abs()); }",
];

const BYTE: &[&str] = &[
    "h.put(&a.is_ascii()); h.put(&a.is_ascii_alphabetic()); h.put(&a.is_ascii_digit()); h.put(&a.is_ascii_alphanumeric()); h.put(&a.is_ascii_uppercase()); h.put(&a.is_ascii_lowercase());",
    "h.put(&a.is_ascii_punctuation()); h.put(&a.is_ascii_graphic()); h.put(&a.is_ascii_whitespace()); h.put(&a.is_ascii_hexdigit()); h.put(&a.is_ascii_control());",
    "h.put(&a.to_ascii_uppercase()); h.put(&a.to_ascii_lowercase()); h.put(&a.eq_ignore_ascii_case(&b)); h.put(&a.escape_ascii().to_string());",
    "let mut x = a; x.make_ascii_uppercase(); let mut y = b; y.make_ascii_lowercase(); h.put(&x); h.put(&y); h.put(&char::from(a)); h.put(&(a as char).is_alphanumeric());",
];

fn prelude(rng: &mut Rng, ty: &str) -> String {
    let b = bits(ty);
    let u = if signed_ty(ty) { counterpart(ty) } else { ty.to_string() };
    let i = if signed_ty(ty) { ty.to_string() } else { counterpart(ty) };
    let ds = *rng.pick(&["0", "7", "ff", "-1", "+5", "1z", "zz", "", "00012", "7fffffff", "-80", "101", "255", "256", "12345678901234567890"]);
    let mut p = String::new();
    p += &format!("        type T = {ty}; type U = {u}; type I = {i};\n");
    for v in ["a", "b", "c"] {
        let l = int_lit(rng, ty);
        p += &format!("        let {v}: T = {l};\n");
    }
    let (ul, il) = (lit(rng, &u), lit(rng, &i));
    p += &format!("        let ua: U = {}; let ib: I = {};\n", bbv(rng, ul), bbv(rng, il));
    let (nv, ev, dv, radix) = (small(rng, "u32", (b - 1) as usize), small(rng, "u32", 4), format!("\"{ds}\""), rng.range(2, 36));
    p += &format!("        let n: u32 = {}; let e: u32 = {}; let d: &str = {}; let r: u32 = {radix};\n", bbv(rng, nv), bbv(rng, ev), bbv(rng, dv));
    p
}

fn signed_ty(ty: &str) -> bool {
    ty.starts_with('i')
}

pub fn generate(rng: &mut Rng) -> String {
    let all = all_ints();
    let ty = *rng.pick(&all);
    let mut tables: Vec<&[&str]> = vec![ANY];
    tables.push(if is_unsigned(ty) { UNSIGNED } else { SIGNED });
    if ty == "u8" {
        tables.push(BYTE);
    }
    let n = rng.range(6, 16);
    let pre = prelude(rng, ty);
    let body = emit(rng, &pre, &tables, n);
    wrap("", &body)
}
