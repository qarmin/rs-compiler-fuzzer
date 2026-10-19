//! Strategy `enums`: pattern matching over enums with data, tuple scrutinees, guards,
//! ranges, `@` bindings, or-patterns and slice patterns. Targets MIR match-tree
//! optimisations (EarlyOtherwiseBranch, SimplifyCfg, MatchBranchSimplification, jump
//! threading), e.g. rust-lang/rust#159618.
//!
//! Safe code only. Every `match` ends in a `_` arm so it is exhaustive; arm bodies only use
//! bindings that every alternative of the arm's pattern provides (or-patterns bind nothing).

use crate::rng::Rng;
use crate::w;

#[derive(Clone, Copy, PartialEq)]
enum Ft {
    U8,
    I16,
    U32,
    Bool,
    OptU8,
}

impl Ft {
    fn name(self) -> &'static str {
        match self {
            Ft::U8 => "u8",
            Ft::I16 => "i16",
            Ft::U32 => "u32",
            Ft::Bool => "bool",
            Ft::OptU8 => "Option<u8>",
        }
    }
}

struct Variant {
    name: String,
    kind: u8, // 0 unit, 1 tuple, 2 struct
    fields: Vec<(String, Ft)>,
}

struct En {
    name: String,
    variants: Vec<Variant>,
}

fn rand_ft(rng: &mut Rng) -> Ft {
    *rng.pick(&[Ft::U8, Ft::I16, Ft::U32, Ft::Bool, Ft::OptU8])
}

fn make_enum(rng: &mut Rng, name: String) -> En {
    let n = rng.range(3, 6);
    let variants = (0..n)
        .map(|i| {
            let kind = if i == 0 { 0 } else { rng.range(0, 2) as u8 };
            let nf = if kind == 0 { 0 } else { rng.range(1, 3) };
            let fields = (0..nf).map(|j| (if kind == 2 { format!("f{j}") } else { format!("{j}") }, rand_ft(rng))).collect();
            Variant { name: format!("V{i}"), kind: kind as u8, fields }
        })
        .collect();
    En { name, variants }
}

fn decl(e: &En) -> String {
    let vs: Vec<String> = e
        .variants
        .iter()
        .map(|v| match v.kind {
            0 => v.name.clone(),
            1 => format!("{}({})", v.name, v.fields.iter().map(|f| f.1.name()).collect::<Vec<_>>().join(", ")),
            _ => format!("{} {{ {} }}", v.name, v.fields.iter().map(|f| format!("{}: {}", f.0, f.1.name())).collect::<Vec<_>>().join(", ")),
        })
        .collect();
    format!("    #[derive(Clone, Copy, Debug, PartialEq)] enum {} {{ {} }}", e.name, vs.join(", "))
}

fn val(rng: &mut Rng, t: Ft, seed: &str) -> String {
    // value expression derived from the runtime seed `v: u32` so constructors are not constants
    let k = rng.below(5);
    match t {
        Ft::U8 => format!("({seed}.wrapping_shr({}) as u8)", k * 3),
        Ft::I16 => format!("({seed}.wrapping_shr({}) as i16)", k * 2),
        Ft::U32 => format!("{seed}.wrapping_mul({})", rng.below(7) + 1),
        Ft::Bool => format!("(({seed} >> {}) & 1 == 1)", k),
        Ft::OptU8 => format!("(if ({seed} >> {}) & 3 == 0 {{ None }} else {{ Some(({seed} >> {}) as u8) }})", k, k + 1),
    }
}

/// `fn mk(sel: u32, v: u32) -> Enum`
fn ctor(rng: &mut Rng, e: &En) -> String {
    let mut arms = String::new();
    for (i, v) in e.variants.iter().enumerate() {
        let body = match v.kind {
            0 => format!("{}::{}", e.name, v.name),
            1 => format!("{}::{}({})", e.name, v.name, v.fields.iter().map(|f| val(rng, f.1, "v")).collect::<Vec<_>>().join(", ")),
            _ => format!("{}::{} {{ {} }}", e.name, v.name, v.fields.iter().map(|f| format!("{}: {}", f.0, val(rng, f.1, "v"))).collect::<Vec<_>>().join(", ")),
        };
        arms += &format!("{i} => {body}, ");
    }
    format!("    fn mk_{0}(sel: u32, v: u32) -> {0} {{ match sel % {1} {{ {arms} _ => unreachable!() }} }}", e.name, e.variants.len())
}

type Binds = Vec<(String, Ft)>;

fn lit_pat(rng: &mut Rng, t: Ft) -> String {
    match t {
        Ft::U8 => match rng.below(3) {
            0 => format!("{}", rng.below(256)),
            1 => { let a = rng.below(200); format!("{}..={}", a, a + rng.below(56)) }
            _ => format!("..={}", rng.below(256)),
        },
        Ft::I16 => match rng.below(3) {
            0 => format!("{}", rng.below(100) as i32 - 50),
            1 => format!("{}..=0", -(rng.below(300) as i32)),
            _ => format!("1..={}", rng.below(300) + 1),
        },
        Ft::U32 => match rng.below(2) {
            0 => format!("{}", rng.below(20)),
            _ => format!("{}..", rng.below(100)),
        },
        Ft::Bool => (if rng.chance(50) { "true" } else { "false" }).into(),
        Ft::OptU8 => match rng.below(3) {
            0 => "None".into(),
            1 => format!("Some({})", lit_pat(rng, Ft::U8)),
            _ => "Some(_)".into(),
        },
    }
}

fn field_pat(rng: &mut Rng, t: Ft, bind: bool, idx: &mut usize, binds: &mut Binds) -> String {
    match rng.below(if bind { 5 } else { 3 }) {
        0 => "_".into(),
        1 | 2 => lit_pat(rng, t),
        3 => {
            *idx += 1;
            let n = format!("b{idx}");
            binds.push((n.clone(), t));
            n
        }
        _ => {
            if matches!(t, Ft::U8 | Ft::I16 | Ft::U32) {
                *idx += 1;
                let n = format!("b{idx}");
                binds.push((n.clone(), t));
                format!("{n} @ {}", lit_pat(rng, t))
            } else {
                "_".into()
            }
        }
    }
}

fn enum_pat(rng: &mut Rng, e: &En, bind: bool, idx: &mut usize, binds: &mut Binds) -> String {
    if rng.chance(15) {
        return "_".into();
    }
    let v = rng.pick(&e.variants);
    let pats: Vec<(String, Ft)> = v.fields.iter().map(|f| (f.0.clone(), f.1)).collect();
    match v.kind {
        0 => format!("{}::{}", e.name, v.name),
        1 => format!("{}::{}({})", e.name, v.name, pats.iter().map(|(_, t)| field_pat(rng, *t, bind, idx, binds)).collect::<Vec<_>>().join(", ")),
        _ => {
            let rest = if rng.chance(30) { ", .." } else { "" };
            let sel: Vec<&(String, Ft)> = if rest.is_empty() { pats.iter().collect() } else { pats.iter().take(1).collect() };
            format!("{}::{} {{ {}{rest} }}", e.name, v.name, sel.iter().map(|(n, t)| format!("{n}: {}", field_pat(rng, *t, bind, idx, binds))).collect::<Vec<_>>().join(", "))
        }
    }
}

fn body_expr(rng: &mut Rng, binds: &Binds) -> String {
    let mut e = format!("{}u32", rng.below(1000));
    for (n, t) in binds {
        let term = match t {
            Ft::OptU8 => format!("{n}.map(|q| q as u32).unwrap_or(77)"),
            Ft::Bool => format!("({n} as u32)"),
            _ => format!("({n} as u32)"),
        };
        e = match rng.below(3) {
            0 => format!("{e}.wrapping_add({term})"),
            1 => format!("{e}.wrapping_mul({term} | 1)"),
            _ => format!("({e} ^ {term})"),
        };
    }
    e
}

fn matcher(rng: &mut Rng, enums: &[En], id: usize) -> (String, Vec<usize>) {
    let arity = rng.range(1, 3).min(enums.len().max(1) + 1);
    let picks: Vec<usize> = (0..arity).map(|_| rng.below(enums.len())).collect();
    let params: Vec<String> = picks.iter().enumerate().map(|(i, &k)| format!("a{i}: {}", enums[k].name)).collect();
    let scrut = if arity == 1 { "a0".to_string() } else { format!("({})", (0..arity).map(|i| format!("a{i}")).collect::<Vec<_>>().join(", ")) };
    let mut s = format!("    #[inline(never)]\n    fn m{id}({}) -> u32 {{\n        match {scrut} {{\n", params.join(", "));
    for _ in 0..rng.range(3, 8) {
        let mut binds = Vec::new();
        let mut idx = 0;
        let alts = if rng.chance(25) { 2 } else { 1 };
        let bind = alts == 1;
        let pats: Vec<String> = (0..alts)
            .map(|_| {
                let ps: Vec<String> = picks.iter().map(|&k| enum_pat(rng, &enums[k], bind, &mut idx, &mut binds)).collect();
                if arity == 1 { ps[0].clone() } else { format!("({})", ps.join(", ")) }
            })
            .collect();
        let guard = if !binds.is_empty() && rng.chance(35) {
            let (n, t) = &binds[0];
            match t {
                Ft::OptU8 => format!(" if {n}.is_some()"),
                Ft::Bool => format!(" if {n}"),
                _ => format!(" if ({n} as u32) % {} != {}", rng.range(2, 5), rng.below(2)),
            }
        } else {
            String::new()
        };
        s += &format!("            {}{guard} => {{ {} }}\n", pats.join(" | "), body_expr(rng, &binds));
    }
    s += &format!("            _ => {},\n        }}\n    }}\n", rng.below(100));
    (s, picks)
}

fn slice_matcher(rng: &mut Rng, id: usize) -> String {
    let mut s = format!("    #[inline(never)]\n    fn sm{id}(s: &[u8], k: u8) -> u32 {{\n        match s {{\n");
    let pool = [
        "[] => 1".to_string(),
        format!("[x] if *x > k => *x as u32 + {}", rng.below(50)),
        "[x] => *x as u32".to_string(),
        "[x, y] => (*x as u32) << 8 | *y as u32".to_string(),
        format!("[first, .., last] if first > last => (*first - *last) as u32 + {}", rng.below(9)),
        "[first, rest @ ..] if rest.len() > 2 => *first as u32 + rest.len() as u32".to_string(),
        "[.., a, b] if a == b => 4000 + *a as u32".to_string(),
        "[first, .., last] => first.wrapping_add(*last) as u32".to_string(),
    ];
    let mut order: Vec<usize> = (0..pool.len()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i + 1));
    }
    for &i in order.iter().take(rng.range(3, pool.len() - 1)) {
        s += &format!("            {},\n", pool[i]);
    }
    s += "            _ => 0xFFFF,\n        }\n    }\n";
    s
}

/// Enums whose variants share one payload type, remapped/rebuilt by `match`
/// (rust-lang/rust#164069 GVN unifying aggregates, #164112/#164135 MatchBranchSimplification and
/// EarlyOtherwiseBranch widening the read of the scrutinee).
fn rebuild(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let payload = *rng.pick(&["u8", "u16", "u32", "i64", "[u8; 24]", "[u8; 64]", "(u8, u16)"]);
    let nv = rng.range(2, 4);
    let repr = *rng.pick(&["", "#[repr(u8)] ", "#[repr(C)] ", "#[repr(u32)] ", "#[repr(C, u8)] "]);
    let vs: Vec<String> = (0..nv).map(|i| format!("V{i}({payload})")).collect();
    w!(items, "    {repr}#[derive(Clone, Copy, PartialEq, Debug)] enum R{id} {{ {}, Unit }}", vs.join(", "));
    let val = |rng: &mut Rng| -> String {
        match payload {
            "u8" => format!("{}", rng.below(256)),
            "u16" => format!("{}", rng.below(65536)),
            "u32" => format!("{}", rng.next_u64() as u32),
            "i64" => format!("{}", rng.next_u64() as i64),
            "[u8; 24]" => format!("[{}; 24]", rng.below(256)),
            "[u8; 64]" => format!("[{}; 64]", rng.below(256)),
            _ => format!("({}, {})", rng.below(256), rng.below(65536)),
        }
    };
    let k = rng.below(nv);
    // remap every payload-carrying variant to variant `k`, keeping the payload
    let arms: Vec<String> = (0..nv).map(|i| format!("R{id}::V{i}(x)")).collect();
    w!(items, "    #[inline(never)]\n    fn to_k{id}(e: R{id}) -> R{id} {{ match e {{ {} => R{id}::V{k}(x), R{id}::Unit => R{id}::Unit }} }}", arms.join(" | "));
    // rebuild identity
    let rb: Vec<String> = (0..nv).map(|i| format!("R{id}::V{i}(x) => R{id}::V{i}(x)")).collect();
    w!(items, "    #[inline(never)]\n    fn same{id}(e: R{id}) -> R{id} {{ match e {{ {}, R{id}::Unit => R{id}::Unit }} }}", rb.join(", "));
    // rotate variants
    let rot: Vec<String> = (0..nv).map(|i| format!("R{id}::V{i}(x) => R{id}::V{}(x)", (i + 1) % nv)).collect();
    w!(items, "    fn rot{id}(e: R{id}) -> R{id} {{ match e {{ {}, R{id}::Unit => R{id}::V0({}) }} }}", rot.join(", "), val(rng));
    // through a reference: copy out only when the variant has a payload
    w!(items, "    #[inline(never)]\n    fn via_ref{id}(p: &R{id}) -> R{id} {{ match *p {{ R{id}::Unit => R{id}::Unit, _ => *p }} }}");
    w!(items, "    fn tag{id}(e: &R{id}) -> u8 {{ match e {{ {}, R{id}::Unit => {nv} }} }}", (0..nv).map(|i| format!("R{id}::V{i}(_) => {i}")).collect::<Vec<_>>().join(", "));
    w!(body, "        {{ let es = [{}, R{id}::Unit];", (0..nv).map(|i| format!("R{id}::V{i}({})", val(rng))).collect::<Vec<_>>().join(", "));
    w!(body, "        for e in es {{ let e = bb(e); let r1 = to_k{id}(e); let r2 = same{id}(e); let r3 = rot{id}(e); let r4 = via_ref{id}(&e);");
    w!(body, "            h.put(&tag{id}(&r1)); h.put(&(r2 == e)); h.put(&tag{id}(&r3)); h.put(&(r4 == e)); h.put(&(std::mem::discriminant(&r1) == std::mem::discriminant(&r2))); h.put(&(to_k{id}(r3) == to_k{id}(e))); }}");
    w!(body, "        h.put(&std::mem::size_of::<R{id}>()); }}");
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    let ne = rng.range(1, 2);
    let enums: Vec<En> = (0..ne).map(|i| make_enum(rng, format!("En{i}"))).collect();
    for e in &enums {
        w!(items, "{}", decl(e));
        w!(items, "{}", ctor(rng, e));
    }
    let nm = rng.range(2, 4);
    w!(body, "        let sels = [0u32, 1, 2, 3, 4, 5, 6, 7];");
    for id in 0..nm {
        let (code, tys) = matcher(rng, &enums, id);
        items += &code;
        let args: Vec<String> = tys.iter().enumerate().map(|(i, k)| format!("mk_En{k}(bb(sa{}), bb(sv{}))", i, i)).collect();
        let loops: String = (0..tys.len()).map(|i| format!("for (sa{i}, sv{i}) in sels.iter().map(|&s| (s, s.wrapping_mul({}).wrapping_add({}))) {{ ", rng.below(200) | 1, rng.below(255))).collect();
        w!(body, "        {loops}h.put(&m{id}({})); {}", args.join(", "), "}".repeat(tys.len()));
    }
    for id in 0..rng.range(1, 2) {
        items += &slice_matcher(rng, id);
        w!(body, "        for n in 0..8usize {{ let v: Vec<u8> = (0..n).map(|i| (i as u8).wrapping_mul({}).wrapping_add({})).collect(); h.put(&sm{id}(&v, bb({}))); }}", rng.below(9) | 1, rng.below(5), rng.below(255));
    }
    for rid in 0..rng.range(1, 3) {
        rebuild(rng, &mut items, &mut body, rid);
    }
    // fieldless enum <-> integer, Option niches
    w!(body, "        h.put(&std::mem::size_of::<En0>());");
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
