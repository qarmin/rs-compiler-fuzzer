//! Strategy `generics`: generic functions over a numeric trait, instantiated at every integer type,
//! called directly, through `fn` pointers and closures, plus generic structs and trait-bound
//! dispatch. The same MIR body is monomorphised twelve times, which stresses MIR inlining, GVN
//! across instantiations, `const BITS`-dependent folding and shift/rotate lowering per type.
//!
//! Safe code only; every operation of the `Num` trait is total (wrapping / masked shifts / `|1` divisors).

use crate::rng::Rng;
use crate::w;

const TYS: &[&str] = &["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize"];

const PRELUDE: &str = r#"    pub trait Num: Copy + PartialOrd + std::fmt::Debug {
        const BITS: u32;
        fn wadd(self, o: Self) -> Self; fn wsub(self, o: Self) -> Self; fn wmul(self, o: Self) -> Self;
        fn xor(self, o: Self) -> Self; fn and(self, o: Self) -> Self; fn or(self, o: Self) -> Self; fn not(self) -> Self;
        fn shl(self, n: u32) -> Self; fn shr(self, n: u32) -> Self; fn rotl(self, n: u32) -> Self;
        fn wdiv(self, o: Self) -> Self; fn wrem(self, o: Self) -> Self;
        fn from_u64(v: u64) -> Self; fn lo(self) -> u64; fn hi(self) -> u64;
        fn pop(self) -> u32; fn clz(self) -> u32; fn zero() -> Self; fn one() -> Self;
    }
    macro_rules! imp { ($($t:ty),*) => { $( impl Num for $t {
        const BITS: u32 = <$t>::BITS;
        #[inline] fn wadd(self, o: Self) -> Self { self.wrapping_add(o) }
        #[inline] fn wsub(self, o: Self) -> Self { self.wrapping_sub(o) }
        #[inline] fn wmul(self, o: Self) -> Self { self.wrapping_mul(o) }
        #[inline] fn xor(self, o: Self) -> Self { self ^ o }
        #[inline] fn and(self, o: Self) -> Self { self & o }
        #[inline] fn or(self, o: Self) -> Self { self | o }
        #[inline] fn not(self) -> Self { !self }
        #[inline] fn shl(self, n: u32) -> Self { self.wrapping_shl(n) }
        #[inline] fn shr(self, n: u32) -> Self { self.wrapping_shr(n) }
        #[inline] fn rotl(self, n: u32) -> Self { self.rotate_left(n) }
        #[inline] fn wdiv(self, o: Self) -> Self { self.wrapping_div(o | 1) }
        #[inline] fn wrem(self, o: Self) -> Self { self.wrapping_rem(o | 1) }
        #[inline] fn from_u64(v: u64) -> Self { v as $t }
        #[inline] fn lo(self) -> u64 { self as u64 }
        #[inline] fn hi(self) -> u64 { ((self as i128) >> 64) as u64 }
        #[inline] fn pop(self) -> u32 { self.count_ones() }
        #[inline] fn clz(self) -> u32 { self.leading_zeros() }
        #[inline] fn zero() -> Self { 0 }
        #[inline] fn one() -> Self { 1 }
    } )* } }
    imp!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
"#;

fn expr(rng: &mut Rng, vars: &[&str], depth: u32) -> String {
    if depth == 0 || rng.chance(15) {
        return if rng.chance(75) { rng.pick(vars).to_string() } else { format!("T::from_u64({}u64)", rng.next_u64() >> rng.below(60)) };
    }
    let d = depth - 1;
    let a = expr(rng, vars, d);
    let b = expr(rng, vars, d);
    match rng.below(18) {
        0 => format!("{a}.wadd({b})"),
        1 => format!("{a}.wsub({b})"),
        2 => format!("{a}.wmul({b})"),
        3 => format!("{a}.xor({b})"),
        4 => format!("{a}.and({b})"),
        5 => format!("{a}.or({b})"),
        6 => format!("{a}.not()"),
        7 => format!("{a}.shl({} % T::BITS)", rng.below(130)),
        8 => format!("{a}.shr(({b}).lo() as u32)"),
        9 => format!("{a}.rotl(({b}).pop() + {})", rng.below(70)),
        10 => format!("{a}.wdiv({b})"),
        11 => format!("{a}.wrem({b})"),
        12 => format!("(if {a} < {b} {{ {a} }} else {{ {b} }})"),
        13 => format!("(if {a} >= {b} {{ {a}.wsub({b}) }} else {{ {b}.wsub({a}) }})"),
        14 => format!("T::from_u64(({a}).clz() as u64)"),
        15 => format!("T::from_u64(({a}).lo() ^ ({b}).hi())"),
        16 => format!("(if ({a}).lo() & {} == 0 {{ {b} }} else {{ {a} }})", 1 << rng.below(4)),
        _ => format!("{a}.wadd(T::one()).shr({})", rng.below(8)),
    }
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::from(PRELUDE);
    let mut body = String::new();
    for id in 0..rng.range(2, 4) {
        let vars_in = ["a", "b", "c", "acc"];
        let sd = rng.range(2, 4) as u32;
        let step = expr(rng, &vars_in, sd);
        let init = expr(rng, &["a", "b", "c"], 2);
        let post = expr(rng, &["acc", "a", "b"], 2);
        let attr = *rng.pick(&["#[inline(never)]", "#[inline(always)]", "#[inline]", ""]);
        w!(items, "    {attr}\n    fn g{id}<T: Num>(a: T, b: T, n: u32) -> T {{");
        w!(items, "        let c = {};", expr(rng, &["a", "b"], 2));
        w!(items, "        let mut acc: T = {init};");
        w!(items, "        for i in 0..n {{ let a = a.wadd(T::from_u64(i as u64)); acc = {step}; }}");
        w!(items, "        match acc.lo() & 3 {{ 0 => {post}, 1 => acc.wadd(c), 2 => if a < b {{ b }} else {{ acc }}, _ => acc }}\n    }}");
        // generic struct with a method and a closure-taking function
        let bump = expr(rng, &["a", "b", "c", "acc"], 3);
        w!(items, "    struct S{id}<T: Num> {{ v: T, k: T }}");
        w!(items, "    impl<T: Num> S{id}<T> {{ #[inline(never)] fn bump(&mut self, x: T) -> T {{ let a = self.v; let b = self.k; let c = x; let acc = a.xor(c); self.v = {bump}; self.v }} }}");
        w!(items, "    fn app{id}<T: Num, F: Fn(T) -> T>(f: F, x: T, n: u32) -> T {{ let mut y = x; for _ in 0..n {{ y = f(y); }} y }}");
        for ty in TYS {
            if rng.chance(75) {
                let n = rng.range(0, 12);
                w!(body, "        {{ let (a, b) = (<{ty} as Num>::from_u64(bb({}u64)), <{ty} as Num>::from_u64(bb({}u64)));", rng.next_u64(), rng.next_u64());
                w!(body, "        let r = g{id}::<{ty}>(a, b, bb({n}u32)); h.put(&r.lo()); h.put(&r.hi());");
                if rng.chance(50) {
                    w!(body, "        let fp: fn({ty}, {ty}, u32) -> {ty} = bb(g{id}::<{ty}>); let r2 = fp(b, a, {}); h.put(&r2.lo());", rng.below(6));
                }
                if rng.chance(50) {
                    w!(body, "        let mut s = S{id}::<{ty}> {{ v: a, k: b }}; for i in 0..4u64 {{ h.put(&s.bump(<{ty} as Num>::from_u64(i)).lo()); }}");
                }
                if rng.chance(40) {
                    w!(body, "        h.put(&app{id}(|v: {ty}| v.wmul(b).wadd(a).rotl(3), a, bb({}u32)).lo());", rng.below(9));
                }
                w!(body, "        }}");
            }
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
