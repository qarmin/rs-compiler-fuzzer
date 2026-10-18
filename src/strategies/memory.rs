//! Strategy `memory`: `unsafe` and layout-sensitive code, written so it stays defined behaviour.
//!
//! Validity arguments, per snippet:
//! - unions: every field is plain integers/arrays of integers of the same size, the union is
//!   always fully initialised through its widest field, so any field read is valid;
//! - `repr(packed)`: fields are only copied out by value (`let x = p.f;`) or read through
//!   `addr_of!(..).read_unaligned()`, never referenced;
//! - `MaybeUninit` stack: an element is only read back after it was written, `len <= N`;
//! - raw-pointer walks: every offset stays within `[0, len)` of one live allocation, `read_unaligned`
//!   / `write_unaligned` are used for any possibly misaligned access;
//! - niches: `NonZero*` values come from `new(..)`/`| 1`; enums are only built from valid discriminants;
//! - provenance: pointers are only turned into integers via `expose_provenance` and back via
//!   `with_exposed_provenance`, and we never compare addresses of different allocations for equality
//!   (that result is legitimately unspecified).

use super::lit;
use crate::rng::Rng;
use crate::w;

fn union_siblings(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    w!(items, "    #[repr(C)] #[derive(Clone, Copy)] struct Pair{id} {{ lo: u32, hi: u32 }}");
    w!(items, "    #[repr(C)] #[derive(Clone, Copy)] union U{id} {{ w: u64, p: Pair{id}, b: [u8; 8], s: [u16; 4] }}");
    w!(items, "    #[inline(never)]\n    fn upd{id}(mut u: U{id}, sel: u32, v: u32) -> u64 {{ unsafe {{");
    w!(items, "        match sel % 4 {{ 0 => u.p.lo = v, 1 => u.p.hi = v, 2 => u.b[(v % 8) as usize] = v as u8, _ => u.s[(v % 4) as usize] = v as u16 }}");
    w!(items, "        let a = u.w; if sel & 8 != 0 {{ u.p.hi = a as u32; }} let b = u.w; a ^ b.rotate_left(7) ^ u.s[1] as u64 }} }}");
    w!(body, "        {{ let mut u = U{id} {{ w: bb({}) }};", lit(rng, "u64"));
    for _ in 0..rng.range(2, 6) {
        match rng.below(4) {
            0 => w!(body, "        h.put(&upd{id}(u, bb({}), bb({})));", rng.below(16), rng.next_u64() as u32),
            1 => w!(body, "        unsafe {{ u.p.lo = bb({}); if bb({}) {{ u.b[2] = 7; }} h.put(&u.w); h.put(&u.s[3]); }}", rng.next_u64() as u32, rng.chance(50)),
            2 => w!(body, "        unsafe {{ u.w = u.w.wrapping_mul({}); h.put(&u.b); }}", lit(rng, "u64")),
            _ => w!(body, "        unsafe {{ let q = u; u.s[0] = q.s[1]; u.s[1] = q.s[0]; h.put(&u.p.lo); }}"),
        }
    }
    w!(body, "        }}");
}

fn packed(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    w!(items, "    #[repr(C, packed)] #[derive(Clone, Copy)] struct P{id} {{ a: u8, b: u32, c: u16, d: u64 }}");
    w!(items, "    const CP{id}: P{id} = P{id} {{ a: {}, b: {}, c: {}, d: {} }};", rng.below(256), rng.next_u64() as u32, rng.below(65536), rng.next_u64());
    w!(items, "    static SP{id}: P{id} = P{id} {{ a: {}, b: {}, c: {}, d: {} }};", rng.below(256), rng.next_u64() as u32, rng.below(65536), rng.next_u64());
    w!(items, "    #[inline(never)]\n    fn sum{id}(xs: &[P{id}]) -> u64 {{ let mut s = 0u64; for p in xs {{ let (a, b, c, d) = (p.a, p.b, p.c, p.d); s = s.wrapping_mul(31).wrapping_add(a as u64 ^ b as u64 ^ ((c as u64) << 3) ^ d); }} s }}");
    w!(body, "        {{ let x = CP{id}.b; let y = bb(CP{id}).c; let z = SP{id}.d; h.put(&x); h.put(&y); h.put(&z);");
    w!(body, "        let mut arr = [CP{id}; {}]; for (i, p) in arr.iter_mut().enumerate() {{ p.b = (i as u32).wrapping_mul({}); let d = p.d; p.d = d.wrapping_add(i as u64); p.a = p.a ^ (i as u8); }}", rng.range(2, 6), rng.next_u64() as u32);
    w!(body, "        h.put(&sum{id}(&arr));");
    w!(body, "        let q = &arr[bb(1usize)]; let u = unsafe {{ std::ptr::addr_of!(q.b).read_unaligned() }}; h.put(&u); }}");
}

fn uninit_stack(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = *rng.pick(&["i32", "u64", "u8", "i16", "u32"]);
    let n = *rng.pick(&[4usize, 8, 16]);
    w!(items, "    // known-bug: rust-lang/rust#164297 (lto=fat)");
    w!(items, "    struct Stack{id} {{ buf: [std::mem::MaybeUninit<{ty}>; {n}], len: usize }}");
    w!(items, "    impl Stack{id} {{");
    w!(items, "        #[inline(always)]\n        fn push(&mut self, v: {ty}) {{ unsafe {{ self.buf.as_mut_ptr().add(self.len).cast::<{ty}>().write(v) }}; self.len += 1; }}");
    w!(items, "        fn pop(&mut self) -> Option<{ty}> {{ if self.len == 0 {{ return None; }} self.len -= 1; Some(unsafe {{ self.buf[self.len].assume_init_read() }}) }}");
    w!(items, "    }}");
    w!(items, "    fn stack{id}(flag: u16) -> u64 {{ let mut s = Stack{id} {{ buf: [std::mem::MaybeUninit::new(0); {n}], len: 0 }};");
    w!(items, "        for i in 0..(flag as usize % {}) + 1 {{ s.push((i as {ty}).wrapping_mul({})); }}", n.min(rng.range(2, 8)), lit(rng, ty));
    w!(items, "        let mut sum: u64 = 0; while let Some(x) = s.pop() {{ sum = sum.wrapping_add(x as u64); }} sum ^ s.len as u64 }}");
    for _ in 0..rng.range(1, 3) {
        w!(body, "        h.put(&stack{id}(bb({}u16)));", rng.below(1000));
    }
}

fn raw_walk(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = *rng.pick(&["u8", "u16", "u32", "u64", "i32", "i64"]);
    let n = rng.range(8, 40);
    w!(items, "    #[inline(never)]\n    fn walk{id}(v: &mut Vec<{ty}>, a: usize, b: usize, k: usize) -> {ty} {{");
    w!(items, "        let len = v.len(); let p = v.as_mut_ptr(); let mut acc: {ty} = 0;");
    w!(items, "        unsafe {{");
    w!(items, "            for i in 0..len {{ *p.add(i) = (*p.add(i)).wrapping_mul({}).wrapping_add(i as {ty}); }}", lit(rng, ty));
    w!(items, "            let (a, b) = (a % len, b % len); let k = k % (len - a.max(b)) ;");
    w!(items, "            std::ptr::copy(p.add(a), p.add(b), k);");
    w!(items, "            std::ptr::swap(p.add(a), p.add(b));");
    w!(items, "            acc = acc.wrapping_add(p.add(len - 1).offset_from(p) as {ty});");
    w!(items, "            for i in 0..len {{ acc = acc.rotate_left(1) ^ p.add(i).read(); }}");
    w!(items, "        }}\n        acc\n    }}");
    w!(body, "        {{ let mut v: Vec<{ty}> = (0..{n}).map(|i| i as {ty}).collect();");
    for _ in 0..rng.range(1, 3) {
        w!(body, "        h.put(&walk{id}(&mut v, bb({}), bb({}), bb({})));", rng.below(100), rng.below(100), rng.below(100));
    }
    w!(body, "        h.put(&v);");
    // unaligned reads/writes over a byte buffer
    w!(body, "        let mut bytes = [0u8; 32]; for (i, b) in bytes.iter_mut().enumerate() {{ *b = (i as u8).wrapping_mul({}); }}", rng.below(256) | 1);
    w!(body, "        let off = bb({}usize); unsafe {{ let p = bytes.as_mut_ptr(); let x = (p.add(off) as *const u32).read_unaligned(); (p.add(off + 5) as *mut u64).write_unaligned(x as u64 ^ {}); h.put(&x); }} h.put(&bytes); }}", rng.below(20), rng.next_u64());
}

fn bytes_and_niches(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let nv = rng.range(3, 6);
    w!(items, "    #[repr(u8)] #[derive(Clone, Copy, PartialEq, Debug)] enum E{id} {{ {} }}", (0..nv).map(|i| format!("V{i} = {}", i * 3 + rng.below(3))).collect::<Vec<_>>().join(", "));
    w!(items, "    impl E{id} {{ fn from_u8(x: u8) -> Option<E{id}> {{ match x {{ {} _ => None }} }} }}", (0..nv).map(|i| format!("x if x == E{id}::V{i} as u8 => Some(E{id}::V{i}),")).collect::<Vec<_>>().join(" "));
    w!(items, "    #[inline(never)]\n    fn next{id}(e: E{id}) -> E{id} {{ match e {{ {} }} }}", (0..nv).map(|i| format!("E{id}::V{i} => E{id}::V{},", (i + 1) % nv)).collect::<Vec<_>>().join(" "));
    w!(body, "        {{ let mut e = E{id}::V0; let mut seen = 0u64; for i in 0..bb({}u32) {{ e = next{id}(e); seen = seen.wrapping_mul(7).wrapping_add(e as u8 as u64 + i as u64); if let Some(f) = E{id}::from_u8(bb(e as u8)) {{ seen ^= (f == e) as u64; }} }} h.put(&seen); h.put(&E{id}::from_u8(bb({}u8)).is_some()); }}", rng.range(0, 12), rng.below(30));
    w!(body, "        {{ let x = bb({}u32); let nz = std::num::NonZeroU32::new(x); h.put(&nz.map(|n| n.get())); h.put(&std::mem::size_of::<Option<std::num::NonZeroU32>>());", rng.below(4));
    w!(body, "        let mut c = std::num::NonZeroU32::new(bb({}u32 | 1)).unwrap(); for _ in 0..bb({}u32) {{ c = std::num::NonZeroU32::new(c.get().wrapping_mul(3) | 1).unwrap(); }} h.put(&c.get());", rng.next_u64() as u32, rng.range(0, 30));
    w!(body, "        let boxed: Option<Box<u64>> = if bb({}) {{ Some(Box::new({})) }} else {{ None }}; h.put(&boxed.as_deref().copied()); h.put(&std::mem::size_of::<Option<Box<u64>>>());", rng.chance(50), rng.next_u64());
    w!(body, "        let r: Option<&u32> = if bb({}) {{ Some(&x) }} else {{ None }}; h.put(&r.copied()); }}", rng.chance(50));
    w!(body, "        {{ let v = bb({}u64); let a = v.to_ne_bytes(); let t: [u8; 8] = unsafe {{ std::mem::transmute(v) }}; h.put(&(a == t)); h.put(&u64::from_le_bytes(a)); let w: [u16; 4] = unsafe {{ std::mem::transmute(v) }}; h.put(&w); h.put(&u32::from_be_bytes([a[0], a[1], a[2], a[3]])); }}", rng.next_u64());
}

fn provenance(rng: &mut Rng, body: &mut String) {
    w!(body, "        {{ let a = [bb({}u8); 4]; let mut b = [bb({}u8); 4];", rng.below(256), rng.below(256));
    w!(body, "        let (pa, pb) = (a.as_ptr(), b.as_mut_ptr()); h.put(&(pa == bb(pa))); h.put(&(pa == pb as *const u8));");
    w!(body, "        let addr = bb(pb).expose_provenance(); let q = std::ptr::with_exposed_provenance_mut::<u8>(addr); unsafe {{ *q.add({}) = bb({}u8); }} h.put(&b); h.put(&unsafe {{ *pa.add(bb({}usize)) }}); }}", rng.below(4), rng.below(256), rng.below(4));
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    for id in 0..rng.range(2, 5) {
        match rng.below(7) {
            0 | 1 => union_siblings(rng, &mut items, &mut body, id),
            2 => packed(rng, &mut items, &mut body, id),
            3 => uninit_stack(rng, &mut items, &mut body, id),
            4 => raw_walk(rng, &mut items, &mut body, id),
            5 => bytes_and_niches(rng, &mut items, &mut body, id),
            _ => provenance(rng, &mut body),
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
