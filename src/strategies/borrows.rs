//! Strategy `borrows`: safe-Rust programs that stress lifetimes and borrow-driven MIR
//! optimisations (ReferencePropagation, GVN, DestinationPropagation, SROA):
//! reborrow chains through nested fields, split borrows, views with explicit lifetimes,
//! `Option<&mut T>` cursors over linked lists, closures capturing by ref/mut/move,
//! two-phase borrows, `Cell`/`RefCell` aliasing, `mem::swap/replace/take`.
//!
//! There is no `unsafe` here, so any divergence is a compiler bug, not a UB question.

use super::{bits, lit, UINTS};
use crate::rng::Rng;
use crate::w;

fn ity(rng: &mut Rng) -> &'static str {
    *rng.pick(&["u8", "u16", "u32", "u64", "i32", "i64", "usize", "u128"])
}

/// Nested structs, `&mut` handed down through helper functions one field at a time.
fn reborrow_chain(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let depth = rng.range(2, 5);
    let ty = ity(rng);
    for d in 0..depth {
        let inner = if d == 0 { ty.to_string() } else { format!("L{id}_{}", d - 1) };
        w!(items, "    #[derive(Clone, Debug)] struct L{id}_{d} {{ tag: u8, inner: {inner}, other: {ty} }}");
    }
    let mut ctor = lit(rng, ty);
    for d in 0..depth {
        ctor = format!("L{id}_{d} {{ tag: {}, inner: {ctor}, other: {} }}", rng.below(256), lit(rng, ty));
    }
    let delta = lit(rng, ty);
    // helper chain
    for lvl in (0..depth).rev() {
        let this = format!("L{id}_{lvl}");
        let attr = *rng.pick(&["#[inline(never)]", "#[inline(always)]", ""]);
        if lvl == 0 {
            w!(items, "    {attr}\n    fn bump{id}_{lvl}(v: &mut {this}) -> &mut {ty} {{ v.other = v.other.wrapping_add({delta}); v.tag = v.tag.wrapping_add(1); &mut v.inner }}");
        } else {
            w!(items, "    {attr}\n    fn bump{id}_{lvl}(v: &mut {this}) -> &mut {ty} {{ v.tag ^= {}; bump{id}_{}(&mut v.inner) }}", rng.below(256), lvl - 1);
        }
    }
    let path = (0..depth).map(|_| ".inner").collect::<String>();
    w!(body, "        {{ let mut val = {ctor};");
    for _ in 0..rng.range(1, 3) {
        w!(body, "        {{ let r = bump{id}_{}(&mut val); *r = r.wrapping_mul({}); let r2 = &mut *r; *r2 = r2.wrapping_add({delta}); }}", depth - 1, lit(rng, ty));
    }
    w!(body, "        h.put(&bb(val{path})); h.put(&val.tag); h.put(&val.other);");
    let mut p = String::from("val");
    for _ in 0..depth {
        p += ".inner";
        w!(body, "        h.put(&{}.tag);", p.rsplit_once('.').unwrap().0);
    }
    w!(body, "        }}");
}

/// A view type with an explicit lifetime handing out sub-borrows.
fn view(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = ity(rng);
    let n = rng.range(4, 16);
    w!(items, "    struct View{id}<'a> {{ data: &'a mut [{ty}], pos: usize }}");
    w!(items, "    impl<'a> View{id}<'a> {{");
    w!(items, "        fn new(data: &'a mut [{ty}]) -> Self {{ View{id} {{ data, pos: 0 }} }}");
    w!(items, "        fn cur(&mut self) -> &mut {ty} {{ let p = self.pos % self.data.len(); &mut self.data[p] }}");
    w!(items, "        fn advance(&mut self, k: usize) -> &mut Self {{ self.pos = self.pos.wrapping_add(k); self }}");
    w!(items, "        fn split(self) -> (&'a mut [{ty}], &'a mut [{ty}]) {{ let m = self.data.len() / 2; self.data.split_at_mut(m) }}");
    w!(items, "        fn first_and_rest(&mut self) -> Option<(&mut {ty}, &mut [{ty}])> {{ self.data.split_first_mut() }}");
    w!(items, "    }}");
    w!(items, "    #[inline(never)]\n    fn pick{id}<'a>(a: &'a mut {ty}, b: &'a mut {ty}, c: bool) -> &'a mut {ty} {{ if c {{ a }} else {{ b }} }}");
    w!(body, "        {{ let mut arr = [0 as {ty}; {n}];");
    w!(body, "        for (i, x) in arr.iter_mut().enumerate() {{ *x = (i as {ty}).wrapping_mul({}); }}", lit(rng, ty));
    w!(body, "        {{ let mut v = View{id}::new(&mut arr);");
    for _ in 0..rng.range(2, 5) {
        match rng.below(4) {
            0 => w!(body, "        *v.advance(bb({}usize)).cur() ^= {};", rng.below(20), lit(rng, ty)),
            1 => w!(body, "        {{ let c = v.cur(); *c = c.wrapping_add({}); }}", lit(rng, ty)),
            2 => w!(body, "        if let Some((f, rest)) = v.first_and_rest() {{ *f = f.wrapping_add(rest.len() as {ty}); if let Some(l) = rest.last_mut() {{ *l = l.wrapping_mul({}); }} }}", lit(rng, ty)),
            _ => w!(body, "        v.advance({});", rng.below(7)),
        }
    }
    w!(body, "        let (l, r) = v.split();");
    w!(body, "        if !l.is_empty() && !r.is_empty() {{ let (a, b) = (&mut l[0], &mut r[0]); let p = pick{id}(a, b, bb({})); *p = p.wrapping_add({}); std::mem::swap(a_or{id}(l), b_or{id}(r)); }}", rng.chance(50), lit(rng, ty));
    w!(body, "        }}");
    w!(body, "        h.put(&arr); }}");
    // tiny helpers so the swap above borrows disjoint slices
    w!(items, "    fn a_or{id}<T>(s: &mut [T]) -> &mut T {{ &mut s[0] }}\n    fn b_or{id}<T>(s: &mut [T]) -> &mut T {{ &mut s[0] }}");
}

/// Singly linked list with a cursor walking `Option<Box<Node>>` by `&mut`.
fn linked_list(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = ity(rng);
    w!(items, "    struct Node{id} {{ v: {ty}, next: Option<Box<Node{id}>> }}");
    w!(items, "    struct List{id} {{ head: Option<Box<Node{id}>>, len: usize }}");
    w!(items, "    impl List{id} {{");
    w!(items, "        fn new() -> Self {{ List{id} {{ head: None, len: 0 }} }}");
    w!(items, "        fn push_front(&mut self, v: {ty}) {{ let old = self.head.take(); self.head = Some(Box::new(Node{id} {{ v, next: old }})); self.len += 1; }}");
    w!(items, "        fn push_back(&mut self, v: {ty}) {{ let mut cur = &mut self.head; while cur.is_some() {{ cur = &mut cur.as_mut().unwrap().next; }} *cur = Some(Box::new(Node{id} {{ v, next: None }})); self.len += 1; }}");
    w!(items, "        fn pop_front(&mut self) -> Option<{ty}> {{ self.head.take().map(|n| {{ self.head = n.next; self.len -= 1; n.v }}) }}");
    w!(items, "        fn reverse(&mut self) {{ let mut prev = None; let mut cur = self.head.take(); while let Some(mut n) = cur {{ cur = n.next.take(); n.next = prev; prev = Some(n); }} self.head = prev; }}");
    w!(items, "        fn remove_if(&mut self, f: impl Fn({ty}) -> bool) {{ let mut cur = &mut self.head; while cur.is_some() {{ if f(cur.as_ref().unwrap().v) {{ let next = cur.as_mut().unwrap().next.take(); *cur = next; self.len -= 1; }} else {{ cur = &mut cur.as_mut().unwrap().next; }} }} }}");
    w!(items, "        fn fold(&self, init: {ty}) -> {ty} {{ let mut acc = init; let mut cur = &self.head; while let Some(n) = cur {{ acc = acc.wrapping_mul(31).wrapping_add(n.v); cur = &n.next; }} acc }}");
    w!(items, "        fn nth_mut(&mut self, k: usize) -> Option<&mut {ty}> {{ let mut cur = self.head.as_mut(); for _ in 0..k {{ cur = cur?.next.as_mut(); }} cur.map(|n| &mut n.v) }}");
    w!(items, "    }}");
    w!(body, "        {{ let mut l = List{id}::new();");
    for _ in 0..rng.range(3, 10) {
        let op = match rng.below(7) {
            0 | 1 => format!("l.push_front({});", lit(rng, ty)),
            2 | 3 => format!("l.push_back({});", lit(rng, ty)),
            4 => "h.put(&l.pop_front());".to_string(),
            5 => "l.reverse();".to_string(),
            _ => format!("l.remove_if(|x| {}.wrapping_add(x) & 1 == {});", lit(rng, ty), rng.below(2)),
        };
        w!(body, "        {op}");
    }
    w!(body, "        if let Some(x) = l.nth_mut(bb({}usize)) {{ *x = x.wrapping_add(1); }}", rng.below(5));
    w!(body, "        h.put(&l.fold({})); h.put(&l.len); }}", lit(rng, ty));
}

/// Closures: capture by ref / mut ref / move, FnMut through generics, boxed dyn, returned impl Fn.
fn closures(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = ity(rng);
    w!(items, "    fn apply{id}<F: FnMut({ty}) -> {ty}>(mut f: F, n: u32, seed: {ty}) -> {ty} {{ let mut x = seed; for _ in 0..n {{ x = f(x); }} x }}");
    w!(items, "    fn make{id}(k: {ty}) -> impl Fn({ty}) -> {ty} {{ move |x| x.wrapping_mul(k) ^ k }}");
    w!(items, "    fn boxed{id}(k: {ty}, inc: bool) -> Box<dyn FnMut({ty}) -> {ty}> {{ let mut calls: {ty} = 0; if inc {{ Box::new(move |x| {{ calls = calls.wrapping_add(1); x.wrapping_add(k).wrapping_add(calls) }}) }} else {{ Box::new(move |x| {{ calls = calls.wrapping_add(2); x ^ k ^ calls }}) }} }}");
    w!(body, "        {{ let base: {ty} = bb({});", lit(rng, ty));
    w!(body, "        let mut counter: {ty} = 0;");
    w!(body, "        let r1 = apply{id}(|x| {{ counter = counter.wrapping_add(x ^ 1); x.wrapping_add(base) }}, bb({}), bb({}));", rng.range(0, 6), lit(rng, ty));
    w!(body, "        let f = make{id}(bb({})); let r2 = apply{id}(&f, bb({}), {});", lit(rng, ty), rng.range(0, 6), lit(rng, ty));
    w!(body, "        let mut b = boxed{id}(bb({}), bb({})); let r3 = apply{id}(&mut *b, bb({}), {});", lit(rng, ty), rng.chance(50), rng.range(0, 6), lit(rng, ty));
    w!(body, "        let mut fs: Vec<Box<dyn Fn({ty}) -> {ty}>> = Vec::new();");
    w!(body, "        for i in 0..bb({}u32) {{ let k = i as {ty}; fs.push(Box::new(move |x| x.wrapping_add(k).rotate_left(i))); }}", rng.range(0, 5));
    w!(body, "        let r4 = fs.iter().fold({}, |a, f| f(a));", lit(rng, ty));
    w!(body, "        h.put(&r1); h.put(&counter); h.put(&r2); h.put(&r3); h.put(&r4); }}");
}

/// Interior mutability + two-phase borrows + mem::{swap,replace,take}.
fn interior(rng: &mut Rng, items: &mut String, body: &mut String, id: usize) {
    let ty = ity(rng);
    w!(items, "    struct Holder{id} {{ c: std::cell::Cell<{ty}>, r: std::cell::RefCell<Vec<{ty}>>, s: String }}");
    w!(items, "    #[inline(never)]\n    fn poke{id}(a: &Holder{id}, b: &Holder{id}, x: {ty}) -> {ty} {{ let before = a.c.get(); b.c.set(b.c.get().wrapping_add(x)); let after = a.c.get(); before.wrapping_sub(after) }}");
    w!(body, "        {{ let hd = Holder{id} {{ c: std::cell::Cell::new({}), r: std::cell::RefCell::new(vec![]), s: String::new() }};", lit(rng, ty));
    w!(body, "        let d = poke{id}(&hd, &hd, bb({})); h.put(&d);", lit(rng, ty));
    w!(body, "        hd.r.borrow_mut().push(hd.c.get()); let n = hd.r.borrow().len(); hd.r.borrow_mut().push(n as {ty}); h.put(&*hd.r.borrow());");
    w!(body, "        let mut v: Vec<{ty}> = vec![{}, {}]; v.push(v.len() as {ty}); let l = v.len(); v[l - 1] = v[0].wrapping_add(v[1]); v.extend_from_slice(&[v[0], v[1]]);", lit(rng, ty), lit(rng, ty));
    w!(body, "        let mut a = v.clone(); let mut b = std::mem::take(&mut v); std::mem::swap(&mut a, &mut b); let c = std::mem::replace(&mut a, vec![{}]); h.put(&a); h.put(&b); h.put(&c); h.put(&v);", lit(rng, ty));
    w!(body, "        let mut s = hd.s.clone(); for i in 0..bb({}u32) {{ s.push(char::from(b'a' + (i % 26) as u8)); }} h.put(&s.len()); h.put(&s.bytes().fold(0u32, |a, b| a.wrapping_mul(7).wrapping_add(b as u32))); }}", rng.range(0, 12));
}

pub fn generate(rng: &mut Rng) -> String {
    let mut items = String::new();
    let mut body = String::new();
    let _ = (bits("u8"), UINTS.len());
    for id in 0..rng.range(2, 5) {
        match rng.below(6) {
            0 | 1 => reborrow_chain(rng, &mut items, &mut body, id),
            2 => view(rng, &mut items, &mut body, id),
            3 => linked_list(rng, &mut items, &mut body, id),
            4 => closures(rng, &mut items, &mut body, id),
            _ => interior(rng, &mut items, &mut body, id),
        }
    }
    format!("{items}    pub fn run(h: &mut H) {{\n{body}    }}\n")
}
