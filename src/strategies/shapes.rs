//! Strategy `shapes`: programs built after specific rustc miscompilation reports that no other strategy covered.
//!
//! - `wide_chain` (rust-lang/rust#164667): chains of shadowed `let`s over 128-bit (and narrower) integers whose seeds go through
//!   `black_box`, mixing `rotate_*`, `checked_mul(..).unwrap_or(..)`, `abs_diff`, casts between widths. The report is a wrong
//!   `abs_diff` result after LLVM's stack colouring merged stack slots.
//! - `ptr_chain` (rust-lang/rust#164137): `static mut` raw pointers that all point at one another; reads and writes through
//!   chains of them in a loop with an early `return` (early-cse treated the function as unreachable).
//!
//! Validity: integer code is wrapping/checked/total only. In `ptr_chain` every static is first set to the address of the first
//! static, so every pointer read back is that valid, aligned address; writes only ever store such a pointer.

use super::{bits, lit_plain, signed};
use crate::rng::Rng;
use crate::w;

const TYS: &[&str] = &["i128", "u128", "i128", "u128", "i64", "u64", "u32", "i32", "u16", "i16", "u8", "i8", "usize"];

fn unsigned_of(ty: &str) -> String {
    match ty.strip_prefix('i') {
        Some(rest) => format!("u{rest}"),
        None => ty.to_string(),
    }
}

fn wide_chain(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let _ = items;
    let mut vars: Vec<(String, &'static str)> = Vec::new();
    w!(body, "        {{ // shape of rust-lang/rust#164667");
    for n in 0..rng.range(2, 4) {
        let ty = *rng.pick(TYS);
        w!(body, "        let s{id}_{n}: {ty} = bb({});", lit_plain(rng, ty));
        vars.push((format!("s{id}_{n}"), ty));
    }
    for step in 0..rng.range(3, 9) {
        let (a, ta) = rng.pick(&vars).clone();
        let (b, tb) = rng.pick(&vars).clone();
        // shadow an existing name half of the time: the report relies on it, and it keeps the same stack slot candidates alive
        let name = if rng.chance(50) { rng.pick(&vars).0.clone() } else { format!("t{id}_{step}") };
        let new_ty;
        let expr = match rng.below(12) {
            0 => {
                new_ty = ta;
                format!("({}).rotate_right({}).checked_mul({a}).unwrap_or(0)", lit_plain(rng, ta), rng.below(bits(ta) as usize))
            }
            1 => {
                new_ty = ta;
                format!("{a}.rotate_left({}).checked_add({b} as {ta}).unwrap_or({})", rng.below(bits(ta) as usize), lit_plain(rng, ta))
            }
            2 => {
                let u = unsigned_of(ta);
                new_ty = if signed(ta) { Box::leak(u.clone().into_boxed_str()) } else { ta };
                format!("({b} as {ta}).abs_diff({a} as {tb} as {ta})")
            }
            3 => {
                let u = unsigned_of(tb);
                new_ty = Box::leak(u.clone().into_boxed_str());
                format!("({a} as {tb} as {u}).abs_diff({b} as {u})")
            }
            4 => {
                new_ty = ta;
                format!("{a}.wrapping_neg().wrapping_add({b} as {ta})")
            }
            5 => {
                new_ty = "u32";
                format!("{a}.leading_zeros() ^ {b}.trailing_zeros()")
            }
            6 => {
                new_ty = ta;
                format!("{a}.swap_bytes().rotate_right({})", rng.below(bits(ta) as usize))
            }
            7 => {
                new_ty = tb;
                format!("({a} as {tb}).wrapping_mul({b}).rotate_left({})", rng.below(bits(tb) as usize))
            }
            8 => {
                new_ty = ta;
                format!("{a}.checked_div(({b} as {ta}) | 1).unwrap_or({a})")
            }
            9 => {
                new_ty = ta;
                format!("{a}.saturating_sub({b} as {ta}).max({}).min({a})", lit_plain(rng, ta))
            }
            10 => {
                new_ty = "u128";
                format!("({a} as u128) << {} | ({b} as u128) >> {}", rng.below(128), rng.below(128))
            }
            _ => {
                new_ty = ta;
                format!("{a}.wrapping_shl({}) ^ ({b} as {ta}).reverse_bits()", rng.below(200))
            }
        };
        w!(body, "        let {name}: {new_ty} = {expr};");
        w!(body, "        h.put(&{name});");
        vars.retain(|(n, _)| *n != name);
        vars.push((name, new_ty));
    }
    w!(body, "        }}");
}

fn ptr_chain(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let n = rng.range(3, 6);
    w!(items, "    // shape of rust-lang/rust#164137 (early-cse, opt-level=1)");
    for k in 0..n {
        w!(items, "    static mut G{id}_{k}: *mut u8 = std::ptr::null_mut::<u8>();");
    }
    let rd = |k: usize| format!("std::ptr::read::<*mut u8>(&raw const G{id}_{k})");
    let hop = |k: usize| format!("std::ptr::read({}.cast::<*mut u8>())", rd(k));
    w!(items, "    #[inline(never)]\n    unsafe fn chain{id}(rounds: u32) -> u32 {{");
    w!(items, "        let mut acc = 0u32;");
    w!(items, "        for i in 0..rounds {{");
    w!(items, "            if i == 0 {{");
    w!(items, "                std::ptr::write(&raw mut G{id}_0, (&raw mut G{id}_0).cast::<u8>());");
    for k in 1..n {
        w!(items, "                std::ptr::write(&raw mut G{id}_{k}, {});", hop(rng.below(k)));
    }
    w!(items, "            }} else {{");
    let early = rng.chance(60);
    for j in 0..rng.range(3, 8) {
        let (x, y) = (rng.below(n), rng.below(n));
        match rng.below(5) {
            0 => w!(items, "                let p{j}: *mut u8 = {};\n                std::ptr::write(p{j}.cast::<*mut u8>(), {});", hop(x), rd(y)),
            1 => w!(items, "                std::ptr::write({}.cast::<*mut u8>(), {});", rd(x), rd(y)),
            2 => w!(items, "                std::ptr::write(&raw mut G{id}_{x}, {});", hop(y)),
            3 => w!(items, "                acc = acc.wrapping_mul(3).wrapping_add(({} == {}) as u32);", rd(x), hop(y)),
            _ => w!(items, "                std::ptr::write(&raw mut G{id}_{x}, {});", rd(y)),
        }
    }
    if early {
        w!(items, "                return acc ^ i;");
    }
    w!(items, "            }}");
    w!(items, "        }}");
    w!(items, "        acc ^ rounds");
    w!(items, "    }}");
    w!(body, "        h.put(&unsafe {{ chain{id}(bb({}u32)) }});", rng.range(1, 5));
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    for id in 0..rng.range(2, 5) {
        if rng.chance(70) {
            wide_chain(rng, &mut items, &mut body, id);
        } else {
            ptr_chain(rng, &mut items, &mut body, id);
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
