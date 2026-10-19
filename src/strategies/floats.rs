//! Strategy `floats`: IEEE-754 arithmetic, float<->int casts (saturating `as`), rounding, fused
//! multiply-add, formatting and parsing.
//!
//! Only operations with a fully specified result are used (+ - * / %, sqrt, abs, rounding,
//! copysign, mul_add, comparisons, `total_cmp`, casts). Deliberately NOT used because the result
//! may legitimately differ between constant folding, vectorisation and libm: `min`/`max` with
//! signed zeros, `powi`, transcendental functions. NaN payloads/signs are canonicalised before
//! hashing (so only "is NaN" matters) and before being used as the sign source of `copysign`
//! (via a select that maps NaN to 1.0).

use super::{all_ints, lit};
use crate::rng::Rng;
use crate::w;

struct V {
    name: String,
}

fn flit(rng: &mut Rng, ty: &str) -> String {
    let c: &[&str] = &[
        "0.0", "-0.0", "1.0", "-1.0", "0.5", "3.25", "1e10", "-1e10", "1e-10", "123456.789", "16777216.0", "16777217.0",
        "9007199254740993.0", "2.5", "-2.5", "0.1", "0.3", "1e30", "-7.75", "255.5", "65535.9", "4294967295.5",
    ];
    let s = match rng.below(14) {
        0 => format!("{ty}::MAX"),
        1 => format!("{ty}::MIN_POSITIVE"),
        2 => format!("{ty}::EPSILON"),
        3 => format!("{ty}::INFINITY"),
        4 if rng.chance(40) => format!("{ty}::NAN"),
        _ => format!("{}{ty}", rng.pick(c)),
    };
    // a leading '-' would bind looser than a method call on the literal
    if s.starts_with('-') { format!("({s})") } else { s }
}

fn fexpr(rng: &mut Rng, vars: &[V], ty: &str, depth: u32) -> String {
    if depth == 0 || rng.chance(15) {
        return if !vars.is_empty() && rng.chance(65) { rng.pick(vars).name.clone() } else { flit(rng, ty) };
    }
    let d = depth - 1;
    let a = fexpr(rng, vars, ty, d);
    let b = fexpr(rng, vars, ty, d);
    let other = if ty == "f64" { "f32" } else { "f64" };
    match rng.below(24) {
        0 => format!("({a} + {b})"),
        1 => format!("({a} - {b})"),
        2 => format!("({a} * {b})"),
        3 => format!("({a} / {b})"),
        4 => format!("({a} % {b})"),
        5 => format!("{a}.sqrt()"),
        6 => format!("{a}.abs()"),
        7 => format!("(-{a})"),
        8 => format!("{a}.floor()"),
        9 => format!("{a}.ceil()"),
        10 => format!("{a}.trunc()"),
        11 => format!("{a}.round()"),
        12 => format!("{a}.round_ties_even()"),
        // the sign bit of a NaN is unspecified (hardware vs constant folding differ): copy signs from canonical values only
        13 => format!("{a}.copysign({}({b}))", if ty == "f64" { "sg64" } else { "sg32" }),
        14 => format!("{a}.mul_add({b}, {})", fexpr(rng, vars, ty, d)),
        15 => format!("{a}.signum()"),
        16 => format!("(if {a} < {b} {{ {a} }} else {{ {b} }})"),
        17 => format!("(if {a} >= {b} {{ {a} }} else {{ {b} }})"),
        18 => format!("{a}.recip()"),
        19 => format!("({a} as {other} as {ty})"),
        20 => {
            let it = *rng.pick(&all_ints());
            format!("({a} as {it} as {ty})")
        }
        21 => format!("{a}.fract()"),
        22 => format!("{ty}::from_bits({a}.to_bits() ^ {})", if ty == "f64" { "0x8000_0000_0000_0000u64" } else { "0x8000_0000u32" }),
        _ => {
            let it = *rng.pick(&all_ints());
            format!("({} as {ty})", lit(rng, it))
        }
    }
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    // NaN -> the integer 1 (not a NaN bit pattern): LLVM folds `select(isnan(x), <qNaN constant>, x)` to `x`
    // (it treats all NaNs as interchangeable) and the payload/sign then leaks through `to_bits`; see
    // reports/llvm-select-nan-canonicalisation.rs
    w!(items, "    fn c64(x: f64) -> u64 {{ if x.is_nan() {{ 1 }} else {{ x.to_bits() }} }}");
    w!(items, "    fn c32(x: f32) -> u32 {{ if x.is_nan() {{ 1 }} else {{ x.to_bits() }} }}");
    w!(items, "    fn sg64(x: f64) -> f64 {{ if x.is_nan() {{ 1.0 }} else {{ x }} }}");
    w!(items, "    fn sg32(x: f32) -> f32 {{ if x.is_nan() {{ 1.0 }} else {{ x }} }}");
    w!(items, "    fn sh(s: &str) -> u64 {{ s.bytes().fold(7u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64)) }}");
    let mut body = String::new();
    for k in 0..rng.range(2, 5) {
        let ty = *rng.pick(&["f64", "f32"]);
        let cv = if ty == "f64" { "c64" } else { "c32" };
        let mut vars = Vec::new();
        w!(body, "        {{");
        for i in 0..rng.range(2, 4) {
            w!(body, "        let v{i}: {ty} = bb({});", flit(rng, ty));
            vars.push(V { name: format!("v{i}") });
        }
        for i in 0..rng.range(2, 6) {
            let d = rng.range(2, 5) as u32;
            let e = fexpr(rng, &vars, ty, d);
            w!(body, "        let t{i}: {ty} = {e};");
            w!(body, "        h.put(&{cv}(t{i})); h.put(&t{i}.is_nan()); h.put(&(t{i} as i8)); h.put(&(t{i} as u8)); h.put(&(t{i} as i32)); h.put(&(t{i} as u64)); h.put(&(t{i} as i128)); h.put(&(t{i} as u128)); h.put(&(t{i} as i64));");
            w!(body, "        h.put(&sh(&format!(\"{{}}|{{:?}}|{{:e}}|{{:.3}}\", t{i}, t{i}, t{i}, t{i})));");
            vars.push(V { name: format!("t{i}") });
        }
        // accumulator loop
        let step = fexpr(rng, &[V { name: "acc".into() }, V { name: "x".into() }], ty, 2);
        w!(body, "        let mut acc: {ty} = bb({}); for i in 0..bb({}u32) {{ let x = i as {ty}; acc = {step}; }} h.put(&{cv}(acc));", flit(rng, ty), rng.range(2, 40));
        // arrays: sum / sort by total_cmp / extremes
        w!(body, "        let mut arr: Vec<{ty}> = (0..bb({}u32)).map(|i| (i as {ty}) * {} - {}).collect();", rng.range(0, 30), flit(rng, ty), flit(rng, ty));
        // NaN sign/payload is unspecified (hardware gives -NaN for 0*inf, Miri a different one): order NaNs as 1.0
        let sg = if ty == "f64" { "sg64" } else { "sg32" };
        w!(body, "        h.put(&{cv}(arr.iter().copied().sum::<{ty}>())); arr.sort_by(|a, b| {sg}(*a).total_cmp(&{sg}(*b))); h.put(&arr.iter().map(|x| {cv}(*x) as u64).fold(0u64, |a, b| a.rotate_left(5) ^ b));");
        // int -> float conversions of edge values
        let it = *rng.pick(&all_ints());
        w!(body, "        h.put(&{cv}(bb({}) as {ty})); h.put(&{cv}(({}) as {ty} / bb({})));", lit(rng, it), lit(rng, it), flit(rng, ty).replace("NAN", "EPSILON"));
        // parse
        w!(body, "        h.put(&\"{}\".parse::<{ty}>().map(|x| {cv}(x) as u64).unwrap_or(1));", rng.pick(&["1.5e3", "-0.0", "1e400", "0.1", "nan", "inf", "123456789.123456789", "1e-50", "x", "2.5E+2"]));
        w!(body, "        }}");
        let _ = k;
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
