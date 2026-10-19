//! Strategy `abi`: calls between functions using different calling conventions
//! (`extern "C"`, `"sysv64"`, `"win64"`, `"system"`) with many argument/return kinds, called
//! directly, through black-boxed function pointers, and chained so that values stay live across
//! calls into a different ABI (the two x86-64 ABIs disagree about which registers - e.g. xmm6-15 -
//! are callee-saved). Related: rust-lang/rust#144371, #163911 (bool return values).
//!
//! All functions are defined in Rust, so this is defined behaviour; the cases are x86_64 only
//! (`#[cfg(target_arch = "x86_64")]` - elsewhere the case is empty).

use crate::rng::Rng;
use crate::w;

#[derive(Clone)]
enum Kind {
    Int(&'static str),
    Bool,
    Float(&'static str),
    Struct(usize), // index into this case's struct list
}

struct Gen {
    structs: Vec<Vec<Kind>>, // fields (never nested structs)
}

fn ty(_g: &Gen, k: &Kind) -> String {
    match k {
        Kind::Int(t) | Kind::Float(t) => t.to_string(),
        Kind::Bool => "bool".into(),
        Kind::Struct(i) => format!("St{i}"),
    }
}

/// Expression of type u64 digesting the value named `v` of kind `k`.
fn fold(g: &Gen, k: &Kind, v: &str) -> String {
    match k {
        Kind::Int("u128") | Kind::Int("i128") => format!("(({v} as u64) ^ (({v} >> 64) as u64))"),
        Kind::Int(_) => format!("({v} as u64)"),
        Kind::Bool => format!("({v} as u64)"),
        Kind::Float("f32") => format!("({v}.to_bits() as u64)"),
        Kind::Float(_) => format!("{v}.to_bits()"),
        Kind::Struct(i) => {
            let parts: Vec<String> = g.structs[*i].iter().enumerate().map(|(j, f)| format!("{}.rotate_left({})", fold(g, f, &format!("{v}.f{j}")), j * 7 + 3)).collect();
            format!("({})", parts.join(" ^ "))
        }
    }
}

/// Expression of kind `k` built from digest variable `d` (u64).
fn make(g: &Gen, k: &Kind, d: &str, salt: u32) -> String {
    match k {
        Kind::Int(t) => format!("({d}.rotate_left({salt}) as {t})"),
        Kind::Bool => format!("({d}.rotate_left({salt}) & 1 == 1)"),
        Kind::Float(t) => format!("((({d}.rotate_left({salt}) % 2000) as i64 - 1000) as {t} * 0.25)"),
        Kind::Struct(i) => {
            let fs: Vec<String> = g.structs[*i].iter().enumerate().map(|(j, f)| format!("f{j}: {}", make(g, f, d, salt + j as u32 * 5 + 1))).collect();
            format!("St{i} {{ {} }}", fs.join(", "))
        }
    }
}

fn lit(g: &Gen, rng: &mut Rng, k: &Kind) -> String {
    match k {
        Kind::Int(t) => super::lit(rng, t),
        Kind::Bool => rng.chance(50).to_string(),
        Kind::Float(t) => format!("{}.{}{t}", rng.below(2000), rng.below(100)),
        Kind::Struct(i) => {
            let fs: Vec<String> = g.structs[*i].iter().enumerate().map(|(j, f)| format!("f{j}: {}", lit(g, rng, f))).collect();
            format!("St{i} {{ {} }}", fs.join(", "))
        }
    }
}

fn rand_kind(rng: &mut Rng, nstructs: usize, allow_struct: bool) -> Kind {
    match rng.below(if allow_struct && nstructs > 0 { 10 } else { 8 }) {
        0 => Kind::Int("u8"),
        1 => Kind::Int("u16"),
        2 => Kind::Int("i32"),
        3 => Kind::Int("u64"),
        4 => Kind::Int(*rng.pick(&["i128", "u128", "i8", "i64"])),
        5 => Kind::Bool,
        6 => Kind::Float("f32"),
        7 => Kind::Float("f64"),
        _ => Kind::Struct(rng.below(nstructs)),
    }
}

struct Func {
    name: String,
    abi: &'static str,
    params: Vec<Kind>,
    ret: Kind,
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    // repr(C) structs of 1..5 scalar fields
    let ns = rng.range(1, 3);
    let mut g = Gen { structs: Vec::new() };
    for i in 0..ns {
        let nf = rng.range(1, 5);
        let fields: Vec<Kind> = (0..nf).map(|_| rand_kind(rng, 0, false)).collect();
        let decl: Vec<String> = fields.iter().enumerate().map(|(j, f)| format!("f{j}: {}", ty(&g, f))).collect();
        w!(items, "    #[cfg(target_arch = \"x86_64\")] #[repr(C)] #[derive(Clone, Copy, Debug)] struct St{i} {{ {} }}", decl.join(", "));
        g.structs.push(fields);
    }
    let mut funcs: Vec<Func> = Vec::new();
    for k in 0..rng.range(3, 6) {
        let abi = *rng.pick(&["C", "sysv64", "win64", "system", "C"]);
        let np = rng.range(1, 9);
        let params: Vec<Kind> = (0..np).map(|_| rand_kind(rng, ns, true)).collect();
        let ret = rand_kind(rng, ns, true);
        let name = format!("ab{k}");
        let sig: Vec<String> = params.iter().enumerate().map(|(i, p)| format!("p{i}: {}", ty(&g, p))).collect();
        let mut b = String::new();
        w!(b, "        let mut d: u64 = {}u64;", rng.next_u64() | 1);
        for (i, p) in params.iter().enumerate() {
            w!(b, "        d = d.rotate_left({}).wrapping_mul({}) ^ {};", rng.below(60) + 1, rng.next_u64() | 1, fold(&g, p, &format!("p{i}")));
        }
        // call an earlier function (possibly with another ABI) and keep our parameters live across the call
        if !funcs.is_empty() && rng.chance(75) {
            let c = &funcs[rng.below(funcs.len())];
            let args: Vec<String> = c
                .params
                .iter()
                .enumerate()
                .map(|(ai, ck)| {
                    // pass one of our parameters of the same kind if there is one
                    let same: Vec<usize> = params.iter().enumerate().filter(|(_, pk)| ty(&g, pk) == ty(&g, ck)).map(|(i, _)| i).collect();
                    if !same.is_empty() && rng.chance(70) { format!("p{}", rng.pick(&same)) } else { make(&g, ck, "d", ai as u32 + 1) }
                })
                .collect();
            let via_ptr = rng.chance(50);
            let call = if via_ptr {
                let ptys: Vec<String> = c.params.iter().map(|p| ty(&g, p)).collect();
                format!("{{ let fp: extern \"{}\" fn({}) -> {} = bb({}); fp({}) }}", c.abi, ptys.join(", "), ty(&g, &c.ret), c.name, args.join(", "))
            } else {
                format!("{}({})", c.name, args.join(", "))
            };
            w!(b, "        let r = {call};");
            w!(b, "        d = d.rotate_left(13) ^ {};", fold(&g, &c.ret, "r"));
            for (i, p) in params.iter().enumerate() {
                w!(b, "        d = d.wrapping_add({}); // live across the call", fold(&g, p, &format!("p{i}")));
            }
        }
        let retexpr = make(&g, &ret, "d", 3);
        w!(items, "    #[cfg(target_arch = \"x86_64\")]\n    {}\n    extern \"{abi}\" fn {name}({}) -> {} {{\n{b}        {retexpr}\n    }}", rng.pick(&["#[inline(never)]", "#[inline(never)]", "#[inline]", ""]), sig.join(", "), ty(&g, &ret));
        funcs.push(Func { name, abi, params, ret });
    }
    w!(body, "        #[cfg(target_arch = \"x86_64\")]\n        {{");
    for _ in 0..rng.range(3, 8) {
        let f = &funcs[rng.below(funcs.len())];
        let args: Vec<String> = f.params.iter().map(|p| format!("bb({})", lit(&g, rng, p))).collect();
        let ptys: Vec<String> = f.params.iter().map(|p| ty(&g, p)).collect();
        let call = if rng.chance(50) {
            format!("{}({})", f.name, args.join(", "))
        } else {
            format!("{{ let fp: extern \"{}\" fn({}) -> {} = bb({}); fp({}) }}", f.abi, ptys.join(", "), ty(&g, &f.ret), f.name, args.join(", "))
        };
        w!(body, "        let r = {call}; h.put(&{});", fold(&g, &f.ret, "r"));
    }
    w!(body, "        }}");
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
