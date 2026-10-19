//! Strategy `loops`: iterator chains, slicing loops, nested counters, Vec manipulation,
//! fixed-size array kernels (auto-vectorisation candidates).
//!
//! This mirrors the shape of recent rustc reports: counters that advance in a loop and
//! are later used to slice (`buf[at..]`), nested `checked_add` loops with `step_by`,
//! `%`/`min` chains after short loops, `i128` accumulations, iterator pipelines with
//! `checked_mul`. Validity: every slice bound is clamped to the length, every arithmetic
//! step is wrapping/checked, every iterator is finite (`cycle` is always followed by `take`).

use super::arith::{gen_expr, Var};
use super::{all_ints, bits, lit};
use crate::rng::Rng;
use crate::w;

const CHAIN_TYS: &[&str] = &["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i32", "i64", "i128"];

#[derive(Clone, Copy)]
struct Flags {
    de: bool,    // DoubleEndedIterator
    exact: bool, // ExactSizeIterator
}

fn small_fn(rng: &mut Rng, ty: &'static str, x: &str) -> String {
    let b = bits(ty);
    match rng.below(8) {
        0 => format!("{x}.wrapping_mul({})", lit(rng, ty)),
        1 => format!("({x} ^ {})", lit(rng, ty)),
        2 => format!("{x}.rotate_left({})", rng.below(b as usize)),
        3 => format!("{x}.wrapping_add({})", lit(rng, ty)),
        4 => format!("{x}.wrapping_shr({})", rng.below(b as usize)),
        5 => format!("{x}.min({})", lit(rng, ty)),
        6 => format!("{x}.saturating_sub({})", lit(rng, ty)),
        _ => format!("({x} & {} | {})", lit(rng, ty), lit(rng, ty)),
    }
}

fn pred(rng: &mut Rng, ty: &'static str, x: &str) -> String {
    let op = *rng.pick(&["<", ">", "!=", "=="]);
    let b = bits(ty);
    format!("{}.rotate_left({}) {op} {}", x, rng.below(b as usize), lit(rng, ty))
}

fn source(rng: &mut Rng, ty: &'static str, with_chain: bool) -> (String, Flags) {
    let f = Flags { de: true, exact: true };
    let k = rng.range(1, 5);
    match rng.below(if with_chain { 8 } else { 7 }) {
        0 => ("v.iter().copied()".into(), f),
        1 => (format!("v.iter().enumerate().map(|(i, &x)| x.wrapping_add(i as {ty}))"), f),
        2 => (format!("(0..n).map(|i| (i as {ty}).wrapping_mul({}))", lit(rng, ty)), f),
        3 => (format!("v.chunks({k}).map(|c| c.iter().fold(0 as {ty}, |a, &b| a.wrapping_add(b)))"), f),
        4 => (format!("v.windows({k}).map(|w| w[0] ^ w[w.len() - 1])"), f),
        5 => ("v.iter().rev().copied()".into(), f),
        6 => (format!("v.iter().copied().zip(v.iter().rev().copied()).map(|(a, b)| a.wrapping_sub(b))"), f),
        _ => ("v.iter().copied().chain(v.iter().copied())".into(), Flags { de: true, exact: false }),
    }
}

fn chain(rng: &mut Rng, ty: &'static str) -> String {
    let (mut s, mut fl) = source(rng, ty, true);
    for _ in 0..rng.range(0, 5) {
        match rng.below(14) {
            0 | 1 => s += &format!(".map(|x| {})", small_fn(rng, ty, "x")),
            2 => {
                s += &format!(".filter(|&x| {})", pred(rng, ty, "x"));
                fl.exact = false;
            }
            3 => {
                s += &format!(".step_by({})", rng.range(1, 4));
                if !fl.exact {
                    fl.de = false;
                }
            }
            4 => {
                s += &format!(".skip({})", rng.range(0, 3));
                if !(fl.de && fl.exact) {
                    fl.de = false;
                }
            }
            5 => {
                s += &format!(".take({})", rng.range(1, 30));
                if !(fl.de && fl.exact) {
                    fl.de = false;
                }
            }
            6 => {
                s += &format!(".take_while(|&x| {})", pred(rng, ty, "x"));
                fl = Flags { de: false, exact: false };
            }
            7 => {
                s += &format!(".skip_while(|&x| {})", pred(rng, ty, "x"));
                fl = Flags { de: false, exact: false };
            }
            8 => {
                s += &format!(".scan({}, |st, x| {{ *st = st.wrapping_mul({}) ^ x; Some(*st) }})", lit(rng, ty), lit(rng, ty));
                fl = Flags { de: false, exact: false };
            }
            9 => {
                s += &format!(".filter_map(|x| x.checked_mul({}))", lit(rng, ty));
                fl.exact = false;
            }
            10 => {
                s += &format!(".map_while(|x| x.checked_add({}))", lit(rng, ty));
                fl = Flags { de: false, exact: false };
            }
            11 => {
                s += &format!(".flat_map(|x| [x, {}])", small_fn(rng, ty, "x"));
                fl.exact = false;
            }
            12 => {
                s += &format!(".cycle().take({})", rng.range(1, 40));
                fl = Flags { de: false, exact: false };
            }
            _ => {
                if fl.de {
                    s += ".rev()";
                } else {
                    s += ".fuse()";
                }
            }
        }
    }
    s
}

fn terminal(rng: &mut Rng, ty: &'static str, it: &str) -> String {
    match rng.below(12) {
        0 => format!("h.put(&{it}.fold({}, |a, x| a.wrapping_mul({}).wrapping_add(x)));", lit(rng, ty), lit(rng, ty)),
        1 => format!("h.put(&{it}.collect::<Vec<{ty}>>());"),
        2 => format!("h.put(&{it}.count());"),
        3 => format!("h.put(&{it}.max());"),
        4 => format!("h.put(&{it}.min());"),
        5 => format!("h.put(&{it}.last());"),
        6 => format!("h.put(&{it}.position(|x| {}));", pred(rng, ty, "x")),
        7 => format!("h.put(&{it}.any(|x| {}));", pred(rng, ty, "x")),
        8 => format!("h.put(&{it}.all(|x| {}));", pred(rng, ty, "x")),
        9 => format!("h.put(&{it}.nth({}));", rng.range(0, 6)),
        10 => format!("h.put(&{it}.try_fold(0 as {ty}, |a, x| a.checked_add(x)));"),
        _ => format!("h.put(&{it}.reduce(|a, b| (a ^ b).max(a.wrapping_sub(b))));"),
    }
}

fn data(rng: &mut Rng, ty: &'static str, s: &mut String) {
    let len = rng.range(0, 40);
    w!(s, "        let n = bb({len}usize);");
    w!(s, "        let v: Vec<{ty}> = (0..n).map(|i| (i as {ty}).wrapping_mul({}).wrapping_add({})).collect();", lit(rng, ty), lit(rng, ty));
}

/// Pipeline snippet.
fn iter_chain(rng: &mut Rng, body: &mut String) {
    let ty = *rng.pick(CHAIN_TYS);
    w!(body, "        {{");
    data(rng, ty, body);
    for _ in 0..rng.range(1, 4) {
        let c = chain(rng, ty);
        let t = terminal(rng, ty, &c);
        w!(body, "        {t}");
    }
    w!(body, "        }}");
}

/// `buf[at..]` after a loop that advances `at` (cf. rust-lang/rust#164295).
fn at_loop(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let acc_step = *rng.pick(&[
        "acc ^= x;",
        "acc = acc.wrapping_add(x << (at & 7));",
        "acc = acc.rotate_left(3) ^ (buf.get(at).copied().unwrap_or(0) as u64);",
        "acc = acc.wrapping_mul(31).wrapping_add(at as u64 ^ x);",
    ]);
    let step = *rng.pick(&["at += more as usize;", "at = (at + 2).min(buf.len());", "if more { at += 1; }"]);
    let start = rng.range(0, 3);
    let attr = *rng.pick(&["#[inline(never)]", "#[inline(always)]", ""]);
    let tail = *rng.pick(&["(acc, buf[at..].len())", "(acc, buf[..at].len() + buf[at..].len())", "(acc ^ buf[at..].iter().map(|&b| b as u64).sum::<u64>(), at)"]);
    let shape = rng.below(3);
    let looped = match shape {
        0 => format!("loop {{\n            {acc_step}\n            let more = at < buf.len();\n            {step}\n            if !more {{ break; }}\n        }}"),
        1 => format!("while at < buf.len() {{\n            {acc_step}\n            let more = true;\n            {step}\n            if at > 1000 {{ break; }}\n        }}"),
        _ => format!("for _ in 0..buf.len() + 2 {{\n            {acc_step}\n            let more = at < buf.len();\n            {step}\n        }}"),
    };
    w!(items, "    // known-bug: rust-lang/rust#164295 (LLVM 23, -O)");
    w!(items, "    {attr}\n    fn at_loop_{id}(buf: &[u8], x: u64) -> (u64, usize) {{\n        let mut at = {start}usize.min(buf.len());\n        let mut acc = 0u64;\n        {looped}\n        {tail}\n    }}");
    for _ in 0..rng.range(1, 3) {
        w!(body, "        {{ let buf = vec![{}u8; bb({}usize)]; h.put(&at_loop_{id}(&buf, bb({}u64)).0); h.put(&at_loop_{id}(&buf, bb({}u64)).1); }}", rng.below(256), rng.range(0, 9), rng.next_u64() % 1000, rng.below(5));
    }
}

/// Nested counters with `checked_*`, `step_by` and an `%`/`min` chain (cf. #164498/#164499).
fn nested_checked(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = *rng.pick(CHAIN_TYS);
    let n1 = rng.range(1, 12);
    let n2 = rng.range(1, 12);
    let step = rng.range(1, 3);
    let vars = [
        Var { name: "a".into(), ty: "usize" },
        Var { name: "b".into(), ty: "usize" },
        Var { name: "t".into(), ty },
    ];
    let depth_ = rng.range(2, 4) as u32;
    let upd = gen_expr(rng, &vars, ty, depth_);
    let op = *rng.pick(&["checked_add", "checked_mul", "checked_sub"]);
    w!(items, "    #[inline(never)]\n    fn nested_{id}(n1: usize, n2: usize) -> (Option<{ty}>, {ty}) {{");
    w!(items, "        let mut total: Option<{ty}> = Some({});", lit(rng, ty));
    w!(items, "        let mut t: {ty} = {};", lit(rng, ty));
    w!(items, "        'outer: for a in 0..n1 {{\n            for b in (0..n2).step_by({step}) {{");
    w!(items, "                t = {upd};");
    w!(items, "                total = total.and_then(|s| s.{op}((a * b) as {ty} | 1));");
    w!(items, "                if total.is_none() && (a + b) % 5 == 2 {{ break 'outer; }}\n            }}\n        }}");
    let m = rng.range(2, 1000);
    let chain = format!("t.wrapping_mul({}).wrapping_rem(({m} as {ty}) | 1)", lit(rng, ty));
    // `%`/`min` chain after the loops
    w!(items, "        let r = ({chain}).min({}).max(t.wrapping_rem(({}) | 1));", lit(rng, ty), lit(rng, ty));
    w!(items, "        (total, r)\n    }}");
    w!(body, "        {{ let (a, b) = nested_{id}(bb({n1}), bb({n2})); h.put(&a); h.put(&b); }}");
}

/// Vec manipulation sequence. Every index is reduced modulo the current length.
fn vec_ops(rng: &mut Rng, body: &mut String) {
    let ty = *rng.pick(CHAIN_TYS);
    w!(body, "        {{");
    w!(body, "        let mut v: Vec<{ty}> = (0..bb({}usize)).map(|i| (i as {ty}).wrapping_mul({})).collect();", rng.range(0, 30), lit(rng, ty));
    for _ in 0..rng.range(3, 9) {
        let op = match rng.below(14) {
            0 => format!("v.push({});", lit(rng, ty)),
            1 => "v.pop();".to_string(),
            2 => format!("if !v.is_empty() {{ let i = bb({}usize) % v.len(); v.remove(i); }}", rng.below(100)),
            3 => format!("{{ let i = bb({}usize) % (v.len() + 1); v.insert(i, {}); }}", rng.below(100), lit(rng, ty)),
            4 => format!("v.truncate(bb({}usize));", rng.range(0, 30)),
            5 => format!("v.retain(|&x| {});", pred(rng, ty, "x")),
            6 => "v.dedup();".to_string(),
            7 => format!("if !v.is_empty() {{ let k = bb({}usize) % v.len(); let d: Vec<{ty}> = v.drain(..k).collect(); h.put(&d); }}", rng.below(100)),
            8 => format!("v.sort_unstable_by_key(|&x| x.rotate_left({}));", rng.below(bits(ty) as usize)),
            9 => "if v.len() > 1 { let l = v.len(); v.swap(0, l - 1); }".to_string(),
            10 => format!("if !v.is_empty() {{ let k = bb({}usize) % v.len(); v.rotate_left(k); }}", rng.below(100)),
            11 => "v.reverse();".to_string(),
            12 => "v.sort();".to_string(),
            _ => format!("{{ let k = bb({}usize); h.put(&v.binary_search(&{}).is_ok()); h.put(&k); }}", rng.below(100), lit(rng, ty)),
        };
        w!(body, "        {op}");
    }
    w!(body, "        h.put(&v);");
    w!(body, "        }}");
}

/// Fixed-size array kernels - auto-vectorisation candidates.
fn array_kernel(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = *rng.pick(&all_ints());
    let n = *rng.pick(&[3usize, 4, 7, 8, 16, 17, 32, 33, 64]);
    let kernel = match rng.below(5) {
        0 => format!("for i in 0..{n} {{ c[i] = a[i].wrapping_mul(b[i]).wrapping_add(c[i]); }}"),
        1 => format!("for i in 0..{n} {{ c[i] = (a[i] ^ b[({n} - 1) - i]).rotate_left(i as u32); }}"),
        2 => format!("for (x, (y, z)) in c.iter_mut().zip(a.iter().zip(b.iter())) {{ *x = (*y).max(*z).wrapping_sub(*x); }}"),
        3 => format!("for i in 1..{n} {{ c[i] = c[i - 1].wrapping_add(a[i] & b[i]); }}"),
        _ => format!("let mut s: {ty} = 0; for i in 0..{n} {{ s = s.wrapping_add(a[i].wrapping_mul(b[i])); c[i] = s; }}"),
    };
    w!(items, "    #[inline(never)]\n    fn kernel_{id}(a: &[{ty}; {n}], b: &[{ty}; {n}], c: &mut [{ty}; {n}]) {{\n        {kernel}\n    }}");
    w!(body, "        {{ let a: [{ty}; {n}] = std::array::from_fn(|i| (i as {ty}).wrapping_mul(bb({})).wrapping_add({}));", lit(rng, ty), lit(rng, ty));
    w!(body, "        let b: [{ty}; {n}] = a.map(|x| x.wrapping_mul(bb({})) ^ {});", lit(rng, ty), lit(rng, ty));
    w!(body, "        let mut c = [bb({}); {n}]; kernel_{id}(&a, &b, &mut c); h.put(&c); }}", lit(rng, ty));
}

/// Accumulator loop with a `match k % M`, `is_power_of_two`, `rem_euclid` and `matches!` ranges,
/// the shape of rust-lang/rust#164298 (i128 accumulator crashing LLVM with AVX2 / x86-64-v3).
fn score_loop(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let acc = *rng.pick(&["i128", "u128", "i64", "u64", "isize", "i32"]);
    let m = rng.range(3, 9);
    let (r1, r2) = (rng.below(m), rng.below(m));
    let a = rng.below(30);
    let b = a + rng.below(20);
    let init = if acc.starts_with('i') { "-1" } else { "1" };
    let c = rng.below(60);
    w!(items, "    #[inline(never)]\n    fn sc{id}(count: u16, seed: u8, q: i8) -> {acc} {{");
    w!(items, "        let mut score: {acc} = {init};");
    w!(items, "        for k in 0..(count as usize).min({}) {{", rng.range(8, 24));
    w!(items, "            match k % {m} {{");
    w!(items, "                {r1} => {{ if (k as u16).is_power_of_two() {{ match (q.rem_euclid({}), matches!(count, {a}..={b} | {c})) {{ (0, true) => {{}} _ => score = score.wrapping_add(k as {acc}).wrapping_add((q as i8 as {acc})) }} }} }}", rng.range(2, 5));
    w!(items, "                {r2} if r_ok{id}(seed, k) => score = score.wrapping_mul(3).wrapping_sub(seed as {acc}),");
    w!(items, "                _ => score = score.wrapping_add(seed as {acc} + {}),", rng.below(2000));
    w!(items, "            }}");
    w!(items, "        }}");
    w!(items, "        score\n    }}");
    w!(items, "    fn r_ok{id}(seed: u8, k: usize) -> bool {{ (seed as usize + k) % 3 != 1 }}");
    w!(body, "        for count in [0u16, 1, 5, 9, 17, 19, 21, 26, 27, 100] {{ for (seed, q) in [(0u8, 0i8), (7, -3), (255, 127), (128, -128)] {{ h.put(&sc{id}(bb(count), bb(seed), bb(q))); }} }}");
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    let parts = rng.range(2, 5);
    for id in 0..parts {
        match rng.below(12) {
            10 | 11 => score_loop(rng, &mut items, &mut body, id),
            0..=3 => iter_chain(rng, &mut body),
            4 | 5 => at_loop(rng, &mut items, &mut body, id),
            6 | 7 => nested_checked(rng, &mut items, &mut body, id),
            8 => vec_ops(rng, &mut body),
            _ => array_kernel(rng, &mut items, &mut body, id),
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
