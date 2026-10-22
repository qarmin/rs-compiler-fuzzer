//! Packing many independent cases into ONE rust file, so a single rustc invocation
//! amortises its startup/link cost (1 x 4s instead of 20 x 1s).
//!
//! Every case lives in its own `mod case_N` (so names never collide) exposing
//! `pub fn run(h: &mut H)`. `main` runs them one after another under `catch_unwind`
//! and prints exactly one line per case: `<id> <hash>` (or `<id> PANIC`). A divergence
//! between two compiler configurations is therefore attributable to a single case just
//! by diffing stdout lines.

use std::fmt::Write;

#[derive(Clone)]
pub struct Case {
    pub id: usize,
    pub strategy: &'static str,
    pub seed: u64,
    /// Module body: items plus `pub fn run(h: &mut H)`.
    pub body: String,
}

pub struct Assembled {
    pub source: String,
    /// `(case id, first line, last line)` - 1-based, inclusive.
    pub spans: Vec<(usize, usize, usize)>,
}

const RT: &str = r#"#![allow(warnings)]
#![allow(unused, dead_code, unsafe_op_in_unsafe_fn)]
mod rt {
    use std::hint::black_box;
    pub struct H(pub u64);
    impl H {
        pub fn new() -> H { H(0xcbf2_9ce4_8422_2325) }
        #[inline(never)]
        pub fn feed(&mut self, v: u64) {
            self.0 = (self.0 ^ v).wrapping_mul(0x0000_0100_0000_01b3);
            self.0 ^= self.0 >> 29;
        }
        pub fn put<T: Feed + ?Sized>(&mut self, v: &T) { v.feed_into(self) }
        pub fn feed128(&mut self, v: u128) { self.feed(v as u64); self.feed((v >> 64) as u64); }
    }
    pub trait Feed { fn feed_into(&self, h: &mut H); }
    macro_rules! feed_int { ($($t:ty),*) => { $( impl Feed for $t { fn feed_into(&self, h: &mut H) { h.feed(*self as u64) } } )* } }
    feed_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, bool, char);
    impl Feed for str { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for b in self.bytes() { h.feed(b as u64) } } }
    impl Feed for String { fn feed_into(&self, h: &mut H) { self.as_str().feed_into(h) } }
    impl Feed for f64 { fn feed_into(&self, h: &mut H) { h.feed(if self.is_nan() { 0x7ff8_dead } else { self.to_bits() }) } }
    impl Feed for f32 { fn feed_into(&self, h: &mut H) { h.feed(if self.is_nan() { 0x7fc0_dead } else { self.to_bits() as u64 }) } }
    impl Feed for std::cmp::Ordering { fn feed_into(&self, h: &mut H) { h.feed(*self as i8 as u64) } }
    impl Feed for std::time::Duration { fn feed_into(&self, h: &mut H) { h.feed128(self.as_nanos()) } }
    impl<A: Feed, B: Feed, C: Feed, D: Feed> Feed for (A, B, C, D) { fn feed_into(&self, h: &mut H) { self.0.feed_into(h); self.1.feed_into(h); self.2.feed_into(h); self.3.feed_into(h) } }
    impl<T: Feed> Feed for std::collections::BTreeSet<T> { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for x in self { x.feed_into(h) } } }
    impl<K: Feed, V: Feed> Feed for std::collections::BTreeMap<K, V> { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for (k, v) in self { k.feed_into(h); v.feed_into(h) } } }
    impl<T: Feed> Feed for std::collections::LinkedList<T> { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for x in self { x.feed_into(h) } } }
    impl Feed for () { fn feed_into(&self, h: &mut H) { h.feed(0x55) } }
    impl<A: Feed, B: Feed, C: Feed> Feed for (A, B, C) { fn feed_into(&self, h: &mut H) { self.0.feed_into(h); self.1.feed_into(h); self.2.feed_into(h) } }
    impl<T: Feed, E: Feed> Feed for Result<T, E> {
        fn feed_into(&self, h: &mut H) { match self { Ok(v) => { h.feed(2); v.feed_into(h) } Err(e) => { h.feed(3); e.feed_into(h) } } }
    }
    impl<T: Feed> Feed for std::collections::VecDeque<T> { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for x in self { x.feed_into(h) } } }
    impl Feed for u128 { fn feed_into(&self, h: &mut H) { h.feed128(*self) } }
    impl Feed for i128 { fn feed_into(&self, h: &mut H) { h.feed128(*self as u128) } }
    impl<T: Feed> Feed for Option<T> {
        fn feed_into(&self, h: &mut H) { match self { Some(v) => { h.feed(1); v.feed_into(h) } None => h.feed(0) } }
    }
    impl<A: Feed, B: Feed> Feed for (A, B) { fn feed_into(&self, h: &mut H) { self.0.feed_into(h); self.1.feed_into(h) } }
    impl<T: Feed> Feed for [T] { fn feed_into(&self, h: &mut H) { h.feed(self.len() as u64); for x in self { x.feed_into(h) } } }
    impl<T: Feed> Feed for Vec<T> { fn feed_into(&self, h: &mut H) { self[..].feed_into(h) } }
    impl<T: Feed, const N: usize> Feed for [T; N] { fn feed_into(&self, h: &mut H) { self[..].feed_into(h) } }
    impl<T: Feed + ?Sized> Feed for &T { fn feed_into(&self, h: &mut H) { (**self).feed_into(h) } }
    #[inline(always)] pub fn bb<T>(x: T) -> T { black_box(x) }
    pub fn report(id: usize, ok: bool, h: &H) {
        if ok { println!("{} {:016x}", id, h.0) } else { println!("{} PANIC", id) }
    }
}
use rt::*;
"#;

pub fn assemble(cases: &[&Case]) -> Assembled {
    let mut s = String::from(RT);
    let mut spans = Vec::new();
    for c in cases {
        let start = s.matches('\n').count() + 1;
        writeln!(s, "mod case_{} {{\n    use super::rt::*;", c.id).unwrap();
        s.push_str(&c.body);
        s.push_str("\n}\n");
        let end = s.matches('\n').count();
        spans.push((c.id, start, end));
    }
    s.push_str(
        "macro_rules! go { ($id:literal, $m:ident) => {{\n    let mut h = H::new();\n    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $m::run(&mut h))).is_ok();\n    report($id, ok, &h);\n}}; }\n",
    );
    s.push_str("fn main() {\n    std::panic::set_hook(Box::new(|_| {}));\n");
    for c in cases {
        writeln!(s, "    go!({}, case_{});", c.id, c.id).unwrap();
    }
    s.push_str("}\n");
    Assembled { source: s, spans }
}
