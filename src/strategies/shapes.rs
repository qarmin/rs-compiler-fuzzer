//! Strategy `shapes`: programs built after specific rustc miscompilation reports that no other strategy covered.
//!
//! - `wide_chain` (rust-lang/rust#164667): chains of shadowed `let`s over 128-bit (and narrower) integers whose seeds go through
//!   `black_box`, mixing `rotate_*`, `checked_mul(..).unwrap_or(..)`, `abs_diff`, casts between widths. The report is a wrong
//!   `abs_diff` result after LLVM's stack colouring merged stack slots.
//! - `ptr_chain` (rust-lang/rust#164137): `static mut` raw pointers that all point at one another; reads and writes through
//!   chains of them in a loop with an early `return` (early-cse treated the function as unreachable).
//! - `interval_search` (rust-lang/rust#165528): an enum of four interval kinds compared through a tuple `match` of two `Ordering`s,
//!   searched linearly with early returns (LLVM 23 DFAJumpThreading threaded the state machine wrongly).
//! - `range_zip` (rust-lang/rust#165551): `(lo..lo + n).zip(hi - n..hi)` over a narrow signed index type that is cast to `usize`
//!   (instruction selection could not select a sign-extended vector add with `target-cpu=x86-64-v4` and fat LTO).
//! - `masked_fold` (rust-lang/rust#165544): a boolean accumulated over `iter_mut().zip(mask)` while the same loop rewrites the elements
//!   (LLVM 23 vectorised it as a find-last reduction and returned a stale lane; needs `x86-64-v3`).
//!
//! Validity: integer code is wrapping/checked/total only. In `ptr_chain` every static is first set to the address of the first
//! static, so every pointer read back is that valid, aligned address; writes only ever store such a pointer.

use super::{bits, lit_plain, signed};
use crate::rng::Rng;
use crate::w;

const TYS: &[&str] = &[
    "i128", "u128", "i128", "u128", "i64", "u64", "u32", "i32", "u16", "i16", "u8", "i8", "usize",
];

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
        w!(
            body,
            "        let s{id}_{n}: {ty} = bb({});",
            lit_plain(rng, ty)
        );
        vars.push((format!("s{id}_{n}"), ty));
    }
    for step in 0..rng.range(3, 9) {
        let (a, ta) = rng.pick(&vars).clone();
        let (b, tb) = rng.pick(&vars).clone();
        // shadow an existing name half of the time: the report relies on it, and it keeps the same stack slot candidates alive
        let name = if rng.chance(50) {
            rng.pick(&vars).0.clone()
        } else {
            format!("t{id}_{step}")
        };
        let new_ty;
        let expr = match rng.below(12) {
            0 => {
                new_ty = ta;
                format!(
                    "({}).rotate_right({}).checked_mul({a}).unwrap_or(0)",
                    lit_plain(rng, ta),
                    rng.below(bits(ta) as usize)
                )
            }
            1 => {
                new_ty = ta;
                format!(
                    "{a}.rotate_left({}).checked_add({b} as {ta}).unwrap_or({})",
                    rng.below(bits(ta) as usize),
                    lit_plain(rng, ta)
                )
            }
            2 => {
                let u = unsigned_of(ta);
                new_ty = if signed(ta) {
                    Box::leak(u.clone().into_boxed_str())
                } else {
                    ta
                };
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
                format!(
                    "{a}.swap_bytes().rotate_right({})",
                    rng.below(bits(ta) as usize)
                )
            }
            7 => {
                new_ty = tb;
                format!(
                    "({a} as {tb}).wrapping_mul({b}).rotate_left({})",
                    rng.below(bits(tb) as usize)
                )
            }
            8 => {
                new_ty = ta;
                format!("{a}.checked_div(({b} as {ta}) | 1).unwrap_or({a})")
            }
            9 => {
                new_ty = ta;
                format!(
                    "{a}.saturating_sub({b} as {ta}).max({}).min({a})",
                    lit_plain(rng, ta)
                )
            }
            10 => {
                new_ty = "u128";
                format!(
                    "({a} as u128) << {} | ({b} as u128) >> {}",
                    rng.below(128),
                    rng.below(128)
                )
            }
            _ => {
                new_ty = ta;
                format!(
                    "{a}.wrapping_shl({}) ^ ({b} as {ta}).reverse_bits()",
                    rng.below(200)
                )
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
    w!(
        items,
        "    // shape of rust-lang/rust#164137 (early-cse, opt-level=1)"
    );
    for k in 0..n {
        w!(
            items,
            "    static mut G{id}_{k}: *mut u8 = std::ptr::null_mut::<u8>();"
        );
    }
    let rd = |k: usize| format!("std::ptr::read::<*mut u8>(&raw const G{id}_{k})");
    let hop = |k: usize| format!("std::ptr::read({}.cast::<*mut u8>())", rd(k));
    w!(
        items,
        "    #[inline(never)]\n    unsafe fn chain{id}(rounds: u32) -> u32 {{"
    );
    w!(items, "        let mut acc = 0u32;");
    w!(items, "        for i in 0..rounds {{");
    w!(items, "            if i == 0 {{");
    w!(
        items,
        "                std::ptr::write(&raw mut G{id}_0, (&raw mut G{id}_0).cast::<u8>());"
    );
    for k in 1..n {
        w!(
            items,
            "                std::ptr::write(&raw mut G{id}_{k}, {});",
            hop(rng.below(k))
        );
    }
    w!(items, "            }} else {{");
    let early = rng.chance(60);
    for j in 0..rng.range(3, 8) {
        let (x, y) = (rng.below(n), rng.below(n));
        match rng.below(5) {
            0 => w!(
                items,
                "                let p{j}: *mut u8 = {};\n                std::ptr::write(p{j}.cast::<*mut u8>(), {});",
                hop(x),
                rd(y)
            ),
            1 => w!(
                items,
                "                std::ptr::write({}.cast::<*mut u8>(), {});",
                rd(x),
                rd(y)
            ),
            2 => w!(
                items,
                "                std::ptr::write(&raw mut G{id}_{x}, {});",
                hop(y)
            ),
            3 => w!(
                items,
                "                acc = acc.wrapping_mul(3).wrapping_add(({} == {}) as u32);",
                rd(x),
                hop(y)
            ),
            _ => w!(
                items,
                "                std::ptr::write(&raw mut G{id}_{x}, {});",
                rd(y)
            ),
        }
    }
    if early {
        w!(items, "                return acc ^ i;");
    }
    w!(items, "            }}");
    w!(items, "        }}");
    w!(items, "        acc ^ rounds");
    w!(items, "    }}");
    w!(
        body,
        "        h.put(&unsafe {{ chain{id}(bb({}u32)) }});",
        rng.range(1, 5)
    );
}

fn interval_search(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = *rng.pick(&["u8", "u8", "u16", "u32", "i8", "i32"]);
    let o = "std::cmp::Ordering";
    let flags: Vec<bool> = (0..4).map(|_| rng.chance(50)).collect();
    let names = ["A", "B", "C", "D"];
    let ord = |rng: &mut Rng| *rng.pick(&["Less", "Greater", "Equal"]);
    let (r_le, r_l, r_e, r_g) = (ord(rng), ord(rng), ord(rng), ord(rng));
    let (inc_a, inc_b) = (*rng.pick(&["Equal", "Less"]), *rng.pick(&["Equal", "Greater"]));
    let (on_greater, on_equal, on_less) = (
        *rng.pick(&["", "", "n += 1;"]),
        *rng.pick(&["return (true, n);", "return (n == 0, n);"]),
        *rng.pick(&["return (false, n);", "return (n > 0, 1);"]),
    );
    w!(items, "    // known-bug: rust-lang/rust#165528 (DFAJumpThreading)");
    w!(items, "    #[allow(dead_code)]");
    w!(items, "    enum Iv{id} {{ A({ty}, {ty}), B({ty}, {ty}), C({ty}, {ty}), D({ty}, {ty}) }}");
    w!(items, "    impl Iv{id} {{");
    w!(items, "        fn lower(&self) -> {ty} {{ match self {{ Iv{id}::A(l, _) | Iv{id}::B(l, _) | Iv{id}::C(l, _) | Iv{id}::D(l, _) => *l }} }}");
    let arms: Vec<String> = names
        .iter()
        .zip(&flags)
        .map(|(n, f)| format!("Iv{id}::{n}(_, h) => (*h, {f})"))
        .collect();
    w!(items, "        fn upper(&self) -> ({ty}, bool) {{ match self {{ {} }} }}", arms.join(", "));
    w!(items, "        fn cmp(&self, other: &Iv{id}) -> {o} {{");
    w!(items, "            let (su, si) = self.upper();");
    w!(items, "            let (ou, oi) = other.upper();");
    w!(items, "            match (self.lower().cmp(&ou), su.cmp(&other.lower())) {{");
    w!(items, "                ({o}::Less, {o}::Equal) => if si {{ {o}::{inc_a} }} else {{ {o}::{r_le} }},");
    w!(items, "                ({o}::Less, _) => {o}::{r_l},");
    w!(items, "                ({o}::Equal, _) => if oi {{ {o}::{inc_b} }} else {{ {o}::{r_e} }},");
    w!(items, "                ({o}::Greater, _) => {o}::{r_g},");
    w!(items, "            }}");
    w!(items, "        }}");
    w!(items, "    }}");
    w!(items, "    #[inline(never)]");
    w!(items, "    fn search{id}(items: &[Iv{id}], key: &Iv{id}) -> (bool, u32) {{");
    w!(items, "        let mut n = 0u32;");
    w!(items, "        for item in items {{");
    w!(items, "            match key.cmp(item) {{");
    w!(items, "                {o}::Greater => {{ {on_greater} }}");
    w!(items, "                {o}::Equal => {{ {on_equal} }}");
    w!(items, "                {o}::Less => {{ {on_less} }}");
    w!(items, "            }}");
    w!(items, "        }}");
    w!(items, "        (false, n)");
    w!(items, "    }}");
    for _ in 0..rng.range(3, 7) {
        let lo = rng.below(5) as u8;
        let hi = lo + rng.below(3) as u8;
        let klo = rng.below(5) as u8;
        let khi = klo + rng.below(3) as u8;
        let item = names[rng.below(4)];
        let key = names[rng.below(4)];
        w!(
            body,
            "        h.put(&search{id}(bb(&[Iv{id}::{item}({lo}, {hi}), Iv{id}::{item}({hi}, {hi})]), bb(&Iv{id}::{key}({klo}, {khi}))));"
        );
    }
}

fn range_zip(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ity = *rng.pick(&["i32", "i32", "i32", "u32", "i16", "i8", "i64"]);
    let ety = *rng.pick(&["u32", "u32", "u64", "u8", "i32"]);
    let op = match rng.below(4) {
        0 | 1 => "v.swap(a as usize, b as usize);".to_string(),
        2 => "let t = v[a as usize]; v[a as usize] = v[b as usize]; v[b as usize] = t;".to_string(),
        _ => "v[a as usize] = v[a as usize].wrapping_add(v[b as usize]);".to_string(),
    };
    let zip = match rng.below(3) {
        0 | 1 => "(lo..lo + n).zip(hi - n..hi)",
        _ => "(lo..lo + n).zip((hi - n..hi).rev())",
    };
    w!(items, "    // shape of rust-lang/rust#165551 (instruction selection of a vector sign extension)");
    w!(items, "    #[inline(never)]");
    w!(items, "    fn swap_ranges{id}(v: &mut [{ety}], lo: {ity}, hi: {ity}, n: {ity}) {{");
    w!(items, "        for (a, b) in {zip} {{ {op} }}");
    w!(items, "    }}");
    for _ in 0..rng.range(1, 3) {
        let n = rng.range(2, 20);
        let lo = rng.below(10);
        let hi = lo + n + rng.below(20);
        w!(
            body,
            "        {{ let mut v: Vec<{ety}> = (0..64).collect(); let (lo, hi, n) = bb(({lo}{ity}, {hi}{ity}, {n}{ity})); swap_ranges{id}(&mut v, lo, hi, n); h.put(&v); }}"
        );
    }
}

fn masked_fold(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let fty = *rng.pick(&["f32", "f64"]);
    let pred = match rng.below(4) {
        0 | 1 => "v.is_finite()".to_string(),
        2 => "*v >= 0.0".to_string(),
        _ => format!("*v < {}.5", rng.below(9)),
    };
    let (init, update, reset) = if rng.chance(70) {
        ("true", format!("valid &= !*on || {pred};"), format!("!*on || !({pred})"))
    } else {
        ("false", format!("valid |= *on && {pred};"), format!("*on && {pred}"))
    };
    let fill = *rng.pick(&["0.0", "1.5"]);
    w!(items, "    // known-bug: rust-lang/rust#165544 (LLVM 23 loop-vectorize find-last reduction)");
    w!(items, "    #[inline(never)]");
    w!(items, "    fn fold{id}(out: &mut [{fty}], mask: &[bool]) -> bool {{");
    w!(items, "        let mut valid = {init};");
    w!(items, "        for (v, on) in out.iter_mut().zip(mask) {{");
    w!(items, "            {update}");
    w!(items, "            if {reset} {{ *v = {fill}; }}");
    w!(items, "        }}");
    w!(items, "        valid");
    w!(items, "    }}");
    for _ in 0..rng.range(1, 3) {
        let n = rng.range(8, 70);
        let (k, r) = (rng.range(2, 5), rng.below(3));
        let special = rng.range(3, 40);
        let sp = *rng.pick(&["f64::NAN", "f64::INFINITY", "-1.25", "7.5"]);
        w!(
            body,
            "        {{ let mask: Vec<bool> = (0..{n}).map(|i| i % {k} != {r}).collect(); let mut out: Vec<{fty}> = bb((0..{n}).map(|i| if i % {special} == {special} - 1 {{ {sp} as {fty} }} else {{ 0.75 + (i % 3) as {fty} }}).collect()); let ok = fold{id}(&mut out, &mask); h.put(&ok); h.put(&out.iter().map(|x| x.to_bits() as u64).collect::<Vec<u64>>()); }}"
        );
    }
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    for id in 0..rng.range(2, 5) {
        match rng.below(10) {
            0..=3 => wide_chain(rng, &mut items, &mut body, id),
            4..=5 => ptr_chain(rng, &mut items, &mut body, id),
            6 => interval_search(rng, &mut items, &mut body, id),
            7 => masked_fold(rng, &mut items, &mut body, id),
            _ => range_zip(rng, &mut items, &mut body, id),
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
