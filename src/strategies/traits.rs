//! Strategy `traits`: dispatch, generics, const evaluation and destructor ordering.
//!
//! - default-vs-override trait methods reached through `dyn`, fn pointers, generics and boxes
//!   (cf. rust-lang/rust#163856, where parameter attributes were deduced from the default body);
//! - associated consts / const generics / monomorphisation of the same body at several types;
//! - `const fn` evaluated at compile time AND at run time (both are hashed);
//! - drop order: a scenario of lets, shadowing, temporaries, moves, `mem::forget`, early exits,
//!   logging each drop into a shared `Vec` that is hashed;
//! - a boxed expression tree evaluated before and after an ownership-consuming simplifier.

use super::lit;
use crate::rng::Rng;
use crate::w;

fn dyn_dispatch(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let nimpl = rng.range(2, 4);
    w!(items, "    trait Sh{id} {{ fn a(&self) -> u32; fn b(&self) -> u32 {{ self.a().wrapping_mul({}) ^ 1 }} fn c(&mut self, x: u32) -> u32 {{ let r = x ^ self.b(); r.rotate_left({}) }} fn d(&self, other: &dyn Sh{id}) -> u32 {{ self.b().wrapping_add(other.a()) }} }}", rng.next_u64() as u32 | 1, rng.below(32));
    for i in 0..nimpl {
        w!(items, "    struct T{id}_{i}(u32);");
        let mut m = format!("fn a(&self) -> u32 {{ self.0.wrapping_add({}) }} ", rng.next_u64() as u32);
        if rng.chance(50) {
            m += &format!("fn b(&self) -> u32 {{ self.0 ^ {} }} ", rng.next_u64() as u32);
        }
        if rng.chance(40) {
            m += &format!("fn c(&mut self, x: u32) -> u32 {{ self.0 = self.0.wrapping_add(x); self.0 ^ {} }} ", rng.next_u64() as u32);
        }
        if rng.chance(30) {
            m += &format!("fn d(&self, o: &dyn Sh{id}) -> u32 {{ o.b() ^ self.a() }} ");
        }
        w!(items, "    impl Sh{id} for T{id}_{i} {{ {m} }}");
    }
    w!(items, "    #[inline(never)]\n    fn via_dyn{id}(s: &mut dyn Sh{id}, o: &dyn Sh{id}, x: u32) -> u32 {{ s.c(x).wrapping_add(s.b()) ^ s.d(o) }}");
    w!(items, "    fn via_gen{id}<T: Sh{id}>(s: &mut T, x: u32) -> u32 {{ s.c(x) ^ s.b() ^ s.a() }}");
    w!(body, "        {{ let mut v: Vec<Box<dyn Sh{id}>> = vec![{}];", (0..nimpl).map(|i| format!("Box::new(T{id}_{i}(bb({})))", rng.next_u64() as u32)).collect::<Vec<_>>().join(", "));
    w!(body, "        let fp: fn(&dyn Sh{id}) -> u32 = |s| s.b(); let fq: fn(&dyn Sh{id}) -> u32 = |s| s.a();");
    w!(body, "        for k in 0..v.len() {{ let o = T{id}_{}(bb({})); let r = via_dyn{id}(&mut *v[k], &o, bb({})); h.put(&r); h.put(&fp(&*v[k])); h.put(&fq(&*v[k])); }}", rng.below(nimpl), rng.next_u64() as u32, rng.next_u64() as u32);
    for i in 0..nimpl {
        w!(body, "        {{ let mut t = T{id}_{i}(bb({})); h.put(&via_gen{id}(&mut t, bb({}))); }}", rng.next_u64() as u32, rng.next_u64() as u32);
    }
    w!(body, "        }}");
}

fn mono(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    w!(items, "    trait Op{id} {{ const K: u32; fn step(x: u32) -> u32 {{ x.wrapping_mul(Self::K) ^ Self::K }} }}");
    w!(items, "    struct OpA{id}; struct OpB{id};");
    w!(items, "    impl Op{id} for OpA{id} {{ const K: u32 = {}; }}", rng.next_u64() as u32 | 1);
    w!(items, "    impl Op{id} for OpB{id} {{ const K: u32 = {}; fn step(x: u32) -> u32 {{ x.rotate_left(Self::K % 32) ^ x }} }}", rng.next_u64() as u32);
    w!(items, "    #[inline(never)]\n    fn rep{id}<O: Op{id}>(n: u32, mut x: u32) -> u32 {{ for _ in 0..n {{ x = O::step(x); }} x }}");
    w!(items, "    fn comp{id}<A: Op{id}, B: Op{id}>(x: u32) -> u32 {{ B::step(A::step(x)) ^ A::K }}");
    w!(items, "    struct Buf{id}<const N: usize>([u32; N]);");
    w!(items, "    impl<const N: usize> Buf{id}<N> {{ fn new(seed: u32) -> Self {{ let mut a = [0u32; N]; for i in 0..N {{ a[i] = seed.wrapping_mul(i as u32 + 1); }} Buf{id}(a) }} fn sum(&self) -> u32 {{ self.0.iter().fold(0u32, |a, &b| a.rotate_left(3) ^ b) }} const LEN: usize = N * 2; }}");
    w!(body, "        {{ let x = bb({}u32); h.put(&rep{id}::<OpA{id}>(bb({}), x)); h.put(&rep{id}::<OpB{id}>(bb({}), x)); h.put(&comp{id}::<OpA{id}, OpB{id}>(x)); h.put(&comp{id}::<OpB{id}, OpA{id}>(x));", rng.next_u64() as u32, rng.range(0, 20), rng.range(0, 20));
    w!(body, "        h.put(&Buf{id}::<3>::new(x).sum()); h.put(&Buf{id}::<{}>::new(x).sum()); h.put(&Buf{id}::<5>::LEN); }}", rng.range(4, 20));
}

fn cexpr(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 || rng.chance(15) {
        return match rng.below(3) {
            0 => "x".into(),
            1 => "y".into(),
            _ => lit(rng, "u64"),
        };
    }
    let (a, b) = (cexpr(rng, depth - 1), cexpr(rng, depth - 1));
    match rng.below(13) {
        0 => format!("{a}.wrapping_add({b})"),
        1 => format!("{a}.wrapping_sub({b})"),
        2 => format!("{a}.wrapping_mul({b})"),
        3 => format!("({a} ^ {b})"),
        4 => format!("({a} & {b})"),
        5 => format!("({a} | {b})"),
        6 => format!("{a}.rotate_left({} as u32)", rng.below(64)),
        7 => format!("{a}.wrapping_shr(({b}) as u32)"),
        8 => format!("{a}.saturating_add({b})"),
        9 => format!("({a}.count_ones() as u64)"),
        10 => format!("{a}.overflowing_mul({b}).0"),
        11 => format!("(if {a} < {b} {{ {a} }} else {{ {b}.swap_bytes() }})"),
        _ => format!("{a}.abs_diff({b})"),
    }
}

fn const_eval(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let step = cexpr(rng, 4);
    let fin = cexpr(rng, 3);
    let n = rng.range(1, 12);
    w!(items, "    const fn cf{id}(x0: u64, y: u64) -> u64 {{ let mut x = x0; let mut i = 0u64; while i < {n} {{ x = {step}; x = x.wrapping_add(i); i += 1; }} let x = x; {fin} }}");
    for k in 0..rng.range(1, 3) {
        let (a, b) = (rng.next_u64(), rng.next_u64());
        w!(items, "    const C{id}_{k}: u64 = cf{id}({a}, {b});");
        w!(items, "    static S{id}_{k}: [u64; 2] = [cf{id}({b}, {a}), C{id}_{k}];");
        w!(body, "        h.put(&C{id}_{k}); h.put(&cf{id}(bb({a}u64), bb({b}u64))); h.put(&(C{id}_{k} == cf{id}(bb({a}u64), bb({b}u64)))); h.put(&S{id}_{k}[0]); h.put(&(S{id}_{k}[0] == cf{id}(bb({b}u64), bb({a}u64))));");
    }
}

fn drops(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    w!(items, "    struct D{id}(u32, std::rc::Rc<std::cell::RefCell<Vec<u32>>>);");
    w!(items, "    impl Drop for D{id} {{ fn drop(&mut self) {{ self.1.borrow_mut().push(self.0); }} }}");
    w!(items, "    struct Pair{id} {{ a: D{id}, b: D{id} }}");
    w!(items, "    fn tmp{id}(d: &D{id}) -> u32 {{ d.0 }}");
    w!(items, "    #[inline(never)]\n    fn scenario{id}(log: &std::rc::Rc<std::cell::RefCell<Vec<u32>>>, c: u32) -> u32 {{");
    w!(items, "        let mut r = 0u32;");
    let mut live: Vec<String> = Vec::new();
    let mut fresh = 0u32;
    let mk = |fresh: &mut u32| {
        *fresh += 1;
        (format!("d{fresh}"), *fresh * 10 + (id as u32) * 1000)
    };
    for _ in 0..rng.range(4, 12) {
        match rng.below(11) {
            0..=2 => {
                let (n, tag) = mk(&mut fresh);
                if rng.chance(25) && !live.is_empty() {
                    // shadowing: the old value lives until end of scope
                    let old = rng.pick(&live).clone();
                    w!(items, "        let {old} = D{id}({tag}, log.clone());");
                } else {
                    w!(items, "        let {n} = D{id}({tag}, log.clone());");
                    live.push(n);
                }
            }
            3 => {
                let (_, tag) = mk(&mut fresh);
                w!(items, "        let _ = D{id}({tag}, log.clone());");
            }
            4 => {
                let (_, tag) = mk(&mut fresh);
                w!(items, "        r = r.wrapping_add(tmp{id}(&D{id}({tag}, log.clone())));");
            }
            5 if !live.is_empty() => {
                let i = rng.below(live.len());
                let n = live.remove(i);
                w!(items, "        drop({n});");
            }
            6 if !live.is_empty() => {
                let i = rng.below(live.len());
                let n = live.remove(i);
                w!(items, "        std::mem::forget({n});");
            }
            7 if !live.is_empty() => {
                let i = rng.below(live.len());
                let n = live.remove(i);
                w!(items, "        let cl = move || {{ let _k = &{n}; {n}.0 }}; r = r.wrapping_add(cl());");
            }
            8 => {
                let (a, ta) = mk(&mut fresh);
                let (b, tb) = mk(&mut fresh);
                let _ = (a, b);
                w!(items, "        {{ let p = Pair{id} {{ a: D{id}({ta}, log.clone()), b: D{id}({tb}, log.clone()) }}; let Pair{id} {{ a: _, b: ref bb_ }} = p; r ^= bb_.0; }}");
            }
            9 => {
                let (_, t1) = mk(&mut fresh);
                let (_, t2) = mk(&mut fresh);
                w!(items, "        {{ let mut v = vec![D{id}({t1}, log.clone()), D{id}({t2}, log.clone()), D{id}({}, log.clone())]; let p = v.pop(); v.truncate(1); drop(p); let o = Some(D{id}({}, log.clone())); let mut o = o; let t = o.take(); r = r.wrapping_add(t.map(|d| d.0).unwrap_or(0)); }}", t1 + 1, t2 + 1);
            }
            _ => {
                let (_, t1) = mk(&mut fresh);
                w!(items, "        'blk: {{ let _g = D{id}({t1}, log.clone()); if c & 1 == 1 {{ break 'blk; }} r += 1; }}");
            }
        }
    }
    w!(items, "        if c == 3 {{ return r; }}\n        r\n    }}");
    w!(body, "        for c in [0u32, 1, 3] {{ let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new())); let r = scenario{id}(&log, bb(c)); h.put(&r); h.put(&*log.borrow()); }}");
}

fn expr_tree(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    w!(items, "    #[derive(Clone)] enum Ex{id} {{ Lit(u32), Add(Box<Ex{id}>, Box<Ex{id}>), Mul(Box<Ex{id}>, Box<Ex{id}>), Neg(Box<Ex{id}>), If(Box<Ex{id}>, Box<Ex{id}>, Box<Ex{id}>), Var }}");
    w!(items, "    fn eval{id}(e: &Ex{id}, v: u32) -> u32 {{ match e {{ Ex{id}::Lit(n) => *n, Ex{id}::Var => v, Ex{id}::Add(a, b) => eval{id}(a, v).wrapping_add(eval{id}(b, v)), Ex{id}::Mul(a, b) => eval{id}(a, v).wrapping_mul(eval{id}(b, v)), Ex{id}::Neg(a) => eval{id}(a, v).wrapping_neg(), Ex{id}::If(c, t, f) => if eval{id}(c, v) & 1 == 1 {{ eval{id}(t, v) }} else {{ eval{id}(f, v) }} }} }}");
    w!(items, "    fn simp{id}(e: Ex{id}) -> Ex{id} {{ match e {{ Ex{id}::Add(a, b) => match (simp{id}(*a), simp{id}(*b)) {{ (Ex{id}::Lit(0), x) | (x, Ex{id}::Lit(0)) => x, (Ex{id}::Lit(x), Ex{id}::Lit(y)) => Ex{id}::Lit(x.wrapping_add(y)), (x, y) => Ex{id}::Add(Box::new(x), Box::new(y)) }}, Ex{id}::Mul(a, b) => match (simp{id}(*a), simp{id}(*b)) {{ (Ex{id}::Lit(1), x) | (x, Ex{id}::Lit(1)) => x, (Ex{id}::Lit(0), _) | (_, Ex{id}::Lit(0)) => Ex{id}::Lit(0), (Ex{id}::Lit(x), Ex{id}::Lit(y)) => Ex{id}::Lit(x.wrapping_mul(y)), (x, y) => Ex{id}::Mul(Box::new(x), Box::new(y)) }}, Ex{id}::Neg(a) => match simp{id}(*a) {{ Ex{id}::Neg(x) => *x, Ex{id}::Lit(n) => Ex{id}::Lit(n.wrapping_neg()), x => Ex{id}::Neg(Box::new(x)) }}, Ex{id}::If(c, t, f) => Ex{id}::If(Box::new(simp{id}(*c)), Box::new(simp{id}(*t)), Box::new(simp{id}(*f))), other => other }} }}");
    fn build(rng: &mut Rng, id: usize, d: u32) -> String {
        if d == 0 || rng.chance(20) {
            return match rng.below(4) {
                0 => "Ex{id}::Var".replace("{id}", &id.to_string()),
                1 => format!("Ex{id}::Lit({})", rng.below(3)),
                _ => format!("Ex{id}::Lit({})", rng.next_u64() as u32),
            };
        }
        let b = |rng: &mut Rng| format!("Box::new({})", build(rng, id, d - 1));
        match rng.below(4) {
            0 => format!("Ex{id}::Add({}, {})", b(rng), b(rng)),
            1 => format!("Ex{id}::Mul({}, {})", b(rng), b(rng)),
            2 => format!("Ex{id}::Neg({})", b(rng)),
            _ => format!("Ex{id}::If({}, {}, {})", b(rng), b(rng), b(rng)),
        }
    }
    let depth_ = rng.range(3, 6) as u32;
    let tree = build(rng, id, depth_);
    w!(body, "        {{ let e = {tree}; let s = simp{id}(e.clone()); for v in [0u32, 1, 2, bb({}), u32::MAX] {{ h.put(&eval{id}(&e, bb(v))); h.put(&(eval{id}(&e, v) == eval{id}(&s, v))); }} }}", rng.next_u64() as u32);
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    for id in 0..rng.range(2, 5) {
        match rng.below(6) {
            0 | 1 => dyn_dispatch(rng, &mut items, &mut body, id),
            2 => mono(rng, &mut items, &mut body, id),
            3 => const_eval(rng, &mut items, &mut body, id),
            4 => drops(rng, &mut items, &mut body, id),
            _ => expr_tree(rng, &mut items, &mut body, id),
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
