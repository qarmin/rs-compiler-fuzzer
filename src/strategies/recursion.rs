//! Strategy `recursion`: recursive functions over boxed enums that combine the recursive result
//! with `max`/`min`/arithmetic *after* the call, plus accumulator ("tail") recursion and mutual
//! recursion. Targets LLVM's TailCallElim and inlining of recursive calls
//! (rust-lang/rust#164469: `std::cmp::max(K::B, e.kind())` on a derive(Ord) enum returned `B` for `A`),
//! and MIR inlining / GVN around recursive functions.
//!
//! Safe code only. Recursion depth is bounded (trees of depth <= 8, accumulators <= 200).

use crate::rng::Rng;
use crate::w;

fn tree(rng: &mut Rng, id: usize, nk: usize, depth: u32) -> String {
    if depth == 0 || rng.chance(15) {
        return if rng.chance(70) { format!("E{id}::Leaf(K{id}::V{})", rng.below(nk)) } else { format!("E{id}::Num({})", rng.below(50) as i32 - 20) };
    }
    match rng.below(4) {
        0 | 1 => format!("E{id}::Wrap(Box::new({}))", tree(rng, id, nk, depth - 1)),
        2 => format!("E{id}::Pair(Box::new({}), Box::new({}))", tree(rng, id, nk, depth - 1), tree(rng, id, nk, depth - 1)),
        _ => format!("E{id}::Opt(Some(Box::new({})))", tree(rng, id, nk, depth - 1)),
    }
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    for id in 0..rng.range(1, 3) {
        let nk = rng.range(3, 5);
        let vs: Vec<String> = (0..nk).map(|i| format!("V{i}")).collect();
        w!(items, "    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)] enum K{id} {{ {} }}", vs.join(", "));
        w!(items, "    enum E{id} {{ Leaf(K{id}), Num(i32), Wrap(Box<E{id}>), Pair(Box<E{id}>, Box<E{id}>), Opt(Option<Box<E{id}>>) }}");
        let mid = rng.below(nk);
        let lo = rng.below(nk);
        let comb = |rng: &mut Rng, x: &str| -> String {
            match rng.below(6) {
                0 => format!("std::cmp::max(K{id}::V{mid}, {x})"),
                1 => format!("std::cmp::min(K{id}::V{mid}, {x})"),
                2 => format!("{x}.max(K{id}::V{lo}).min(K{id}::V{mid})"),
                3 => format!("if {x} > K{id}::V{mid} {{ K{id}::V{lo} }} else {{ {x} }}"),
                4 => format!("{{ let k = {x}; if k == K{id}::V{lo} {{ K{id}::V{mid} }} else {{ k }} }}"),
                _ => format!("Ord::max({x}, K{id}::V{lo})"),
            }
        };
        let (c1, c2, c3) = (comb(rng, "e.kind()"), comb(rng, "a.kind()"), comb(rng, "b.kind()"));
        w!(items, "    impl E{id} {{");
        w!(items, "        pub fn kind(&self) -> K{id} {{ match self {{ E{id}::Leaf(k) => *k, E{id}::Num(n) => if *n < 0 {{ K{id}::V{lo} }} else {{ K{id}::V{mid} }}, E{id}::Wrap(e) => {c1}, E{id}::Pair(a, b) => std::cmp::max({c2}, {c3}), E{id}::Opt(None) => K{id}::V0, E{id}::Opt(Some(e)) => {c1} }} }}");
        let (acc_a, acc_b) = (rng.below(7) as i32 + 1, rng.below(100) as i32);
        w!(items, "        pub fn weigh(&self) -> i64 {{ match self {{ E{id}::Leaf(k) => *k as i64, E{id}::Num(n) => *n as i64, E{id}::Wrap(e) => e.weigh().wrapping_mul({acc_a}) ^ {acc_b}, E{id}::Pair(a, b) => a.weigh().wrapping_add(b.weigh() << 1), E{id}::Opt(o) => o.as_ref().map_or(-1, |e| e.weigh() + 1) }} }}");
        w!(items, "        pub fn depth(&self) -> u32 {{ match self {{ E{id}::Wrap(e) => 1 + e.depth(), E{id}::Pair(a, b) => 1 + a.depth().max(b.depth()), E{id}::Opt(Some(e)) => 1 + e.depth(), _ => 0 }} }}");
        w!(items, "    }}");
        // post-processing of the recursive result, as in the issue
        let (m1, m2) = (rng.below(nk), rng.below(nk));
        w!(items, "    pub fn f{id}(e: &E{id}) -> K{id} {{ match e.kind() {{ K{id}::V{m1} => K{id}::V{m2}, k => k }} }}");
        // accumulator recursion + mutual recursion
        w!(items, "    #[inline(never)]\n    fn acc{id}(n: u32, acc: u64) -> u64 {{ if n == 0 {{ acc }} else {{ acc{id}(n - 1, acc.wrapping_mul(31) ^ (n as u64).rotate_left({})) }} }}", rng.below(60));
        w!(items, "    fn even{id}(n: u32, s: u32) -> u32 {{ if n == 0 {{ s }} else {{ odd{id}(n - 1, s.wrapping_add(n)) }} }}");
        w!(items, "    fn odd{id}(n: u32, s: u32) -> u32 {{ if n == 0 {{ s ^ 1 }} else {{ even{id}(n - 1, s.wrapping_mul(3)) }} }}");
        w!(items, "    #[inline(never)]\n    fn build{id}(depth: u32, seed: u32) -> E{id} {{ if depth == 0 {{ return E{id}::Leaf(match seed % {nk} {{ {} _ => K{id}::V0 }}); }} match seed % 4 {{ 0 => E{id}::Wrap(Box::new(build{id}(depth - 1, seed.wrapping_mul(7).wrapping_add(1)))), 1 => E{id}::Pair(Box::new(build{id}(depth - 1, seed ^ 5)), Box::new(build{id}(depth - 1, seed.wrapping_add(3)))), 2 => E{id}::Num(seed as i32 % 9 - 4), _ => E{id}::Opt(if seed & 8 == 0 {{ None }} else {{ Some(Box::new(build{id}(depth - 1, seed >> 1))) }}) }} }}",
            (0..nk).map(|i| format!("{i} => K{id}::V{i},")).collect::<Vec<_>>().join(" "));
        for _ in 0..rng.range(2, 4) {
            let d = rng.range(2, 6) as u32;
            let t = tree(rng, id, nk, d);
            w!(body, "        {{ let e = {t}; h.put(&(f{id}(&e) as u8)); h.put(&(e.kind() as u8)); h.put(&e.weigh()); h.put(&e.depth()); }}");
        }
        w!(body, "        for s in [0u32, 1, 2, 3, 5, 8, 13, 21, bb({})] {{ let e = build{id}(bb({}u32), s); h.put(&(f{id}(&e) as u8)); h.put(&e.weigh()); }}", rng.below(1000), rng.range(1, 7));
        w!(body, "        h.put(&acc{id}(bb({}u32), bb({}u64))); h.put(&even{id}(bb({}u32), bb(1u32)));", rng.range(0, 200), rng.next_u64() % 1000, rng.range(0, 150));
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
