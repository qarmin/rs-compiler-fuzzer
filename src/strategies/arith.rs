//! Strategy `arith`: random typed integer expression trees.
//!
//! Safe code only. All operations are total: wrapping/saturating/checked/overflowing
//! arithmetic, `wrapping_shl/shr` (shift amount is masked by the language), divisors
//! are `x | 1` (never zero), and signed division uses `wrapping_div/rem` (no MIN / -1 trap).
//! Variants: pure functions that are `#[inline(never)]` / `#[inline(always)]` / plain,
//! and accumulator loops folding an expression over the induction variable.

use super::{all_ints, bits, lit, signed};
use crate::rng::Rng;
use crate::w;

pub(super) struct Var {
    pub name: String,
    pub ty: &'static str,
}

struct Ctx<'a> {
    vars: &'a [Var],
}

fn leaf(rng: &mut Rng, cx: &Ctx, ty: &'static str) -> String {
    let same: Vec<&Var> = cx.vars.iter().filter(|v| v.ty == ty).collect();
    if !same.is_empty() && rng.chance(70) {
        return rng.pick(&same).name.clone();
    }
    if !cx.vars.is_empty() && rng.chance(50) {
        let v = rng.pick(cx.vars);
        return format!("({} as {ty})", v.name);
    }
    lit(rng, ty)
}

fn cond(rng: &mut Rng, cx: &Ctx, depth: u32) -> String {
    let ty = *rng.pick(&all_ints());
    let a = expr(rng, cx, ty, depth.saturating_sub(1));
    let b = expr(rng, cx, ty, depth.saturating_sub(1));
    let op = *rng.pick(&["<", "<=", ">", ">=", "==", "!="]);
    let c = format!("{a} {op} {b}");
    match rng.below(6) {
        0 => format!("!({c})"),
        1 => {
            let d = cond(rng, cx, depth.saturating_sub(2));
            format!("(({c}) && ({d}))")
        }
        2 => {
            let d = cond(rng, cx, depth.saturating_sub(2));
            format!("(({c}) || ({d}))")
        }
        _ => format!("({c})"),
    }
}

fn expr(rng: &mut Rng, cx: &Ctx, ty: &'static str, depth: u32) -> String {
    if depth == 0 || rng.chance(12) {
        return leaf(rng, cx, ty);
    }
    let d = depth - 1;
    let sub = |rng: &mut Rng| expr(rng, cx, ty, d);
    let b = bits(ty);
    match rng.below(24) {
        0 => format!("{}.wrapping_add({})", sub(rng), sub(rng)),
        1 => format!("{}.wrapping_sub({})", sub(rng), sub(rng)),
        2 => format!("{}.wrapping_mul({})", sub(rng), sub(rng)),
        3 => format!("({} & {})", sub(rng), sub(rng)),
        4 => format!("({} | {})", sub(rng), sub(rng)),
        5 => format!("({} ^ {})", sub(rng), sub(rng)),
        6 => format!("{}.wrapping_shl({} as u32)", sub(rng), expr(rng, cx, "u8", d)),
        7 => format!("{}.wrapping_shr({} as u32)", sub(rng), expr(rng, cx, "u8", d)),
        8 => format!("{}.rotate_left({} as u32)", sub(rng), expr(rng, cx, "u8", d)),
        9 => format!("{}.rotate_right({} as u32)", sub(rng), expr(rng, cx, "u8", d)),
        10 => format!("{}.saturating_add({})", sub(rng), sub(rng)),
        11 => format!("{}.saturating_sub({})", sub(rng), sub(rng)),
        12 => format!("{}.checked_add({}).unwrap_or({})", sub(rng), sub(rng), sub(rng)),
        13 => format!("{}.checked_mul({}).unwrap_or({})", sub(rng), sub(rng), sub(rng)),
        14 => format!("{}.overflowing_mul({}).0", sub(rng), sub(rng)),
        15 => format!("({}.overflowing_sub({}).1 as {ty})", sub(rng), sub(rng)),
        16 => {
            if signed(ty) {
                format!("{}.wrapping_div({} | 1)", sub(rng), sub(rng))
            } else {
                format!("({} / ({} | 1))", sub(rng), sub(rng))
            }
        }
        17 => {
            if signed(ty) {
                format!("{}.wrapping_rem({} | 1)", sub(rng), sub(rng))
            } else {
                format!("({} % ({} | 1))", sub(rng), sub(rng))
            }
        }
        18 => match rng.below(5) {
            0 => format!("{}.min({})", sub(rng), sub(rng)),
            1 => format!("{}.max({})", sub(rng), sub(rng)),
            2 => format!("(!{})", sub(rng)),
            3 => format!("{}.wrapping_neg()", sub(rng)),
            _ => format!("{}.swap_bytes()", sub(rng)),
        },
        19 => match rng.below(4) {
            0 => format!("({}.count_ones() as {ty})", sub(rng)),
            1 => format!("({}.leading_zeros() as {ty})", sub(rng)),
            2 => format!("({}.trailing_zeros() as {ty})", sub(rng)),
            _ => format!("{}.reverse_bits()", sub(rng)),
        },
        20 => {
            let e = rng.below(4);
            format!("{}.wrapping_pow({e})", sub(rng))
        }
        21 => {
            let other = *rng.pick(&all_ints());
            format!("({} as {ty})", expr(rng, cx, other, d))
        }
        22 => format!("(if {} {{ {} }} else {{ {} }})", cond(rng, cx, d), sub(rng), sub(rng)),
        _ => {
            if b >= 16 && !signed(ty) {
                format!("(({} | 1).ilog2() as {ty})", sub(rng))
            } else {
                format!("{}.wrapping_add({})", sub(rng), sub(rng))
            }
        }
    }
}

/// Entry point reused by other strategies for loop bodies etc.
pub(super) fn gen_expr(rng: &mut Rng, vars: &[Var], ty: &'static str, depth: u32) -> String {
    expr(rng, &Ctx { vars }, ty, depth)
}

fn feed(var: &str) -> String {
    format!("        h.put(&{var});\n")
}

pub fn generate(rng: &mut Rng) -> String {
    let mut s = String::new();
    let types = all_ints();
    let nfn = rng.range(2, 5);
    let mut calls = String::new();
    for f in 0..nfn {
        let nparams = rng.range(1, 4);
        let params: Vec<Var> = (0..nparams).map(|i| Var { name: format!("p{i}"), ty: *rng.pick(&types) }).collect();
        let ret = *rng.pick(&types);
        let attr = *rng.pick(&["#[inline(never)]", "#[inline(always)]", "#[inline]", "", "#[cold] #[inline(never)]"]);
        let sig: Vec<String> = params.iter().map(|p| format!("{}: {}", p.name, p.ty)).collect();
        let mut locals: Vec<Var> = params.iter().map(|p| Var { name: p.name.clone(), ty: p.ty }).collect();
        let mut body = String::new();
        for l in 0..rng.range(1, 4) {
            let ty = *rng.pick(&types);
            let depth_ = rng.range(2, 5) as u32;
            let e = expr(rng, &Ctx { vars: &locals }, ty, depth_);
            w!(body, "        let l{l}: {ty} = {e};");
            locals.push(Var { name: format!("l{l}"), ty });
        }
        let depth_ = rng.range(2, 5) as u32;
        let tail = expr(rng, &Ctx { vars: &locals }, ret, depth_);
        w!(s, "    {attr}\n    fn f{f}({}) -> {ret} {{\n{body}        {tail}\n    }}", sig.join(", "));
        for _ in 0..rng.range(1, 3) {
            let args: Vec<String> = params.iter().map(|p| format!("bb({})", lit(rng, p.ty))).collect();
            w!(calls, "        let r = f{f}({});\n        h.put(&r);", args.join(", "));
        }
    }

    w!(s, "    pub fn run(h: &mut H) {{");
    // plain straight-line section
    let nin = rng.range(2, 5);
    let mut vars: Vec<Var> = Vec::new();
    for i in 0..nin {
        let ty = *rng.pick(&types);
        w!(s, "        let v{i}: {ty} = bb({});", lit(rng, ty));
        vars.push(Var { name: format!("v{i}"), ty });
    }
    for i in 0..rng.range(2, 6) {
        let ty = *rng.pick(&types);
        let depth_ = rng.range(2, 6) as u32;
        let e = expr(rng, &Ctx { vars: &vars }, ty, depth_);
        w!(s, "        let t{i}: {ty} = {e};");
        s.push_str(&feed(&format!("t{i}")));
        vars.push(Var { name: format!("t{i}"), ty });
    }
    s.push_str(&calls);

    // accumulator loops over an expression of (acc, i, inputs)
    for k in 0..rng.range(1, 3) {
        let ty = *rng.pick(&types);
        let n = rng.range(3, 40);
        let mut lv: Vec<Var> = Vec::new();
        lv.push(Var { name: "acc".into(), ty });
        lv.push(Var { name: "i".into(), ty: "u32" });
        for v in vars.iter().take(3) {
            lv.push(Var { name: v.name.clone(), ty: v.ty });
        }
        let depth_ = rng.range(2, 5) as u32;
        let step = expr(rng, &Ctx { vars: &lv }, ty, depth_);
        w!(s, "        let mut acc{k}: {ty} = bb({});", lit(rng, ty));
        let step = step.replace("acc", &format!("acc{k}"));
        w!(s, "        for i in 0..bb({n}u32) {{ acc{k} = {step}; }}");
        s.push_str(&feed(&format!("acc{k}")));
    }
    w!(s, "    }}");
    s
}
