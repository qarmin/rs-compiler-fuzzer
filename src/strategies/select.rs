//! Strategy `select`: structs and tuples built by `if`/`else`/`match` over clamped integers with
//! mixed integer/float/bool fields. Targets InstCombine's select -> min/max rewrites and
//! range-metadata handling (rust-lang/rust#163728: the clamped `i64` field of a conditionally
//! constructed struct came out unclamped), SROA and GVN of aggregates.
//!
//! Safe code only. Inputs sweep the boundary values (limit-1, limit, limit+1, MIN, MAX...).

use super::{bits, lit, signed};
use crate::rng::Rng;
use crate::w;

const INTS: &[&str] = &["i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "i128", "u128"];

fn clamp_expr(rng: &mut Rng, ty: &str, x: &str, lo: &str, hi: &str) -> String {
    match rng.below(7) {
        0 => format!("{x}.max({lo})"),
        1 => format!("{x}.min({hi})"),
        2 => format!("{x}.max({lo}).min({hi})"),
        3 => format!("std::cmp::min(std::cmp::max({x}, {lo}), {hi})"),
        4 => format!("if {x} < {lo} {{ {lo} }} else if {x} > {hi} {{ {hi} }} else {{ {x} }}"),
        5 => format!("{x}.saturating_sub(1).max({lo}).saturating_add(1).min({hi})"),
        _ => format!("[{x}, {lo}, {hi}].into_iter().max().unwrap().min({hi})"),
    }
    .replace("TY", ty)
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    for id in 0..rng.range(2, 5) {
        let ty = *rng.pick(INTS);
        let b = bits(ty);
        let sh = rng.range(3.min(b as usize - 2), (b.min(62) - if signed(ty) { 2 } else { 1 }) as usize);
        let (hi, lo) = if signed(ty) {
            (format!("((1{ty} << {sh}) - 1)"), format!("(-(1{ty} << {sh}))"))
        } else {
            (format!("((1{ty} << {sh}) - 1)"), format!("{}{ty}", rng.below(5)))
        };
        let float = *rng.pick(&["f64", "f32"]);
        let other = *rng.pick(&["u8", "i32", "u16", "i64"]);
        w!(items, "    #[derive(Clone, Copy, Debug, PartialEq)] struct S{id} {{ a: {ty}, f: {float}, c: {other}, d: bool }}");
        // constructor with a branch on the first field
        let cmp = *rng.pick(&[">", ">=", "<", "<="]);
        let thr = if cmp.starts_with('>') { hi.clone() } else { lo.clone() };
        let clamp = clamp_expr(rng, ty, "lv.a", &lo, &hi);
        let then_c = lit(rng, other);
        let else_c = format!("lv.c.wrapping_add({})", lit(rng, other));
        let shape = rng.below(4);
        let ctor = match shape {
            0 => format!("if lv.a {cmp} {thr} {{ S{id} {{ a: {thr}, f: 1.0, c: {then_c}, d: true }} }} else {{ S{id} {{ a: {clamp}, f: lv.f, c: {else_c}, d: lv.d }} }}"),
            1 => format!("let a = if lv.a {cmp} {thr} {{ {thr} }} else {{ {clamp} }}; let f = if lv.a {cmp} {thr} {{ 1.0 }} else {{ lv.f }}; S{id} {{ a, f, c: {else_c}, d: lv.a {cmp} {thr} }}"),
            2 => format!("match lv.a {cmp} {thr} {{ true => S{id} {{ a: {thr}, f: lv.f + 1.0, c: {then_c}, d: !lv.d }}, false => S{id} {{ a: {clamp}, f: lv.f, c: {else_c}, d: lv.d }} }}"),
            _ => format!("let t = (if lv.a {cmp} {thr} {{ {thr} }} else {{ {clamp} }}, lv.f * 0.5); S{id} {{ a: t.0, f: t.1, c: if t.0 == {thr} {{ {then_c} }} else {{ {else_c} }}, d: t.0 {cmp} {thr} }}"),
        };
        let attr = *rng.pick(&["#[inline(never)]", "#[inline(always)]", ""]);
        w!(items, "    {attr}\n    fn mk{id}(lv: S{id}) -> S{id} {{ {ctor} }}");
        // generic variant through a closure / min_by, as in the issue's second reproducer
        w!(items, "    fn mk{id}_by(lv: S{id}, g: impl Fn({ty}) -> {ty}) -> S{id} {{ let a = g(lv.a); S{id} {{ a: std::cmp::min_by(a, {hi}, |x, y| x.cmp(y)), f: lv.f, c: lv.c, d: lv.d }} }}");
        // sweep boundary values
        let vals = [
            format!("{thr}"),
            format!("{thr}.wrapping_add(1)"),
            format!("{thr}.wrapping_sub(1)"),
            format!("{ty}::MAX"),
            format!("{ty}::MIN"),
            "0".to_string(),
            format!("{lo}.wrapping_sub(1)"),
            format!("{lo}.wrapping_add(1)"),
        ];
        w!(body, "        for v in [{}] {{", vals.join(", "));
        w!(body, "            let lv = S{id} {{ a: bb(v), f: bb({}.0), c: bb({}), d: bb({}) }};", rng.below(3), lit(rng, other), rng.chance(50));
        w!(body, "            let r = mk{id}(lv); h.put(&r.a); h.put(&r.f.to_bits()); h.put(&r.c); h.put(&r.d);");
        w!(body, "            let r2 = mk{id}_by(lv, |x| x.wrapping_mul({})); h.put(&r2.a);", lit(rng, ty));
        w!(body, "        }}");
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
