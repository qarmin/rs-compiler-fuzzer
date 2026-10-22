//! `stdlib` family: `BTreeMap`/`BTreeSet`, `BinaryHeap`, `LinkedList`, `HashMap`/`HashSet` (only order-independent
//! observations: lookups, sorted dumps), `Entry` API.
//! Prelude: `T` (key type), `a b c: T`, `v: Vec<T>`, `m: BTreeMap<T, u64>`, `s: BTreeSet<T>`, `k: usize`.

use super::stdlib::{bbv, emit, pick_table, wrap};
use super::{lit, small_ints};
use crate::rng::Rng;

pub const NAMES: &[&str] = &["bmap", "bset", "heap", "list", "hash"];

const BMAP: &[&str] = &[
    "let mut x = m.clone(); h.put(&x.insert(a, 1)); h.put(&x.insert(a, 2)); h.put(&x.get(&a)); h.put(&x.get(&b)); h.put(&x.contains_key(&c)); h.put(&x.remove(&a)); h.put(&x.remove_entry(&b)); h.put(&x.len()); h.put(&x.is_empty()); h.put(&x.get_key_value(&c)); h.put(&x.get_mut(&a).map(|p| { *p += 1; *p }));",
    "let mut x = m.clone(); *x.entry(a).or_insert(5) += 1; *x.entry(b).or_default() += 2; *x.entry(c).or_insert_with(|| 9) += 3; x.entry(a).and_modify(|p| *p *= 2).or_insert(0); h.put(&x); h.put(&x.entry(b).key().wrapping_add(0)); h.put(&x.len());",
    "let mut x = m.clone(); match x.entry(a) { std::collections::btree_map::Entry::Occupied(mut o) => { h.put(&o.get()); *o.get_mut() += 1; h.put(&o.key().wrapping_add(0)); h.put(&o.insert(7)); h.put(&o.remove()); } std::collections::btree_map::Entry::Vacant(v) => { h.put(&v.key().wrapping_add(0)); h.put(&*v.insert(3)); } } h.put(&x);",
    "h.put(&m.first_key_value()); h.put(&m.last_key_value()); let mut x = m.clone(); h.put(&x.pop_first()); h.put(&x.pop_last()); if let Some(mut f) = x.first_entry() { *f.get_mut() += 5; h.put(&f.key().wrapping_add(0)); } if let Some(l) = x.last_entry() { h.put(&l.remove()); } h.put(&x);",
    "h.put(&m.range(a.min(b)..=a.max(b)).collect::<Vec<_>>()); h.put(&m.range(a..).next()); h.put(&m.range(..b).next_back()); h.put(&m.range(a.min(c)..a.max(c)).count()); let mut x = m.clone(); for (_, v) in x.range_mut(a..) { *v += 1; } h.put(&x); h.put(&m.range((std::ops::Bound::Excluded(a), std::ops::Bound::Unbounded)).count());",
    "let mut x = m.clone(); for v in x.values_mut() { *v = v.wrapping_mul(3); } h.put(&x); for (k, v) in x.iter_mut() { *v = v.wrapping_add(*k as u64); } h.put(&x); x.retain(|k, v| (*k as u64 ^ *v) % 3 != 0); h.put(&x); h.put(&x.clone().into_values().collect::<Vec<_>>()); h.put(&x.clone().into_keys().collect::<Vec<_>>());",
    "let mut x = m.clone(); let y = x.split_off(&a); h.put(&x); h.put(&y); x.append(&mut y.clone()); h.put(&x); x.extend([(b, 1u64), (c, 2)]); x.extend(y.iter().map(|(k, v)| (*k, *v))); h.put(&x); h.put(&(x == y)); h.put(&x.cmp(&y)); h.put(&x.clone().into_iter().rev().take(2).collect::<Vec<_>>());",
    "let x: std::collections::BTreeMap<T, u64> = v.iter().map(|k| (*k, 1)).collect(); h.put(&x); let y = std::collections::BTreeMap::from([(a, 1u64), (b, 2), (c, 3)]); h.put(&y); h.put(&std::collections::BTreeMap::<T, u64>::new().first_key_value()); h.put(&y.iter().map(|(k, _)| k).copied().collect::<Vec<T>>());",
    "let mut x = m.clone(); let e: Vec<(T, u64)> = x.extract_if(.., |k, v| (*k as u64 ^ *v) % 2 == 0).collect(); h.put(&e); h.put(&x); use std::collections::btree_map::Entry; match x.entry(a) { Entry::Occupied(mut o) => { *o.get_mut() += 1; let r = o.into_mut(); *r += 2; h.put(&*r); } Entry::Vacant(v) => { let k = v.key().wrapping_add(0); let mut oe = v.insert_entry(9); h.put(&(k, *oe.get())); *oe.get_mut() += 1; } } h.put(&x); h.put(&*x.entry(b).or_insert_with_key(|k| *k as u64 & 7)); let k2 = match x.entry(c) { Entry::Vacant(v) => v.into_key(), Entry::Occupied(o) => *o.key() }; h.put(&k2);",
];

const BSET: &[&str] = &[
    "let mut x = s.clone(); h.put(&x.insert(a)); h.put(&x.insert(a)); h.put(&x.contains(&b)); h.put(&x.remove(&a)); h.put(&x.take(&b)); h.put(&x.replace(c)); h.put(&x.get(&c)); h.put(&x.len()); h.put(&x.is_empty()); h.put(&x);",
    "let t: std::collections::BTreeSet<T> = v.iter().copied().collect(); h.put(&s.union(&t).copied().collect::<Vec<_>>()); h.put(&s.intersection(&t).copied().collect::<Vec<_>>()); h.put(&s.difference(&t).copied().collect::<Vec<_>>()); h.put(&s.symmetric_difference(&t).copied().collect::<Vec<_>>());",
    "let t: std::collections::BTreeSet<T> = v.iter().copied().collect(); h.put(&s.is_subset(&t)); h.put(&s.is_superset(&t)); h.put(&s.is_disjoint(&t)); h.put(&(&s | &t)); h.put(&(&s & &t)); h.put(&(&s - &t)); h.put(&(&s ^ &t)); h.put(&(s == t)); h.put(&s.cmp(&t)); h.put(&s.partial_cmp(&t));",
    "h.put(&s.first()); h.put(&s.last()); let mut x = s.clone(); h.put(&x.pop_first()); h.put(&x.pop_last()); h.put(&x.range(a..).next()); h.put(&x.range(..=b).next_back()); h.put(&x.range(a.min(c)..a.max(c)).count()); h.put(&x.iter().rev().take(2).collect::<Vec<_>>());",
    "let mut x = s.clone(); let y = x.split_off(&a); h.put(&x); h.put(&y); x.append(&mut y.clone()); x.extend([a, b]); x.extend(v.iter()); h.put(&x); x.retain(|p| p % 2 == 0 || *p == a); h.put(&x); h.put(&x.clone().into_iter().collect::<Vec<_>>());",
    "let x: std::collections::BTreeSet<T> = std::collections::BTreeSet::from([a, b, c]); h.put(&x); h.put(&x.iter().copied().max()); h.put(&x.iter().copied().min()); h.put(&x.into_iter().map(|p| p.wrapping_add(1)).collect::<std::collections::BTreeSet<_>>());",
    "let mut x = s.clone(); let e: Vec<T> = x.extract_if(.., |p| p % 2 == 0).collect(); h.put(&e); h.put(&x); h.put(&x.is_subset(&s));",
];

const HEAP: &[&str] = &[
    "let mut x: std::collections::BinaryHeap<T> = v.iter().copied().collect(); h.put(&x.peek()); x.push(a); x.push(b); h.put(&x.pop()); h.put(&x.pop()); h.put(&x.len()); h.put(&x.is_empty()); h.put(&x.clone().into_sorted_vec()); h.put(&x.into_vec().len());",
    "let mut x: std::collections::BinaryHeap<T> = v.iter().copied().collect(); if let Some(mut t) = x.peek_mut() { *t = a; } h.put(&x.clone().into_sorted_vec()); let mut y = std::collections::BinaryHeap::from(vec![a, b, c]); x.append(&mut y); h.put(&y.len()); h.put(&x.into_sorted_vec()); ",
    "let mut x: std::collections::BinaryHeap<std::cmp::Reverse<T>> = v.iter().map(|p| std::cmp::Reverse(*p)).collect(); x.push(std::cmp::Reverse(a)); h.put(&x.pop().map(|r| r.0)); h.put(&x.peek().map(|r| r.0)); let mut out = Vec::new(); while let Some(std::cmp::Reverse(p)) = x.pop() { out.push(p); } h.put(&out);",
    "let mut x: std::collections::BinaryHeap<(T, T)> = v.iter().map(|p| (*p, p.wrapping_neg())).collect(); x.extend([(a, b)]); x.retain(|p| p.0 != c); h.put(&x.drain().count()); h.put(&x.len()); let y: std::collections::BinaryHeap<T> = std::collections::BinaryHeap::from([a, b, c]); h.put(&y.iter().copied().max()); h.put(&y.into_sorted_vec());",
    "let mut x = std::collections::BinaryHeap::<T>::with_capacity(k); for p in &v { x.push(*p); if x.len() > 3 { x.pop(); } } h.put(&x.into_sorted_vec()); let mut y = std::collections::BinaryHeap::from(v.clone()); y.clear(); h.put(&y.is_empty());",
];

const LIST: &[&str] = &[
    "let mut x: std::collections::LinkedList<T> = v.iter().copied().collect(); x.push_back(a); x.push_front(b); h.put(&x.front()); h.put(&x.back()); h.put(&x.pop_front()); h.put(&x.pop_back()); h.put(&x.len()); h.put(&x.contains(&c)); h.put(&x.iter().rev().copied().collect::<Vec<_>>());",
    "let mut x: std::collections::LinkedList<T> = v.iter().copied().collect(); let mut y = x.split_off(k.min(x.len())); h.put(&x.iter().count()); h.put(&y.iter().count()); x.append(&mut y); h.put(&y.is_empty()); h.put(&x); x.clear(); h.put(&x.is_empty());",
    "let mut x: std::collections::LinkedList<T> = v.iter().copied().collect(); for p in x.iter_mut() { *p = p.wrapping_add(a); } h.put(&x.iter().copied().collect::<Vec<_>>()); if let Some(f) = x.front_mut() { *f = b; } if let Some(l) = x.back_mut() { *l = c; } h.put(&x); h.put(&(x == x.clone())); h.put(&x.cmp(&x.clone()));",
    "let x: std::collections::LinkedList<T> = std::collections::LinkedList::from([a, b, c]); h.put(&x.into_iter().rev().collect::<Vec<_>>()); let mut y = std::collections::LinkedList::new(); y.extend(v.iter().copied()); h.put(&y.len());",
    "let mut x: std::collections::LinkedList<T> = v.iter().copied().collect(); let e: Vec<T> = x.extract_if(|p| *p % 2 == 0).collect(); h.put(&e); h.put(&x); *x.push_back_mut(a) += 1; *x.push_front_mut(b) += 1; h.put(&x);",
];

const HASH: &[&str] = &[
    "let mut x: std::collections::HashMap<T, u64> = m.iter().map(|(k, v)| (*k, *v)).collect(); h.put(&x.insert(a, 1)); h.put(&x.get(&a)); h.put(&x.contains_key(&b)); h.put(&x.remove(&c)); *x.entry(b).or_insert(0) += 5; h.put(&x.get(&b)); h.put(&x.len()); let mut ks: Vec<_> = x.keys().copied().collect(); ks.sort(); h.put(&ks); let mut vs: Vec<_> = x.values().copied().collect(); vs.sort(); h.put(&vs);",
    "let mut x: std::collections::HashMap<T, u64> = m.iter().map(|(k, v)| (*k, *v)).collect(); x.retain(|k, v| (*k as u64 ^ *v) % 2 == 0); let mut d: Vec<_> = x.drain().collect(); d.sort(); h.put(&d); h.put(&x.is_empty()); let y = std::collections::HashMap::from([(a, 1u8), (b, 2)]); h.put(&y.get(&a)); h.put(&y.len()); h.put(&y.get_key_value(&b));",
    "let x: std::collections::HashSet<T> = v.iter().copied().collect(); let y: std::collections::HashSet<T> = s.iter().copied().collect(); let mut u: Vec<_> = x.union(&y).copied().collect(); u.sort(); h.put(&u); let mut i: Vec<_> = x.intersection(&y).copied().collect(); i.sort(); h.put(&i); let mut d: Vec<_> = x.difference(&y).copied().collect(); d.sort(); h.put(&d); h.put(&x.is_subset(&y)); h.put(&x.is_disjoint(&y)); h.put(&x.len()); h.put(&x.contains(&a)); h.put(&(x == y));",
    "let mut x: std::collections::HashSet<T> = v.iter().copied().collect(); h.put(&x.insert(a)); h.put(&x.insert(a)); h.put(&x.remove(&b)); h.put(&x.take(&c)); h.put(&x.replace(a)); h.put(&x.get(&a)); x.retain(|p| p % 2 == 0); let mut o: Vec<_> = x.into_iter().collect(); o.sort(); h.put(&o);",
    "let mut x = std::collections::HashMap::<T, Vec<T>>::new(); for p in &v { x.entry(p % 3).or_default().push(*p); } let mut ks: Vec<_> = x.iter().map(|(k, v)| (*k, v.len())).collect(); ks.sort(); h.put(&ks); h.put(&x.values().map(|v| v.len()).sum::<usize>()); h.put(&x.get(&a).map(|v| v.len())); h.put(&(x.capacity() >= x.len()));",
    "let mut x: std::collections::HashMap<T, u64> = m.iter().map(|(k, v)| (*k, *v)).collect(); let mut e: Vec<(T, u64)> = x.extract_if(|k, v| (*k as u64 ^ *v) % 2 == 0).collect(); e.sort(); h.put(&e); let mut ks: Vec<_> = x.keys().copied().collect(); ks.sort(); h.put(&ks); h.put(&x.try_reserve(k).is_ok()); if a != b { h.put(&x.get_disjoint_mut([&a, &b]).iter().count()); } use std::collections::hash_map::Entry; match x.entry(c) { Entry::Occupied(o) => { *o.into_mut() += 1; } Entry::Vacant(v) => { let mut oe = v.insert_entry(4); *oe.get_mut() += 1; } } h.put(&x.get(&c)); let _ = x.hasher();",
    "let h1: std::collections::HashSet<T, std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>> = v.iter().copied().collect(); let mut o: Vec<_> = h1.iter().copied().collect(); o.sort(); h.put(&o); let mut m2 = std::collections::HashMap::<T, u8, std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>>::with_hasher(Default::default()); m2.insert(a, 1); h.put(&m2.get(&a)); let m3 = std::collections::HashMap::<T, u8, std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>>::with_capacity_and_hasher(k, Default::default()); h.put(&m3.len()); let s3 = std::collections::HashSet::<T, std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>>::with_hasher(Default::default()); h.put(&s3.is_empty()); let mut hs: std::collections::HashSet<T> = v.iter().copied().collect(); let mut e: Vec<T> = hs.extract_if(|p| p % 2 == 0).collect(); e.sort(); h.put(&e); h.put(&hs.try_reserve(k).is_ok()); let _ = hs.hasher();",
];

fn map_lit(rng: &mut Rng, ty: &str) -> String {
    let n = rng.range(0, 6);
    let items: Vec<String> = (0..n).map(|_| format!("({}, {}u64)", if rng.chance(50) { lit(rng, ty) } else { small_ints(rng, ty) }, rng.below(100))).collect();
    format!("std::collections::BTreeMap::from([{}])", items.join(", "))
}

fn set_lit(rng: &mut Rng, ty: &str) -> String {
    let n = rng.range(0, 6);
    let items: Vec<String> = (0..n).map(|_| if rng.chance(50) { lit(rng, ty) } else { small_ints(rng, ty) }).collect();
    format!("std::collections::BTreeSet::from([{}])", items.join(", "))
}

fn vec_lit(rng: &mut Rng, ty: &str) -> String {
    let n = rng.range(0, 7);
    let items: Vec<String> = (0..n).map(|_| if rng.chance(40) { lit(rng, ty) } else { small_ints(rng, ty) }).collect();
    format!("vec![{}]", items.join(", "))
}

pub fn generate(rng: &mut Rng) -> String {
    let ty = *rng.pick(&["u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "usize", "u128", "i128", "isize"]);
    let mut p = format!("        type T = {ty};\n");
    for v in ["a", "b", "c"] {
        let l = if rng.chance(50) { lit(rng, ty) } else { small_ints(rng, ty) };
        p += &format!("        let {v}: T = {};\n", bbv(rng, l));
    }
    let (v, m, s) = (vec_lit(rng, ty), map_lit(rng, ty), set_lit(rng, ty));
    p += &format!("        let v: Vec<T> = {}; let m: std::collections::BTreeMap<T, u64> = {}; let s: std::collections::BTreeSet<T> = {};\n", bbv(rng, v), bbv(rng, m), bbv(rng, s));
    let k = rng.below(7);
    p += &format!("        let k: usize = {};\n", bbv(rng, format!("{k}usize")));
    let n = rng.range(5, 10);
    let table = pick_table(rng, &[("bmap", BMAP), ("bmap", BMAP), ("bset", BSET), ("bset", BSET), ("heap", HEAP), ("list", LIST), ("hash", HASH), ("hash", HASH)]);
    let body = emit(rng, &p, &[table], n);
    wrap("", &body)
}
