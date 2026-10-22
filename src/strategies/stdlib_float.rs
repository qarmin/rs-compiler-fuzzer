//! `stdlib` family: `f32`/`f64` methods with fully specified results (no libm functions, see `stdlib.rs`).
//! Prelude: `T`, `a b c` (any finite or infinite value, never NaN), `p q r` (finite, non-zero: safe for `min`/`max`/`clamp`,
//! whose result for signed zeros is unspecified), `i: i32`, `u: u64`/`u32` bit pattern.

use super::stdlib::{bbv, emit, wrap};
use crate::rng::Rng;

pub const NAMES: &[&str] = &["float"];

const ANY: &[&str] = &[
    "h.put(&a.abs()); h.put(&(-b)); h.put(&c.signum()); h.put(&a.copysign(b)); h.put(&p.copysign(-q)); h.put(&c.recip());",
    "h.put(&a.floor()); h.put(&b.ceil()); h.put(&c.round()); h.put(&a.trunc()); h.put(&b.fract()); h.put(&c.round_ties_even());",
    "h.put(&a.sqrt()); h.put(&p.abs().sqrt()); h.put(&b.mul_add(c, a)); h.put(&p.mul_add(q, r)); h.put(&a.mul_add(0.0, b));",
    "h.put(&p.min(q)); h.put(&p.max(q)); h.put(&p.clamp(q.min(r), q.max(r))); h.put(&p.min(r).max(q)); h.put(&T::min(q, r));",
    "h.put(&a.total_cmp(&b)); h.put(&b.total_cmp(&b)); h.put(&a.partial_cmp(&c)); h.put(&(a < b)); h.put(&(a == c)); h.put(&(a >= b)); h.put(&a.lt(&c)); h.put(&b.ge(&a));",
    "h.put(&a.is_nan()); h.put(&(a / b).is_nan()); h.put(&a.is_infinite()); h.put(&b.is_finite()); h.put(&c.is_normal()); h.put(&a.is_subnormal()); h.put(&b.is_sign_positive()); h.put(&c.is_sign_negative());",
    "h.put(&(match a.classify() { std::num::FpCategory::Nan => 0u8, std::num::FpCategory::Infinite => 1, std::num::FpCategory::Zero => 2, std::num::FpCategory::Subnormal => 3, std::num::FpCategory::Normal => 4 }));",
    "h.put(&a.to_bits()); h.put(&T::from_bits(a.to_bits() ^ 1)); h.put(&T::from_bits(u)); h.put(&T::from_bits(u.rotate_left(3)).is_nan());",
    "h.put(&a.to_be_bytes()); h.put(&b.to_le_bytes()); h.put(&c.to_ne_bytes()); h.put(&T::from_be_bytes(a.to_be_bytes())); h.put(&T::from_le_bytes(b.to_le_bytes())); h.put(&T::from_ne_bytes(c.to_ne_bytes()));",
    "h.put(&a.to_degrees()); h.put(&b.to_radians()); h.put(&(a % p)); h.put(&(a + b)); h.put(&(a - b)); h.put(&(a * c)); h.put(&(a / q));",
    "h.put(&a.rem_euclid(p)); h.put(&b.div_euclid(q)); h.put(&a.midpoint(b)); h.put(&p.midpoint(q)); h.put(&a.next_up()); h.put(&b.next_down());",
    "h.put(&(a as i8)); h.put(&(a as u8)); h.put(&(b as i16)); h.put(&(b as u16)); h.put(&(c as i32)); h.put(&(c as u32)); h.put(&(a as i64)); h.put(&(a as u64)); h.put(&(b as i128)); h.put(&(c as u128)); h.put(&(a as usize)); h.put(&(b as isize));",
    "h.put(&(i as T)); h.put(&(u as T)); h.put(&(i as T as i32)); h.put(&(a as f32)); h.put(&(b as f64)); h.put(&(a as f32 as f64)); h.put(&f64::from(a as f32)); h.put(&f64::from(i)); h.put(&f64::from(i as u8)); h.put(&f32::from(i as i16)); h.put(&f32::from(true as u8));",
    "h.put(&T::MAX); h.put(&T::MIN); h.put(&T::MIN_POSITIVE); h.put(&T::EPSILON); h.put(&T::INFINITY); h.put(&T::NEG_INFINITY); h.put(&T::NAN); h.put(&T::MANTISSA_DIGITS); h.put(&T::DIGITS); h.put(&T::RADIX); h.put(&T::MAX_EXP); h.put(&T::MIN_EXP);",
    "h.put(&format!(\"{}\", a)); h.put(&format!(\"{:?}\", b)); h.put(&format!(\"{:.3}\", c)); h.put(&format!(\"{:e}\", a)); h.put(&format!(\"{:E}\", b)); h.put(&format!(\"{:10.2}|{:<10.1}|{:+}\", c, a, b));",
    "h.put(&format!(\"{:08.3}\", p)); h.put(&format!(\"{:.0}\", q)); h.put(&format!(\"{:.1e}\", r)); h.put(&format!(\"{:#?}\", p)); h.put(&format!(\"{:>9}\", a)); h.put(&format!(\"{:^9.2}\", b));",
    "h.put(&\"1.5e3\".parse::<T>().ok()); h.put(&\"-0\".parse::<T>().ok()); h.put(&\"inf\".parse::<T>().ok()); h.put(&\"NaN\".parse::<T>().map(|x| x.is_nan()).ok()); h.put(&\".5\".parse::<T>().ok()); h.put(&\"1e400\".parse::<T>().ok()); h.put(&\"abc\".parse::<T>().is_err());",
    "h.put(&a.to_string().parse::<T>().ok()); h.put(&format!(\"{:e}\", b).parse::<T>().ok()); h.put(&format!(\"{:?}\", c).parse::<T>().ok());",
    "let mut v = [a, b, c, p, q, r]; v.sort_by(|x, y| x.total_cmp(y)); h.put(&v); v.sort_unstable_by(|x, y| y.total_cmp(x)); h.put(&v); h.put(&v.iter().copied().fold(T::NEG_INFINITY, T::max)); h.put(&v.iter().copied().fold(T::INFINITY, T::min));",
    "let v = [p, q, r, p]; h.put(&v.iter().sum::<T>()); h.put(&v.iter().product::<T>()); h.put(&v.iter().copied().sum::<T>()); h.put(&v.iter().copied().fold(0.0, |s, x| s + x)); h.put(&v.iter().copied().map(|x| x * 2.0).sum::<T>());",
    "let v = [p, q, r]; h.put(&v.iter().copied().max_by(|x, y| x.total_cmp(y))); h.put(&v.iter().copied().min_by(|x, y| x.partial_cmp(y).unwrap())); h.put(&v.iter().position(|&x| x == q)); h.put(&v.iter().copied().reduce(T::max));",
    "h.put(&a.max(f64::NEG_INFINITY as T).is_nan()); h.put(&(a == a)); h.put(&(a != b)); h.put(&(0.0 as T == -0.0)); h.put(&(p.abs() > 0.0)); h.put(&(a.abs() >= 0.0));",
    "let mut x = a; x += b; x -= p; x *= q; x /= r; x %= p; h.put(&x); h.put(&(x.is_finite() as u8));",
    "{ use std::ops::*; h.put(&Add::add(a, b)); h.put(&Sub::sub(a, b)); h.put(&Mul::mul(p, q)); h.put(&Div::div(a, r)); h.put(&Rem::rem(a, q)); h.put(&Neg::neg(c)); }",
    "{ use std::hash::{Hash, Hasher}; let mut s = std::collections::hash_map::DefaultHasher::new(); a.to_bits().hash(&mut s); b.to_bits().hash(&mut s); h.put(&s.finish()); }",
    "h.put(&(p as i32).wrapping_add(q as i32)); h.put(&((a.abs().min(1e9)) as i32)); h.put(&((b.round() as i64).wrapping_mul(3))); h.put(&(c.clamp(-100.0, 100.0) as u8));",
    "h.put(&T::from_bits(a.to_bits() & !(1 << (T::MANTISSA_DIGITS + 5)))); h.put(&a.to_bits().count_ones()); h.put(&(a.to_bits() >> 3)); h.put(&(T::from_bits(b.to_bits() | 1) - b));",
    "h.put(&(a.abs() + p.abs()).floor().rem_euclid(7.0)); h.put(&((b * 0.5).trunc() * 2.0)); h.put(&(a - a.trunc() - a.fract())); h.put(&(c.round() - c.round_ties_even()).abs());",
    "/*libm*/ h.put(&qz(a.sin())); h.put(&qz(b.cos())); h.put(&qz(c.tan())); h.put(&qz(p.asin())); h.put(&qz(q.acos()));",
    "/*libm*/ h.put(&qz(a.atan())); h.put(&qz(b.atan2(c))); h.put(&qz(p.sinh())); h.put(&qz(q.cosh())); h.put(&qz(r.tanh())); h.put(&qz(a.asinh())); h.put(&qz(p.abs().acosh())); h.put(&qz(q.atan())); h.put(&{ let (s, c2) = a.sin_cos(); qz(s + c2) });",
    "/*libm*/ h.put(&qz(p.abs().sqrt().exp())); h.put(&qz(a.exp2())); h.put(&qz(b.exp_m1())); h.put(&qz(p.abs().ln())); h.put(&qz(q.abs().log2())); h.put(&qz(r.abs().log10())); h.put(&qz(p.abs().ln_1p())); h.put(&qz(q.abs().log(3.0))); h.put(&qz(p.abs().cbrt()));",
    "/*libm*/ h.put(&qz(p.powf(2.0))); h.put(&qz(q.abs().powf(r))); h.put(&qz(a.powi(2))); h.put(&qz(b.powi(-3))); h.put(&qz(p.hypot(q))); h.put(&qz((r.abs() * 0.001).atanh()));",
];

fn flit(rng: &mut Rng, ty: &str, nonzero: bool) -> String {
    let c: &[&str] = &[
        "0.0", "-0.0", "1.0", "-1.0", "0.5", "3.25", "1e10", "-1e10", "1e-10", "123456.789", "16777216.0", "16777217.0", "9007199254740993.0",
        "2.5", "-2.5", "0.1", "0.3", "1e30", "-7.75", "255.5", "65535.9", "4294967295.5", "1.5", "-0.5", "7.0", "100.0",
    ];
    let nz: &[&str] = &["1.0", "-1.0", "0.5", "3.25", "1e10", "-1e10", "1e-10", "123456.789", "2.5", "-2.5", "0.1", "-7.75", "255.5", "1.5", "-0.5", "7.0"];
    let s = if nonzero {
        match rng.below(10) {
            0 => format!("{ty}::MAX"),
            1 => format!("{ty}::MIN_POSITIVE"),
            2 => format!("{ty}::EPSILON"),
            3 => format!("-{ty}::MAX"),
            _ => format!("{}{ty}", rng.pick(nz)),
        }
    } else {
        match rng.below(14) {
            0 => format!("{ty}::MAX"),
            1 => format!("{ty}::MIN_POSITIVE"),
            2 => format!("{ty}::EPSILON"),
            3 => format!("{ty}::INFINITY"),
            4 => format!("{ty}::NEG_INFINITY"),
            5 => format!("-{ty}::MAX"),
            _ => format!("{}{ty}", rng.pick(c)),
        }
    };
    if s.starts_with('-') { format!("({s})") } else { s }
}

pub fn generate(rng: &mut Rng) -> String {
    let ty = *rng.pick(&["f32", "f64"]);
    let bitty = if ty == "f32" { "u32" } else { "u64" };
    let mut p = format!("        type T = {ty};\n        fn qz(x: T) -> i64 {{ (x * 1024.0).round() as i64 }}\n");
    for v in ["a", "b", "c"] {
        let l = flit(rng, ty, false);
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    for v in ["p", "q", "r"] {
        let l = flit(rng, ty, true);
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    let iv = format!("{}i32", rng.below(2000) as i64 - 1000);
    let uv = format!("{}{bitty}", rng.next_u64() >> if ty == "f32" { 32 } else { 0 });
    p += &format!("        let i: i32 = {}; let u: {bitty} = {};\n", bbv(rng, iv), bbv(rng, uv));
    let n = rng.range(6, 14);
    let body = emit(rng, &p, &[ANY], n);
    wrap("", &body)
}
