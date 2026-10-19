//! Strategy `unwind`: panics, landing pads and destructors.
//!
//! Functions create drop guards (logging their id when dropped), build partially initialised
//! arrays/vectors/structs, evaluate arguments that may panic, and panic in many different ways
//! (explicit, out-of-bounds, unwrap on None, division by zero, RefCell double borrow, bad slice
//! range, failed parse/convert). The caller wraps each call in `catch_unwind`; the hashed
//! observation is: did it panic, the panic message, the return value and the exact order in
//! which destructors ran. Related: rust-lang/rust#144371 (unwinding and callee-saved registers).
//!
//! Defined behaviour only: no panic inside a destructor, no unwinding through `extern "C"`.

use crate::rng::Rng;
use crate::w;

const PRELUDE: &str = r#"    use std::cell::RefCell;
    use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
    use std::rc::Rc;
    type Log = Rc<RefCell<Vec<u32>>>;
    struct G(u32, Log);
    impl Drop for G { fn drop(&mut self) { self.1.borrow_mut().push(self.0); } }
    struct Pair(G, G);
    fn msg(p: &Box<dyn std::any::Any + Send>) -> u64 {
        let s: &str = if let Some(s) = p.downcast_ref::<&str>() { s } else if let Some(s) = p.downcast_ref::<String>() { s } else { "?" };
        s.bytes().fold(5u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64))
    }
"#;

/// An expression that panics at runtime (the compiler cannot know it will).
fn panic_expr(rng: &mut Rng, k: usize) -> String {
    match rng.below(10) {
        0 => format!("panic!(\"explicit {k}\")"),
        1 => format!("{{ let v = vec![1u32, 2, 3]; v[bb({}usize)] }}", rng.range(3, 9)),
        2 => "{ let o: Option<u32> = if bb(true) { None } else { Some(1) }; o.unwrap() }".to_string(),
        3 => format!("{{ 10u32 / bb({}u32) }}", 0),
        4 => "{ let c = RefCell::new(1u32); let _a = c.borrow_mut(); let b = c.borrow_mut(); *b }".to_string(),
        5 => format!("{{ let s = [1u8, 2, 3, 4, 5]; let (a, b) = (bb({}usize), bb({}usize)); s[a..b].len() as u32 }}", 4, 2),
        6 => "{ \"x1\".parse::<i32>().unwrap() as u32 }".to_string(),
        7 => "{ u8::try_from(bb(300u32)).unwrap() as u32 }".to_string(),
        8 => format!("{{ let mut v = vec![0u32; 2]; v.remove(bb({}usize)) }}", rng.range(5, 9)),
        _ => "{ char::from_digit(bb(40u32), 10).unwrap() as u32 }".to_string(),
    }
}

fn func(rng: &mut Rng, k: usize, items: &mut String) {
    let modulus = rng.range(2, 5);
    let hit = rng.below(modulus);
    let mut b = String::new();
    let mut guards = 0;
    for _ in 0..rng.range(3, 8) {
        match rng.below(11) {
            0 | 1 => {
                guards += 1;
                w!(b, "        let _g{guards} = G({}, log.clone());", k * 100 + guards);
            }
            2 => w!(b, "        log.borrow_mut().push(x.wrapping_mul({}) ^ {});", rng.below(9) + 1, rng.below(50)),
            3 if k > 0 => {
                let j = rng.below(k);
                w!(b, "        let r{k}_{j} = f{j}(x.wrapping_add({}), log); acc = acc.wrapping_add(r{k}_{j});", rng.below(5));
            }
            4 => {
                let e = panic_expr(rng, k);
                w!(b, "        if x % {modulus} == {hit} {{ acc = acc.wrapping_add({e}); }}");
            }
            5 => {
                // vector built from an iterator that may panic part-way: the built prefix must be dropped
                guards += 1;
                let n = rng.range(2, 5);
                w!(b, "        let _v{guards}: Vec<G> = (0..{n}u32).map(|i| {{ if i == x % {} {{ panic!(\"mid {k}\") }} G({}, log.clone()) }}).collect();", n + 1, k * 100 + guards * 10);
            }
            6 => {
                guards += 1;
                w!(b, "        let _a{guards} = [G({}, log.clone()), if x % {modulus} == {hit} {{ panic!(\"arr {k}\") }} else {{ G({}, log.clone()) }}, G({}, log.clone())];", k * 100 + guards * 10, k * 100 + guards * 10 + 1, k * 100 + guards * 10 + 2);
            }
            7 => {
                guards += 1;
                w!(b, "        let _p{guards} = Pair(G({}, log.clone()), if x % {modulus} == {hit} {{ resume_unwind(Box::new(\"resumed {k}\")) }} else {{ G({}, log.clone()) }});", k * 100 + guards * 10, k * 100 + guards * 10 + 1);
            }
            8 => {
                // nested catch_unwind inside the function; the outer code continues after a caught panic
                let e = panic_expr(rng, k);
                w!(b, "        let inner = catch_unwind(AssertUnwindSafe(|| {{ let _gi = G({}, log.clone()); if x % {modulus} != {hit} {{ {e} }} else {{ 7u32 }} }}));", k * 100 + 99);
                w!(b, "        acc = acc.wrapping_add(match &inner {{ Ok(v) => *v, Err(p) => msg(p) as u32 }});");
            }
            9 => w!(b, "        std::mem::drop(G({}, log.clone())); acc ^= {};", k * 100 + 98, rng.below(1000)),
            _ => {
                // temporaries in the argument list: evaluation and drop order around a panicking argument
                let e = panic_expr(rng, k);
                w!(b, "        acc = acc.wrapping_add(take2(G({}, log.clone()), if x % {modulus} == {hit} {{ {e} }} else {{ 1 }}, G({}, log.clone())));", k * 100 + 90, k * 100 + 91);
            }
        }
    }
    w!(items, "    #[inline(never)]\n    fn f{k}(x: u32, log: &Log) -> u32 {{\n        let mut acc: u32 = x;\n{b}        acc ^ {}\n    }}", rng.below(100));
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::from(PRELUDE);
    w!(items, "    fn take2(a: G, b: u32, c: G) -> u32 {{ let _ = (&a, &c); b }}");
    let nf = rng.range(2, 5);
    for k in 0..nf {
        func(rng, k, &mut items);
    }
    let mut body = String::new();
    w!(body, "        let log: Log = Rc::new(RefCell::new(Vec::new()));");
    for _ in 0..rng.range(3, 7) {
        let k = rng.below(nf);
        w!(body, "        for x in [0u32, 1, 2, 3, 7, {}] {{", rng.below(100));
        w!(body, "            let r = catch_unwind(AssertUnwindSafe(|| f{k}(bb(x), &log)));");
        w!(body, "            match &r {{ Ok(v) => {{ h.put(&1u8); h.put(v); }} Err(p) => {{ h.put(&0u8); h.put(&msg(p)); }} }}");
        w!(body, "            h.put(&*log.borrow()); log.borrow_mut().clear();");
        w!(body, "        }}");
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
